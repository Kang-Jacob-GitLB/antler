//! `src/main/git/parsers/fileHistory.ts` — `git log --follow -z --name-status` 출력.

use serde::Serialize;

use super::log::FIELD_SEP;
use super::{js_number_or_zero, JsNumber};

const FIELD_COUNT: usize = 6;

/// `git log --follow` 에 붙일 출력 형식 인자. `--name-status -z` 와 함께 쓰면 커밋마다
/// `\x01<필드들>\0\n<상태>\0<경로>[\0<새 경로>]\0` 가 나온다(이름 변경 추적 결과가 경로에 담긴다).
pub const FILE_HISTORY_FORMAT_ARGS: [&str; 4] = ["-z", "--name-status", "-M", "--format=%x01%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub hash: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    /// epoch seconds
    pub author_date: JsNumber,
    pub subject: String,
    /// name-status 첫 글자: A 추가 · M 수정 · D 삭제 · R 이름 변경 · C 복사 · T 타입 변경
    pub status: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// 정규식 `\x01(?=[0-9a-f]{40,64}\x1f)` 로 나눈다 — SOH 뒤에 해시 모양이 이어질 때만 레코드 경계다
fn split_records(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut last = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b != 0x01 {
            continue;
        }
        let run = bytes[i + 1..].iter().take_while(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')).count();
        if (40..=64).contains(&run) && bytes.get(i + 1 + run) == Some(&0x1f) {
            out.push(&s[last..i]);
            last = i + 1;
        }
    }
    out.push(&s[last..]);
    out
}

/// `git log --follow -z --name-status` 출력을 커밋 목록으로 바꾼다.
/// `fallback_path`: name-status 가 없는 레코드(예: 상태 줄이 생략된 병합)에 쓸 요청 경로.
pub fn parse_file_history(buf: &[u8], fallback_path: &str) -> Vec<FileHistoryEntry> {
    let text = String::from_utf8_lossy(buf);
    let mut out = Vec::new();
    for chunk in split_records(&text) {
        if chunk.is_empty() {
            continue;
        }
        let nul = chunk.find('\0');
        let head = nul.map_or(chunk, |i| &chunk[..i]);
        let f: Vec<&str> = head.split(FIELD_SEP).collect();
        if f.len() < FIELD_COUNT {
            continue;
        }
        let subject = f[FIELD_COUNT - 1..].join(FIELD_SEP);
        let rest = nul.map_or("", |i| chunk[i + 1..].trim_start_matches('\n'));
        let parts: Vec<&str> = rest.split('\0').collect();
        let status = parts[0].chars().next().map_or_else(|| "M".to_string(), String::from);
        let renamed = status == "R" || status == "C";
        let non_empty = |i: usize| parts.get(i).copied().filter(|p| !p.is_empty());
        let path = non_empty(if renamed { 2 } else { 1 }).unwrap_or(fallback_path);
        let old_path = if renamed { non_empty(1) } else { None };
        out.push(FileHistoryEntry {
            hash: f[0].to_string(),
            parents: if f[1].is_empty() { Vec::new() } else { f[1].split(' ').map(str::to_string).collect() },
            author_name: f[2].to_string(),
            author_email: f[3].to_string(),
            author_date: js_number_or_zero(f[4]),
            subject,
            status,
            path: path.to_string(),
            old_path: old_path.map(str::to_string),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soh_without_hash_is_not_a_boundary() {
        let h = "a".repeat(40);
        let text = format!("\x01{h}\x1f\x1fn\x1fe\x1f1\x1fsubject \x01notahash\0\nM\0p\0");
        let v = parse_file_history(text.as_bytes(), "fb");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].subject, "subject \x01notahash");
        assert_eq!(v[0].path, "p");
    }
}
