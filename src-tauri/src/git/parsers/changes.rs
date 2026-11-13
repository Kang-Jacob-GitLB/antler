//! `src/main/git/parsers/changes.ts` — `--name-status -z` · `--numstat -z` 출력과 둘의 병합.

use std::collections::HashMap;

use serde::Serialize;

use super::{js_number, JsNumber};

/// 파일 상태 한 글자
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FileStatus {
    A,
    M,
    D,
    R,
    C,
    T,
    U,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub status: FileStatus,
    /// 변경 후 경로(삭제면 삭제된 경로)
    pub path: String,
    /// 이름 변경·복사의 원래 경로
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// 이름 변경·복사 유사도(0~100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<JsNumber>,
    /// 바이너리면 null
    pub additions: Option<JsNumber>,
    pub deletions: Option<JsNumber>,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub status: FileStatus,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<JsNumber>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NumstatEntry {
    pub path: String,
    pub additions: Option<JsNumber>,
    pub deletions: Option<JsNumber>,
    pub binary: bool,
}

/// git 이 알려 주는 알 수 없는 상태 문자(X, B 등)는 수정으로 취급한다
fn to_status(letter: Option<char>) -> FileStatus {
    match letter {
        Some('A') => FileStatus::A,
        Some('D') => FileStatus::D,
        Some('R') => FileStatus::R,
        Some('C') => FileStatus::C,
        Some('T') => FileStatus::T,
        Some('U') => FileStatus::U,
        _ => FileStatus::M,
    }
}

fn split_nul_tokens(buf: &[u8]) -> Vec<String> {
    let mut t: Vec<String> = String::from_utf8_lossy(buf).split('\0').map(str::to_string).collect();
    // -z 출력은 NUL 로 끝나므로 마지막 빈 조각을 버린다
    if t.last().is_some_and(String::is_empty) {
        t.pop();
    }
    t
}

/// `--name-status -z` 출력. "M\0path\0", "R100\0old\0new\0", "C075\0old\0new\0".
pub fn parse_name_status(buf: &[u8]) -> Vec<StatusEntry> {
    let t = split_nul_tokens(buf);
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let code = &t[i];
        i += 1;
        let status = to_status(code.chars().next());
        if status == FileStatus::R || status == FileStatus::C {
            let (Some(old_path), Some(path)) = (t.get(i), t.get(i + 1)) else { break };
            i += 2;
            // 코드는 'R'·'C' 로 시작하므로 1바이트 뒤가 유사도다
            let sim = if code.len() > 1 { js_number(&code[1..]) } else { JsNumber(f64::NAN) };
            out.push(StatusEntry { status, path: path.clone(), old_path: Some(old_path.clone()), similarity: (!sim.0.is_nan()).then_some(sim) });
        } else {
            let Some(path) = t.get(i) else { break };
            i += 1;
            out.push(StatusEntry { status, path: path.clone(), old_path: None, similarity: None });
        }
    }
    out
}

/// `--numstat -z` 출력. "1\t2\tpath\0", 이름 변경은 "1\t2\t\0old\0new\0", 바이너리는 "-\t-\t".
pub fn parse_numstat(buf: &[u8]) -> Vec<NumstatEntry> {
    let t = split_nul_tokens(buf);
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let tok = &t[i];
        i += 1;
        let Some(a) = tok.find('\t') else { continue };
        let Some(b) = tok[a + 1..].find('\t').map(|p| a + 1 + p) else { continue };
        let (add, del) = (&tok[..a], &tok[a + 1..b]);
        let mut path = tok[b + 1..].to_string();
        if path.is_empty() {
            // 이름 변경·복사: 다음 두 토큰이 old, new
            i += 1;
            path = t.get(i).cloned().unwrap_or_default();
            i += 1;
            if path.is_empty() {
                continue;
            }
        }
        let binary = add == "-" && del == "-";
        out.push(NumstatEntry {
            path,
            additions: (!binary).then(|| js_number(add)),
            deletions: (!binary).then(|| js_number(del)),
            binary,
        });
    }
    out
}

/// name-status 순서를 유지하며 numstat 통계를 변경 후 경로로 합친다.
pub fn parse_changes(name_status: &[u8], numstat: &[u8]) -> Vec<FileChange> {
    let stats: HashMap<String, NumstatEntry> = parse_numstat(numstat).into_iter().map(|e| (e.path.clone(), e)).collect();
    parse_name_status(name_status)
        .into_iter()
        .map(|e| {
            let s = stats.get(&e.path);
            FileChange {
                status: e.status,
                additions: s.and_then(|s| s.additions),
                deletions: s.and_then(|s| s.deletions),
                binary: s.is_some_and(|s| s.binary),
                path: e.path,
                old_path: e.old_path,
                similarity: e.similarity,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_status_letters_become_modified() {
        let v = parse_name_status(b"X\0a\0B\0b\0\0c\0");
        assert!(v.iter().all(|e| e.status == FileStatus::M));
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn numstat_rename_uses_new_path() {
        let v = parse_numstat(b"1\t2\t\0old\0new\0");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "new");
    }
}
