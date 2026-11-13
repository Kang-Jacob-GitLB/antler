//! `src/main/git/parsers/submodule.ts`

use std::collections::{HashMap, HashSet};

use serde::Serialize;

/// 인덱스의 gitlink(모드 160000) 조회. 커밋 전 상태(`git add` 직후)도 반영하려고 ls-tree 가 아니라 인덱스를 본다.
pub const GITLINK_ARGS: [&str; 3] = ["ls-files", "-s", "-z"];
/// `.gitmodules` 를 설정 파일로 읽는다. 파일을 설정으로 읽을 뿐이라 명령을 실행하지 않는다.
/// `git submodule` 은 하위 저장소 설정까지 건드리므로 쓰지 않는다.
pub const GITMODULES_ARGS: [&str; 6] = ["config", "-z", "--file", ".gitmodules", "--get-regexp", "^submodule\\."];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gitlink {
    pub path: String,
    /// 상위 저장소가 기록한 커밋
    pub sha: String,
}

/// TS 는 경로 → 모듈 정보의 `Map` 이다. Rust 는 삽입 순서를 지킨 목록으로 돌려주고 경로를 항목에 담는다.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleEntry {
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// `ls-files -s -z` 출력에서 gitlink 만 뽑는다. 레코드: `160000 <sha> <stage>\t<path>`. 충돌(stage>0)은 경로당 하나만 남긴다.
pub fn parse_gitlinks(buf: &[u8]) -> Vec<Gitlink> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for rec in String::from_utf8_lossy(buf).split('\0') {
        if !rec.starts_with("160000 ") {
            continue;
        }
        let Some(tab) = rec.find('\t') else { continue };
        let sha = rec[..tab].split(' ').nth(1).unwrap_or("");
        let path = &rec[tab + 1..];
        if sha.is_empty() || path.is_empty() || seen.contains(path) {
            continue;
        }
        seen.insert(path.to_string());
        out.push(Gitlink { path: path.to_string(), sha: sha.to_string() });
    }
    out
}

#[derive(Default)]
struct Partial {
    path: Option<String>,
    url: Option<String>,
    branch: Option<String>,
}

/// `config -z --get-regexp` 출력(`key\nvalue\0`)에서 경로 → 모듈 정보를 만든다.
/// 키는 `submodule.<name>.<변수>` — 이름에 '.' 이 들어갈 수 있으므로 마지막 '.' 로 자른다. URL 은 표시용 텍스트일 뿐이다.
pub fn parse_gitmodules(buf: &[u8]) -> Vec<ModuleEntry> {
    const PREFIX: &str = "submodule.";
    // 이름의 처음 나온 순서를 지킨다(JS Map 의 삽입 순서)
    let mut names: Vec<(String, Partial)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for rec in String::from_utf8_lossy(buf).split('\0') {
        if rec.is_empty() {
            continue;
        }
        let (key, value) = rec.split_once('\n').unwrap_or((rec, ""));
        if !key.starts_with(PREFIX) {
            continue;
        }
        let Some(dot) = key.rfind('.') else { continue };
        if dot <= PREFIX.len() {
            continue;
        }
        let name = &key[PREFIX.len()..dot];
        let variable = key[dot + 1..].to_lowercase();
        let i = *index.entry(name.to_string()).or_insert_with(|| {
            names.push((name.to_string(), Partial::default()));
            names.len() - 1
        });
        let cur = &mut names[i].1;
        match variable.as_str() {
            "path" => cur.path = Some(value.to_string()),
            "url" => cur.url = Some(value.to_string()),
            "branch" => cur.branch = Some(value.to_string()),
            _ => {}
        }
    }
    // 같은 경로를 쓰는 이름이 둘이면 뒤의 것이 덮어쓴다(자리는 처음 그대로)
    let mut out: Vec<ModuleEntry> = Vec::new();
    let mut by_path: HashMap<String, usize> = HashMap::new();
    for (name, v) in names {
        let Some(path) = v.path.filter(|p| !p.is_empty()) else { continue };
        let entry = ModuleEntry { path: path.clone(), name, url: v.url.filter(|u| !u.is_empty()), branch: v.branch.filter(|b| !b.is_empty()) };
        match by_path.get(&path) {
            Some(&i) => out[i] = entry,
            None => {
                by_path.insert(path, out.len());
                out.push(entry);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// `rev-parse --absolute-git-dir HEAD` 출력(줄 단위)에서 git-dir 과 HEAD 해시를 읽는다. HEAD 가 없으면 해시는 없다.
pub fn parse_probe(stdout: &str) -> Probe {
    let lines: Vec<&str> = stdout.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let git_dir = lines[0];
    let second = lines.get(1).copied().unwrap_or("");
    // 정규식 `^[0-9a-f]{40}([0-9a-f]{24})?$` (대소문자 무시)
    let is_hash = matches!(second.len(), 40 | 64) && second.bytes().all(|b| b.is_ascii_hexdigit());
    Probe { git_dir: (!git_dir.is_empty()).then(|| git_dir.to_string()), head: is_hash.then(|| second.to_ascii_lowercase()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmoduleStatus {
    Clean,
    Modified,
    Unknown,
}

pub fn submodule_status(recorded: &str, head: Option<&str>) -> SubmoduleStatus {
    match head {
        None | Some("") => SubmoduleStatus::Unknown,
        Some(h) if h == recorded => SubmoduleStatus::Clean,
        Some(_) => SubmoduleStatus::Modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_check() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        assert_eq!(submodule_status(&a, Some(&a)), SubmoduleStatus::Clean);
        assert_eq!(submodule_status(&a, Some(&b)), SubmoduleStatus::Modified);
        assert_eq!(submodule_status(&a, None), SubmoduleStatus::Unknown);
    }

    #[test]
    fn probe_without_head() {
        assert_eq!(parse_probe("/r/.git\nHEAD\n"), Probe { git_dir: Some("/r/.git".into()), head: None });
        assert_eq!(parse_probe(""), Probe::default());
    }
}
