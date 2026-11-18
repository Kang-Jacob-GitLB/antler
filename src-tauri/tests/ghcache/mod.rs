//! GitHub 캐시 시험(`github_cache_exec.rs`·`github_cache_isolation.rs`)의 공용 도구. 실제 git 을 띄우되 네트워크는 쓰지 않는다
//! (`file:///` 소스 저장소와 127.0.0.1 가짜 서버뿐).
//!
//! **준비용 git 은 항상 깨끗한 전역 설정**(`tests/gitconfig`)으로 돈다 — 각 시험 바이너리의 프로세스 전역 설정(`isolate`)이 적대적일 수 있어서다.
//! 제품 코드의 실행기를 거치지 않는 날 프로세스는 이 파일의 `raw` 뿐이다.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use antler_lib::git::exec::{ExecError, GitExecutor, GithubExecutor, RunOpts, RunResult};
use antler_lib::git::fetch::run::GitRunner;
use antler_lib::git::github_cache::GithubCachePath;
use antler_lib::git::github_link::GithubLink;
use antler_lib::net::redact::SecretString;

pub fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn repo_root() -> PathBuf {
    manifest().parent().expect("src-tauri 의 상위").to_path_buf()
}

/// 준비용 날 프로세스. 실행기를 거치지 않는다 — 제품 코드는 이 길을 쓰지 않는다(clippy.toml 의 disallowed-methods)
#[allow(clippy::disallowed_methods)]
pub fn raw(program: &str) -> std::process::Command {
    let mut c = std::process::Command::new(program);
    // 깨끗한 전역 설정(프로세스 전역이 적대적이어도 준비 단계는 영향을 받지 않는다)
    c.env("GIT_CONFIG_GLOBAL", manifest().join("tests").join("gitconfig")).env("GIT_CONFIG_NOSYSTEM", "1").env_remove("GIT_DEFAULT_REF_FORMAT");
    // 시험 바이너리가 부모 환경을 적대적으로 만들 수 있다(`github_cache_env.rs`) — 준비 단계는 받지 않는다
    for k in ["GIT_COMMON_DIR", "GIT_TRACE", "GIT_TRACE_CURL", "GIT_TRACE2_EVENT", "GIT_TRACE_REDACT", "GIT_CURL_VERBOSE", "GIT_SSL_NO_VERIFY", "GIT_NAMESPACE", "GIT_ALTERNATE_OBJECT_DIRECTORIES"] {
        c.env_remove(k);
    }
    c
}

pub fn raw_git(dir: &Path, args: &[&str]) -> String {
    let out = raw("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().expect("git 실행");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-ghc-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn str(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // 읽기 전용 pack 때문에 실패할 수 있다 — 임시 폴더라 남아도 무해하다
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `file:///C:/…` (공백 등은 퍼센트 인코딩)
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let enc: String = s.chars().map(|c| if c == ' ' { "%20".to_string() } else { c.to_string() }).collect();
    format!("file:///{}", enc.trim_start_matches('/'))
}

pub struct Source {
    /// 이 폴더 아래 `<owner>/<repo>.git` 가 소스다(`github_remote_url(origin = file_url(srv))` 가 그대로 가리킨다)
    pub srv: TempDir,
    pub first: String,
    pub second: String,
}

impl Source {
    pub fn origin(&self) -> String {
        file_url(self.srv.path())
    }

    pub fn dir(&self, owner: &str, repo: &str) -> PathBuf {
        self.srv.path().join(owner).join(format!("{repo}.git"))
    }
}

/// partial clone 필터를 허용하는 소스 저장소(`<srv>/<owner>/<repo>.git`). 커밋 둘, 파일 하나가 두 번 바뀐다
pub fn make_source(owner: &str, repo: &str) -> Source {
    let srv = TempDir::new("srv");
    let dir = srv.path().join(owner).join(format!("{repo}.git"));
    fs::create_dir_all(&dir).unwrap();
    let out = raw("git").args(["init", "-q", "-b", "main"]).arg(&dir).output().expect("git init");
    assert!(out.status.success());
    raw_git(&dir, &["config", "uploadpack.allowFilter", "true"]);
    raw_git(&dir, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    fs::write(dir.join("f.txt"), "one\n").unwrap();
    raw_git(&dir, &["add", "f.txt"]);
    raw_git(&dir, &["commit", "-q", "-m", "first"]);
    let first = raw_git(&dir, &["rev-parse", "HEAD"]);
    fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
    raw_git(&dir, &["commit", "-q", "-am", "second"]);
    let second = raw_git(&dir, &["rev-parse", "HEAD"]);
    Source { srv, first, second }
}

/// 캐시 기준 폴더·훅 폴더·저장소 폴더(`<cache>/<소문자 owner>/<소문자 repo>.git`, 만들어 둔다)와 그 위의 GitHub 실행기
pub struct Cache {
    pub root: TempDir,
    pub cache_dir: String,
    pub hooks: String,
}

impl Cache {
    pub fn new() -> Self {
        let root = TempDir::new("cache");
        let hooks = root.path().join("no-hooks");
        fs::create_dir(&hooks).unwrap();
        let cache = root.path().join("remote-repos");
        fs::create_dir(&cache).unwrap();
        Cache { cache_dir: cache.to_string_lossy().into_owned(), hooks: hooks.to_string_lossy().into_owned(), root }
    }

    /// 저장소 폴더를 만들고 캐시 경로로 돌려준다
    pub fn repo(&self, owner: &str, repo: &str) -> GithubCachePath {
        let dir = Path::new(&self.cache_dir).join(owner.to_lowercase()).join(format!("{}.git", repo.to_lowercase()));
        fs::create_dir_all(&dir).unwrap();
        GithubCachePath::new(&self.cache_dir, &dir.to_string_lossy()).expect("캐시 모양")
    }

    pub fn executor(&self, protocol: &str, token: Option<&'static str>) -> GitExecutor {
        GitExecutor::Github(
            GithubExecutor::new("git", self.cache_dir.clone(), self.hooks.clone(), std::sync::Arc::new(move || token.map(SecretString::new)), std::sync::Arc::new(Vec::new))
                .with_protocol_for_test(protocol),
        )
    }
}

pub fn link(owner: &str, repo: &str, branch: Option<&str>) -> GithubLink {
    GithubLink { owner: owner.to_string(), repo: repo.to_string(), default_branch: branch.map(str::to_string) }
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

pub type Reply = (i32, String, String);

/// 가짜 실행기: 명령(`args.join(" ")`)별 응답(없는 명령은 `default_code`)이나 호출 순서대로의 응답(모자라면 성공)을 주고, 호출한 argv 를 남긴다. 실제 프로세스는 없다
pub struct Fake {
    kind: &'static str,
    script: HashMap<String, Reply>,
    default_code: i32,
    sequence: Option<Mutex<VecDeque<Reply>>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl Fake {
    pub fn scripted(kind: &'static str, script: HashMap<String, Reply>, default_code: i32) -> Self {
        Fake { kind, script, default_code, sequence: None, calls: Mutex::new(Vec::new()) }
    }

    pub fn sequenced(kind: &'static str, replies: Vec<Reply>) -> Self {
        Fake { kind, script: HashMap::new(), default_code: 0, sequence: Some(Mutex::new(replies.into())), calls: Mutex::new(Vec::new()) }
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl GitRunner for Fake {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn run(&self, _repo_path: &str, args: &[String], _opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        self.calls.lock().unwrap().push(args.to_vec());
        let (code, stdout, stderr) = match &self.sequence {
            Some(q) => q.lock().unwrap().pop_front().unwrap_or((0, String::new(), String::new())),
            None => self.script.get(&args.join(" ")).cloned().unwrap_or((self.default_code, String::new(), String::new())),
        };
        async move { Ok(RunResult { stdout: stdout.into_bytes(), stderr, code }) }
    }
}
