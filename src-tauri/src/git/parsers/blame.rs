//! `src/main/git/parsers/blame.ts` — `git blame --porcelain` 출력 파서.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{js_number_or_zero, JsNumber};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    /// epoch seconds
    pub author_date: JsNumber,
    pub summary: String,
    /// 이력의 끝(루트)이라 더 거슬러 갈 수 없는 커밋
    pub boundary: bool,
}

/// 같은 커밋이 연속으로 만든 줄 묶음. start 는 lines 의 0 기반 위치
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlameGroup {
    pub hash: String,
    pub start: usize,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedBlame {
    pub commits: BTreeMap<String, BlameCommit>,
    pub groups: Vec<BlameGroup>,
    pub lines: Vec<String>,
    /// 줄 안에 NUL 이 있었는지(바이너리)
    pub binary: bool,
}

/// 정규식 `^([0-9a-f]{40,64}) \d+ \d+(?: \d+)?$` — 머리 줄이면 해시
fn header_hash(line: &str) -> Option<&str> {
    let mut it = line.split(' ');
    let hash = it.next()?;
    if !(40..=64).contains(&hash.len()) || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let nums: Vec<&str> = it.collect();
    let ok = (2..=3).contains(&nums.len()) && nums.iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(hash)
}

/// 줄마다 `<해시> <원래 줄> <최종 줄> [<묶음 줄 수>]` 머리 → (그 커밋을 처음 만났을 때만) 메타 줄들 → `\t<내용>` 순서다.
/// 같은 커밋이 연속으로 만든 줄은 한 묶음(group)으로 합친다(원래 줄이 떨어져 있어도 최종 줄이 이어지면 같은 묶음).
pub fn parse_blame(buf: &[u8]) -> ParsedBlame {
    let text = String::from_utf8_lossy(buf);
    let mut commits: BTreeMap<String, BlameCommit> = BTreeMap::new();
    let mut groups: Vec<BlameGroup> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut binary = false;
    let mut awaiting: Option<String> = None;

    for line in text.split('\n') {
        let Some(hash) = awaiting.as_deref() else {
            if let Some(h) = header_hash(line) {
                commits.entry(h.to_string()).or_insert_with(|| BlameCommit {
                    hash: h.to_string(),
                    author_name: String::new(),
                    author_email: String::new(),
                    author_date: JsNumber(0.0),
                    summary: String::new(),
                    boundary: false,
                });
                awaiting = Some(h.to_string());
            }
            continue;
        };
        if let Some(content) = line.strip_prefix('\t') {
            if content.contains('\0') {
                binary = true;
            }
            match groups.last_mut() {
                Some(last) if last.hash == hash => last.count += 1,
                _ => groups.push(BlameGroup { hash: hash.to_string(), start: lines.len(), count: 1 }),
            }
            lines.push(content.to_string());
            awaiting = None;
            continue;
        }
        let Some(cur) = commits.get_mut(hash) else { continue };
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => cur.author_name = value.to_string(),
            // 앞의 '<' 와 뒤의 '>' 를 한 번씩만 뗀다
            "author-mail" => {
                let v = value.strip_prefix('<').unwrap_or(value);
                cur.author_email = v.strip_suffix('>').unwrap_or(v).to_string();
            }
            "author-time" => cur.author_date = js_number_or_zero(value),
            "summary" => cur.summary = value.to_string(),
            "boundary" => cur.boundary = true,
            _ => {}
        }
    }
    ParsedBlame { commits, groups, lines, binary }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_shapes() {
        let h = "a".repeat(40);
        assert!(header_hash(&format!("{h} 1 1")).is_some());
        assert!(header_hash(&format!("{h} 1 1 3")).is_some());
        assert!(header_hash(&format!("{h} 1")).is_none());
        assert!(header_hash(&format!("{h} 1 1 3 4")).is_none());
        assert!(header_hash(&format!("{h} 1 x")).is_none());
        assert!(header_hash(&format!("{} 1 1", "a".repeat(39))).is_none());
        assert!(header_hash(&format!("{} 1 1", "A".repeat(40))).is_none());
        assert!(header_hash(&format!("{} 1 1", "a".repeat(65))).is_none());
    }
}
