//! `src/main/git/parsers/stash.ts`

use serde::Serialize;

use super::log::FIELD_SEP;
use super::nul_records::split_nul;
use super::{js_number, JsNumber};

/// `git stash list` 인자. %gd 는 stash@{n}, %gs 는 reflog 제목("On main: 메시지")
pub const STASH_ARGS: [&str; 4] = ["stash", "list", "-z", "--format=%H%x1f%gd%x1f%ct%x1f%gs"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stash {
    pub index: JsNumber,
    /// stash@{0}
    #[serde(rename = "ref")]
    pub ref_: String,
    pub hash: String,
    pub message: String,
    /// epoch seconds
    pub date: JsNumber,
}

/// `stash@{숫자}` 의 숫자(정규식 `^stash@\{(\d+)\}$`)
fn stash_index(r: &str) -> Option<&str> {
    let n = r.strip_prefix("stash@{")?.strip_suffix('}')?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

pub fn parse_stashes(buf: &[u8]) -> Vec<Stash> {
    let mut out = Vec::new();
    for rec in split_nul(buf) {
        let f: Vec<&str> = rec.trim_start_matches('\n').split(FIELD_SEP).collect();
        if f.len() < 4 {
            continue;
        }
        let Some(n) = stash_index(f[1]) else { continue };
        out.push(Stash {
            index: js_number(n),
            ref_: f[1].to_string(),
            hash: f[0].to_string(),
            date: js_number(f[2]),
            message: f[3..].join(FIELD_SEP),
        });
    }
    out
}
