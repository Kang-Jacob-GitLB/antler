//! `src/main/git/parsers/refs.ts`

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use super::{js_number, js_trim, JsNumber};

/// `git for-each-ref` 에 붙일 출력 형식. 필드는 NUL(%00), ref 하나는 한 줄
pub const REFS_FORMAT: &str = "%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)%00%(upstream:short)%00%(upstream:track,nobracket)\
%00%(symref)%00%(objecttype)%00%(*objecttype)%00%(creatordate:unix)";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    /// 짧은 이름. 로컬: feature/x, 리모트: origin/feature/x
    pub name: String,
    /// refs/heads/feature/x, refs/remotes/origin/feature/x
    pub full_name: String,
    pub hash: String,
    /// 로컬 브랜치의 업스트림 짧은 이름(origin/feature/x)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<JsNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<JsNumber>,
    /// 업스트림이 설정됐지만 원격에서 사라짐
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_gone: Option<bool>,
    pub is_head: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RemoteGroup {
    /// 리모트 이름(origin)
    pub name: String,
    /// 이 리모트의 브랜치들. name 은 'origin/feature/x' 형식, origin/HEAD 는 제외
    pub branches: Vec<Branch>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub name: String,
    pub full_name: String,
    /// 태그가 가리키는 커밋(annotated 태그는 peel 한 커밋 해시)
    pub hash: String,
    pub annotated: bool,
    /// 최종 대상이 커밋인지(트리·블롭·중첩 태그면 false — 그래프에서 찾아갈 수 없다)
    pub targets_commit: bool,
    /// 만든 시각(unix 초). annotated 는 태그 시각, lightweight 는 커밋 시각
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<JsNumber>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedRefs {
    pub local: Vec<Branch>,
    pub remotes: Vec<RemoteGroup>,
    pub tags: Vec<Tag>,
}

/// 로컬 브랜치의 업스트림 ahead/behind/gone(`RefsDetails['tracking']` 의 값)
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracking {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<JsNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<JsNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_gone: Option<bool>,
}

// ---------- 자연 정렬 ----------

/// CLDR root 정렬에서 ASCII 기호의 순서(공백이 가장 앞, 숫자·글자보다 앞)
const SYMBOL_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Symbol(usize),
    /// 숫자 덩어리: 앞의 0 을 뗀 자릿수, 그 숫자 문자열(길이가 같으면 사전순 = 수치순)
    Number(usize, String),
    Letter(u32),
}

fn sort_keys(s: &str) -> Vec<SortKey> {
    let mut keys = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut run = String::from(c);
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                run.push(d);
            }
            let trimmed = run.trim_start_matches('0');
            keys.push(SortKey::Number(trimmed.len(), trimmed.to_string()));
        } else if let Some(i) = SYMBOL_ORDER.find(c) {
            keys.push(SortKey::Symbol(i));
        } else if c.is_ascii_control() {
            // 제어 문자는 정렬에서 무시된다
        } else {
            keys.push(SortKey::Letter(c.to_lowercase().next().map_or(0, u32::from)));
        }
    }
    keys
}

/// `a.localeCompare(b, undefined, { numeric: true })` 의 근사. ICU 전체를 옮긴 것이 아니다 —
/// 숫자 덩어리는 수치로, 글자는 대소문자 무시(같으면 소문자가 먼저), ASCII 기호는 CLDR root 순서, ASCII 밖 글자는 코드 포인트 순이다.
/// 골든(tests/golden/parsers)에 든 이름만 TS 와 같음을 확인했다.
pub fn natural_compare(a: &str, b: &str) -> Ordering {
    sort_keys(a).cmp(&sort_keys(b)).then_with(|| {
        // 1차(글자·숫자)가 같을 때만: 처음 다른 대소문자는 소문자가 앞
        for (x, y) in a.chars().zip(b.chars()) {
            if x != y && x.to_lowercase().eq(y.to_lowercase()) {
                return if x.is_lowercase() { Ordering::Less } else { Ordering::Greater };
            }
        }
        Ordering::Equal
    })
}

// ---------- 해석 ----------

/// `(\d+)` 앞에 `label` 이 붙은 첫 위치의 숫자(정규식 `/ahead (\d+)/` 의 exec)
fn count_after(t: &str, label: &str) -> Option<JsNumber> {
    t.match_indices(label).find_map(|(i, _)| {
        let rest = &t[i + label.len()..];
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        (n > 0).then(|| js_number(&rest[..n]))
    })
}

/// `%(upstream:track,nobracket)` 출력("ahead 1, behind 2" / "ahead 3" / "behind 1" / "gone") 해석
pub fn parse_track(track: &str) -> Tracking {
    let t = js_trim(track);
    if t.is_empty() {
        return Tracking::default();
    }
    if t == "gone" {
        return Tracking { upstream_gone: Some(true), ..Tracking::default() };
    }
    Tracking { ahead: count_after(t, "ahead "), behind: count_after(t, "behind "), upstream_gone: None }
}

/// `git remote` 출력(줄 단위)에서 리모트 이름 목록을 뽑는다
pub fn parse_remote_names(buf: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(buf)
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `REFS_TRACK_ARGS` 출력 → 브랜치 전체 이름별 ahead/behind/gone(변화 없는 브랜치는 넣지 않는다)
pub fn parse_tracking(buf: &[u8]) -> BTreeMap<String, Tracking> {
    let mut out = BTreeMap::new();
    for line in String::from_utf8_lossy(buf).split('\n') {
        let mut it = line.split('\0');
        let full_name = it.next().unwrap_or("");
        let track = it.next().unwrap_or("");
        if full_name.is_empty() || js_trim(track).is_empty() {
            continue;
        }
        out.insert(full_name.to_string(), parse_track(track));
    }
    out
}

/// for-each-ref 출력을 로컬 브랜치·리모트 그룹·태그로 나눈다.
/// 리모트 이름에 '/' 가 들어갈 수 있어(team/upstream) 이름 목록과 가장 긴 접두 일치로 그룹을 정한다.
/// 목록에 없는 리모트(방금 지워진 경우 등)는 첫 '/' 앞을 이름으로 본다.
pub fn parse_refs(buf: &[u8], remote_names: &[String]) -> ParsedRefs {
    let mut local: Vec<Branch> = Vec::new();
    let mut tags: Vec<Tag> = Vec::new();
    let mut groups: Vec<(String, Vec<Branch>)> = Vec::new();
    let mut group_index: HashMap<String, usize> = HashMap::new();
    // 긴 이름부터 비교해 'team' 과 'team/upstream' 이 같이 있어도 올바르게 가른다(JS length = UTF-16 단위)
    let mut names: Vec<&String> = remote_names.iter().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.encode_utf16().count()));

    for line in String::from_utf8_lossy(buf).split('\n') {
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 4 {
            continue;
        }
        let at = |i: usize| f.get(i).copied().unwrap_or("");
        let (full_name, hash, peeled, head_mark) = (f[0], f[1], f[2], f[3]);
        let (upstream, track, symref, kind, peeled_kind, created) = (at(4), at(5), at(6), at(7), at(8), at(9));

        if let Some(name) = full_name.strip_prefix("refs/heads/") {
            let mut b = Branch {
                name: name.to_string(),
                full_name: full_name.to_string(),
                hash: hash.to_string(),
                upstream: None,
                ahead: None,
                behind: None,
                upstream_gone: None,
                is_head: head_mark == "*",
            };
            if !upstream.is_empty() {
                b.upstream = Some(upstream.to_string());
                let t = parse_track(track);
                b.upstream_gone = t.upstream_gone;
                b.ahead = t.ahead;
                b.behind = t.behind;
            }
            local.push(b);
        } else if let Some(short) = full_name.strip_prefix("refs/remotes/") {
            // origin/HEAD 같은 심볼릭 ref 는 목록에서 뺀다(이름이 우연히 x/HEAD 인 실제 브랜치는 남긴다)
            if !symref.is_empty() {
                continue;
            }
            let remote = names
                .iter()
                .find(|n| short.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('/')))
                .map(|n| n.to_string())
                .unwrap_or_else(|| short.split('/').next().unwrap_or(short).to_string());
            let idx = *group_index.entry(remote.clone()).or_insert_with(|| {
                groups.push((remote, Vec::new()));
                groups.len() - 1
            });
            groups[idx].1.push(Branch {
                name: short.to_string(),
                full_name: full_name.to_string(),
                hash: hash.to_string(),
                upstream: None,
                ahead: None,
                behind: None,
                upstream_gone: None,
                is_head: false,
            });
        } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
            // annotated 태그는 %(*objectname) 이 가리키는 커밋 해시를 쓴다
            // 트리·블롭을 가리키거나(v2.6.11-tree) 태그를 가리키는 태그는 그래프에서 찾아갈 커밋이 없다
            let targets_commit = if peeled.is_empty() { kind == "commit" } else { peeled_kind == "commit" };
            // %(creatordate): annotated 는 태그를 만든 시각, lightweight 는 가리키는 커밋의 시각
            let date = js_number(created);
            tags.push(Tag {
                name: name.to_string(),
                full_name: full_name.to_string(),
                hash: if peeled.is_empty() { hash } else { peeled }.to_string(),
                annotated: !peeled.is_empty(),
                targets_commit,
                date: (!created.is_empty() && date.0.is_finite()).then_some(date),
            });
        }
    }

    local.sort_by(|a, b| natural_compare(&a.name, &b.name));
    // 태그는 최신순: 만든 시각 내림차순, 시각이 같거나 없으면 이름 역순(v0.10.0 이 v0.9.0 보다 위)
    tags.sort_by(|a, b| {
        let d = b.date.map_or(f64::NEG_INFINITY, |d| d.0) - a.date.map_or(f64::NEG_INFINITY, |d| d.0);
        if d > 0.0 {
            Ordering::Greater
        } else if d < 0.0 {
            Ordering::Less
        } else {
            natural_compare(&b.name, &a.name)
        }
    });
    let mut remotes: Vec<RemoteGroup> = groups
        .into_iter()
        .map(|(name, mut branches)| {
            branches.sort_by(|a, b| natural_compare(&a.name, &b.name));
            RemoteGroup { name, branches }
        })
        .collect();
    remotes.sort_by(|a, b| natural_compare(&a.name, &b.name));
    ParsedRefs { local, remotes, tags }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(t: &Tracking) -> String {
        serde_json::to_string(t).unwrap()
    }

    #[test]
    fn track_forms() {
        assert_eq!(j(&parse_track("ahead 1, behind 2")), r#"{"ahead":1,"behind":2}"#);
        assert_eq!(j(&parse_track("ahead 3")), r#"{"ahead":3}"#);
        assert_eq!(j(&parse_track("behind 1")), r#"{"behind":1}"#);
        assert_eq!(j(&parse_track(" gone ")), r#"{"upstreamGone":true}"#);
        assert_eq!(j(&parse_track("")), "{}");
        assert_eq!(j(&parse_track("ahead x, ahead 7")), r#"{"ahead":7}"#);
    }

    #[test]
    fn tracking_lines() {
        let m = parse_tracking(b"refs/heads/a\0ahead 1, behind 2\nrefs/heads/b\0\nrefs/heads/c\0gone\nrefs/heads/d\0behind 4\n");
        assert_eq!(m.len(), 3);
        assert_eq!(j(&m["refs/heads/c"]), r#"{"upstreamGone":true}"#);
    }

    #[test]
    fn remote_names_strip_cr_and_blank() {
        assert_eq!(parse_remote_names(b"origin\r\n\r\nteam/upstream\nsolo"), ["origin", "team/upstream", "solo"]);
    }

    #[test]
    fn natural_order_basics() {
        let mut v = vec!["v1.10.0", "v1.2.0", "b", "B", "a", "V1.3", "10", "9", "feature/2", "feature-2"];
        v.sort_by(|a, b| natural_compare(a, b));
        assert_eq!(v, ["9", "10", "a", "b", "B", "feature-2", "feature/2", "v1.2.0", "V1.3", "v1.10.0"]);
    }
}
