//! `src/main/git/parsers/log.ts`

use serde::Serialize;

use super::nul_records::split_nul;
use super::{js_number, js_trim, JsNumber};

/// 필드 구분자 US(0x1f). 커밋 사이는 `-z` 의 NUL 로 구분된다
pub const FIELD_SEP: &str = "\x1f";

const FIELD_COUNT: usize = 8;

/// `git log` 에 붙일 출력 형식 인자
pub const LOG_FORMAT_ARGS: [&str; 3] = ["-z", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%D%x1f%s", "--decorate=full"];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefLabel {
    pub kind: RefKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// epoch seconds
    pub author_date: JsNumber,
    /// epoch seconds
    pub commit_date: JsNumber,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

fn label(kind: RefKind, name: &str) -> RefLabel {
    RefLabel { kind, name: name.to_string() }
}

/// `%D` + `--decorate=full` 출력을 ref 라벨로 변환한다.
/// 예: "HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1.0"
pub fn parse_decorations(raw: &str) -> Vec<RefLabel> {
    if js_trim(raw).is_empty() {
        return Vec::new();
    }
    let mut refs = Vec::new();
    for part in raw.split(", ") {
        let item = js_trim(part);
        if item.is_empty() {
            continue;
        }
        if item == "HEAD" {
            refs.push(label(RefKind::Head, "HEAD"));
        } else if let Some(target) = item.strip_prefix("HEAD -> ") {
            // head 라벨의 name 은 HEAD 가 가리키는 브랜치 이름(분리된 HEAD 면 'HEAD')
            refs.push(label(RefKind::Head, target.strip_prefix("refs/heads/").unwrap_or(target)));
            push_ref(&mut refs, target);
        } else if let Some(tag) = item.strip_prefix("tag: ") {
            push_ref(&mut refs, tag);
        } else {
            push_ref(&mut refs, item);
        }
    }
    refs
}

fn push_ref(refs: &mut Vec<RefLabel>, full: &str) {
    if let Some(n) = full.strip_prefix("refs/heads/") {
        refs.push(label(RefKind::Local, n));
    } else if let Some(n) = full.strip_prefix("refs/remotes/") {
        // origin/HEAD 같은 심볼릭 리모트 HEAD 는 라벨로 쓰지 않는다
        if !n.ends_with("/HEAD") {
            refs.push(label(RefKind::Remote, n));
        }
    } else if let Some(n) = full.strip_prefix("refs/tags/") {
        refs.push(label(RefKind::Tag, n));
    }
    // refs/stash 등 나머지는 그래프 라벨로 쓰지 않는다
}

pub fn parse_log_record(record: &str) -> Option<Commit> {
    // 레코드 앞에 남은 개행(일부 git 버전의 구분자 차이)을 걷어낸다
    let rec = record.trim_start_matches('\n');
    if rec.is_empty() {
        return None;
    }
    let f: Vec<&str> = rec.split(FIELD_SEP).collect();
    if f.len() < FIELD_COUNT {
        return None;
    }
    Some(Commit {
        hash: f[0].to_string(),
        parents: if f[1].is_empty() { Vec::new() } else { f[1].split(' ').map(str::to_string).collect() },
        author_name: f[2].to_string(),
        author_email: f[3].to_string(),
        author_date: js_number(f[4]),
        commit_date: js_number(f[5]),
        // 제목에 US 문자가 들어간 극단적 경우를 대비해 나머지를 합친다
        subject: f[FIELD_COUNT - 1..].join(FIELD_SEP),
        refs: parse_decorations(f[6]),
    })
}

pub fn parse_log(buf: &[u8]) -> Vec<Commit> {
    split_nul(buf).iter().filter_map(|r| parse_log_record(r)).collect()
}
