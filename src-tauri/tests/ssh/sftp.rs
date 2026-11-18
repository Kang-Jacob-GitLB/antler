//! 시험 서버의 SFTP 하위 시스템(`server.rs` 가 `Opts::sftp` 로 켠다) — 메모리 위의 가짜 파일 트리다. 실제 파일 시스템·사용자 홈은 건드리지 않는다.
//! 심볼릭 링크를 Windows 권한 없이 만들 수 있고, 적대적인 서버 응답(너무 긴 이름·`/` 가 든 이름·가짜 `realpath`·끝없는 목록)을 그대로 보낼 수 있다.
//! 읽기 요청(`init`·`realpath`·`opendir`·`readdir`·`close`·`stat`)만 답한다. **쓰기 계열 요청은 `OpUnsupported`** 이고 그 횟수를 센다(제품 코드가 쓰기를 보내지 않는지 본다).

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use russh_sftp::protocol::{Attrs, File, FileAttributes, Handle, Name, Status, StatusCode};
use russh_sftp::server::Handler;

/// 서버가 SFTP 하위 시스템 요청에 답하는 방식
#[derive(Clone)]
pub enum Sftp {
    /// 하위 시스템을 거부한다(SFTP 를 막은 서버)
    Off,
    /// 가짜 트리를 서비스한다
    Tree(Arc<FakeTree>),
    /// 수락한 뒤 크기가 상한을 넘는 패킷 머리와 끝없는 쓰레기를 보낸다(클라이언트 파서가 끝나도 서버는 계속 보낸다)
    Garbage,
    /// 수락하고 아무 응답도 하지 않는다
    Silent,
}

#[derive(Clone)]
pub enum Node {
    Dir,
    File,
    /// 대상 경로(절대)
    Link(String),
}

/// 메모리 파일 트리와 적대 응답 설정
pub struct FakeTree {
    pub home: String,
    nodes: BTreeMap<String, Node>,
    /// `realpath` 가 항상 이 값을 돌려준다(적대 서버)
    pub realpath_override: Option<String>,
    /// 모든 목록에 덧붙는 날 항목(이름, 퍼미션 비트) — `/` 가 든 이름·너무 긴 이름 등
    pub extra_entries: Vec<(String, u32)>,
    /// 모든 `readdir` 가 `.`·`..` 만 돌려준다(EOF 를 보내지 않는다 — 적대 서버)
    pub dots_only: bool,
    /// 서버가 받은 요청 수(종류별)
    pub counts: Counts,
}

#[derive(Default)]
pub struct Counts {
    pub opendir: AtomicUsize,
    pub stat: AtomicUsize,
    pub unsupported: AtomicUsize,
}

fn norm(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    format!("/{}", stack.join("/"))
}

fn parent(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

impl FakeTree {
    pub fn new(home: &str) -> FakeTree {
        let mut t = FakeTree { home: home.to_string(), nodes: BTreeMap::new(), realpath_override: None, extra_entries: Vec::new(), dots_only: false, counts: Counts::default() };
        t.nodes.insert("/".to_string(), Node::Dir);
        t.dir(home);
        t
    }

    fn put(&mut self, path: &str, node: Node) {
        let p = norm(path);
        let mut up = parent(&p);
        while !self.nodes.contains_key(&up) {
            self.nodes.insert(up.clone(), Node::Dir);
            up = parent(&up);
        }
        self.nodes.insert(p, node);
    }

    pub fn dir(&mut self, path: &str) -> &mut Self {
        self.put(path, Node::Dir);
        self
    }

    pub fn file(&mut self, path: &str) -> &mut Self {
        self.put(path, Node::File);
        self
    }

    pub fn link(&mut self, path: &str, target: &str) -> &mut Self {
        self.put(path, Node::Link(norm(target)));
        self
    }

    /// 상대(`.`·`./x`)는 홈 기준으로 푼다
    fn resolve(&self, path: &str) -> String {
        if path == "." {
            self.home.clone()
        } else if let Some(rest) = path.strip_prefix("./") {
            norm(&format!("{}/{rest}", self.home))
        } else if path.starts_with('/') {
            norm(path)
        } else {
            norm(&format!("{}/{path}", self.home))
        }
    }

    /// 링크를 따라간 최종 노드(최대 16단계)
    fn follow(&self, path: &str) -> Option<Node> {
        let mut cur = norm(path);
        for _ in 0..16 {
            match self.nodes.get(&cur)? {
                Node::Link(t) => cur = t.clone(),
                n => return Some(n.clone()),
            }
        }
        None
    }

    fn children(&self, dir: &str) -> Vec<File> {
        let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
        let mut out: Vec<File> = self
            .nodes
            .iter()
            .filter(|(p, _)| p.len() > prefix.len() && p.starts_with(&prefix) && !p[prefix.len()..].contains('/'))
            .map(|(p, n)| File::new(&p[prefix.len()..], attrs_of(n)))
            .collect();
        out.extend(self.extra_entries.iter().map(|(name, perm)| File::new(name.clone(), FileAttributes { permissions: Some(*perm), ..FileAttributes::empty() })));
        out
    }
}

fn attrs_of(n: &Node) -> FileAttributes {
    let perm = match n {
        Node::Dir => 0o040_755,
        Node::File => 0o100_644,
        Node::Link(_) => 0o120_777,
    };
    FileAttributes { permissions: Some(perm), ..FileAttributes::empty() }
}

/// 연결 하나의 SFTP 서버 상태
pub struct FakeSftp {
    pub tree: Arc<FakeTree>,
    handles: HashMap<String, (Vec<File>, usize)>,
    next: usize,
}

impl FakeSftp {
    pub fn new(tree: Arc<FakeTree>) -> FakeSftp {
        FakeSftp { tree, handles: HashMap::new(), next: 0 }
    }
}

/// 한 번의 `readdir` 응답에 싣는 항목 수(실제 서버처럼 여러 번에 나눠 준다)
const BATCH: usize = 100;

impl Handler for FakeSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        self.tree.counts.unsupported.fetch_add(1, Ordering::SeqCst);
        StatusCode::OpUnsupported
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        if let Some(over) = &self.tree.realpath_override {
            return Ok(Name { id, files: vec![File::dummy(over.clone())] });
        }
        let resolved = self.tree.resolve(&path);
        // OpenSSH 처럼 없는 경로는 오류
        if self.tree.follow(&resolved).is_none() {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Name { id, files: vec![File::dummy(resolved)] })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        self.tree.counts.opendir.fetch_add(1, Ordering::SeqCst);
        let resolved = self.tree.resolve(&path);
        if !matches!(self.tree.follow(&resolved), Some(Node::Dir)) {
            return Err(StatusCode::NoSuchFile);
        }
        // 실제 서버처럼 `.`·`..` 가 앞에 온다
        let mut entries = vec![File::new(".", attrs_of(&Node::Dir)), File::new("..", attrs_of(&Node::Dir))];
        entries.extend(self.tree.children(&resolved));
        self.next += 1;
        let handle = format!("h{}", self.next);
        self.handles.insert(handle.clone(), (entries, 0));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        if self.tree.dots_only {
            return Ok(Name { id, files: vec![File::new(".", attrs_of(&Node::Dir)), File::new("..", attrs_of(&Node::Dir))] });
        }
        let (entries, pos) = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        if *pos >= entries.len() {
            return Err(StatusCode::Eof);
        }
        let end = (*pos + BATCH).min(entries.len());
        let files = entries[*pos..end].to_vec();
        *pos = end;
        Ok(Name { id, files })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.handles.remove(&handle);
        Ok(Status { id, status_code: StatusCode::Ok, error_message: String::new(), language_tag: String::new() })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        self.tree.counts.stat.fetch_add(1, Ordering::SeqCst);
        let resolved = self.tree.resolve(&path);
        match self.tree.follow(&resolved) {
            Some(n) => Ok(Attrs { id, attrs: attrs_of(&n) }),
            None => Err(StatusCode::NoSuchFile),
        }
    }
}
