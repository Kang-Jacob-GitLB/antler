//! 외부 열기가 쓰는 Node `path` 의 posix·win32 판(`node:path`). plan.ts 는 플랫폼을 **인자로** 받아 `win32`·`posix` 를 고르므로(테스트가 리눅스·맥 사례를
//! Windows 에서 돌린다) Rust 도 cfg 가 아니라 [`PathMod`] 값으로 고른다.
//!
//! win32 의 `normalize`·`join`·`isAbsolute`·`dirname`·`basename` 은 `git::exec::ssh::win32_path` 를 그대로 쓴다(그 파일은 건드리지 않는다).
//! 여기서 더하는 것은 그 파일에 없는 `win32.relative`(Node `lib/path.js` 의 이식)와 posix 판 전부다.
//!
//! **절대 경로만 다룬다.** Node 의 `relative`·`resolve` 는 상대 경로·드라이브 없는 `\x` 에 cwd 를 쓰지만, 여기서는 그런 입력을 `None` 으로 돌려
//! 호출자가 "저장소 밖"으로 취급하게 한다(cwd 에 기대면 저장소가 정한 경로로 판정이 흔들린다). 입력은 모두 `absoluteTarget`·`realpath` 가 만든 절대 경로다.
//! 알려진 차이: win32 비교의 소문자 변환은 글자 하나씩(`char::to_lowercase` 의 첫 글자)이다. JS `toLowerCase` 는 `İ` 처럼 길이가 바뀌는 글자가 있으나,
//! 길이가 바뀌면 Node 자신이 원문과 인덱스가 어긋나는 코드라 따라 하지 않는다.

use crate::git::exec::ssh::win32_path as w32;

/// 경로 규칙. plan.ts 의 `pathMod(platform)`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMod {
    Win32,
    Posix,
}

impl PathMod {
    pub fn sep(self) -> char {
        match self {
            PathMod::Win32 => '\\',
            PathMod::Posix => '/',
        }
    }

    pub fn is_absolute(self, p: &str) -> bool {
        match self {
            PathMod::Win32 => w32::is_absolute(p),
            PathMod::Posix => p.starts_with('/'),
        }
    }

    pub fn normalize(self, p: &str) -> String {
        match self {
            PathMod::Win32 => w32::normalize(p),
            PathMod::Posix => posix_normalize(p),
        }
    }

    pub fn join(self, parts: &[&str]) -> String {
        match self {
            PathMod::Win32 => w32::join(parts),
            PathMod::Posix => posix_join(parts),
        }
    }

    pub fn dirname(self, p: &str) -> String {
        match self {
            PathMod::Win32 => w32::dirname(p),
            PathMod::Posix => posix_dirname(p),
        }
    }

    /// `path.relative(from, to)`. 둘 다 완전한 절대 경로가 아니면 None
    pub fn relative(self, from: &str, to: &str) -> Option<String> {
        match self {
            PathMod::Win32 => win32_relative(from, to),
            PathMod::Posix => posix_relative(from, to),
        }
    }
}

// ── posix ─────────────────────────────────────────────────────────────────────

/// `path.posix.normalize`
pub fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let is_abs = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !is_abs {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let mut res = out.join("/");
    if res.is_empty() {
        if is_abs {
            return "/".to_string();
        }
        return if trailing { "./".to_string() } else { ".".to_string() };
    }
    if trailing {
        res.push('/');
    }
    if is_abs {
        format!("/{res}")
    } else {
        res
    }
}

/// `path.posix.join`
pub fn posix_join(parts: &[&str]) -> String {
    let joined = parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/");
    if joined.is_empty() {
        return ".".to_string();
    }
    posix_normalize(&joined)
}

/// `path.posix.dirname`
pub fn posix_dirname(path: &str) -> String {
    let b = path.as_bytes();
    if b.is_empty() {
        return ".".to_string();
    }
    let has_root = b[0] == b'/';
    let mut end: isize = -1;
    let mut matched_slash = true;
    let mut i = b.len() as isize - 1;
    while i >= 1 {
        if b[i as usize] == b'/' {
            if !matched_slash {
                end = i;
                break;
            }
        } else {
            matched_slash = false;
        }
        i -= 1;
    }
    if end == -1 {
        return if has_root { "/".to_string() } else { ".".to_string() };
    }
    if has_root && end == 1 {
        return "//".to_string();
    }
    path[..end as usize].to_string()
}

/// `path.posix.basename`(확장자 인자 없음)
pub fn posix_basename(path: &str) -> String {
    let t = path.trim_end_matches('/');
    t.rsplit('/').next().unwrap_or("").to_string()
}

/// `path.posix.resolve` 의 절대 경로 입력판: 정규화하고 끝의 `/` 를 뗀다
fn posix_resolve_abs(p: &str) -> Option<String> {
    if !p.starts_with('/') {
        return None;
    }
    let n = posix_normalize(p);
    Some(if n.len() > 1 { n.trim_end_matches('/').to_string() } else { n })
}

/// `path.posix.relative`(절대 경로만). 구성 요소 단위로 공통 앞부분을 구한다 — Node 의 문자 단위 구현과 결과가 같다
pub fn posix_relative(from: &str, to: &str) -> Option<String> {
    let f = posix_resolve_abs(from)?;
    let t = posix_resolve_abs(to)?;
    if f == t {
        return Some(String::new());
    }
    let fs: Vec<&str> = f.split('/').filter(|s| !s.is_empty()).collect();
    let ts: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    let common = fs.iter().zip(ts.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; fs.len() - common];
    parts.extend(&ts[common..]);
    Some(parts.join("/"))
}

// ── win32 ─────────────────────────────────────────────────────────────────────

fn is_bs(c: char) -> bool {
    c == '\\'
}

/// 드라이브(`C:\`·`C:/`)나 UNC(`\\`·`//` 로 시작)로 뿌리가 정해진 경로인가. 드라이브 없는 `\x`·`C:x` 는 cwd 가 뿌리를 정하므로 아니다
fn has_full_root(p: &str) -> bool {
    let c: Vec<char> = p.chars().take(3).collect();
    let sep = |x: char| x == '\\' || x == '/';
    (c.len() >= 2 && sep(c[0]) && sep(c[1])) || (c.len() == 3 && c[0].is_ascii_alphabetic() && c[1] == ':' && sep(c[2]))
}

/// 뿌리(`C:\`·`\\server\share\`)의 글자 수. 뿌리 뒤의 끝 `\` 만 뗄 수 있다
fn root_len(n: &[char]) -> usize {
    if n.len() >= 2 && is_bs(n[0]) && is_bs(n[1]) {
        // \\server\share\
        let mut i = 2;
        while i < n.len() && !is_bs(n[i]) {
            i += 1;
        }
        i += 1;
        while i < n.len() && !is_bs(n[i]) {
            i += 1;
        }
        return (i + 1).min(n.len());
    }
    if n.len() >= 3 && n[1] == ':' {
        return 3;
    }
    1
}

/// `path.win32.resolve` 의 절대 경로 입력판
fn win32_resolve_abs(p: &str) -> Option<String> {
    if !has_full_root(p) {
        return None;
    }
    let n: Vec<char> = w32::normalize(p).chars().collect();
    let root = root_len(&n);
    let mut end = n.len();
    while end > root && is_bs(n[end - 1]) {
        end -= 1;
    }
    Some(n[..end].iter().collect())
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// `path.win32.relative`(Node 22 의 `lib/path.js` 이식, 절대 경로만)
pub fn win32_relative(from_raw: &str, to_raw: &str) -> Option<String> {
    let from_orig = win32_resolve_abs(from_raw)?;
    let to_orig = win32_resolve_abs(to_raw)?;
    if from_orig == to_orig {
        return Some(String::new());
    }
    let from: Vec<char> = from_orig.chars().map(lower).collect();
    let to: Vec<char> = to_orig.chars().map(lower).collect();
    if from == to {
        return Some(String::new());
    }
    let to_o: Vec<char> = to_orig.chars().collect();
    let mut from_start = 0usize;
    while from_start < from.len() && is_bs(from[from_start]) {
        from_start += 1;
    }
    let mut from_end = from.len();
    while from_end > from_start + 1 && is_bs(from[from_end - 1]) {
        from_end -= 1;
    }
    let from_len = from_end - from_start;
    let mut to_start = 0usize;
    while to_start < to.len() && is_bs(to[to_start]) {
        to_start += 1;
    }
    let mut to_end = to.len();
    while to_end > to_start + 1 && is_bs(to[to_end - 1]) {
        to_end -= 1;
    }
    let to_len = to_end - to_start;
    let length = from_len.min(to_len);
    let mut last_common_sep: isize = -1;
    let mut i = 0usize;
    while i < length {
        let fc = from[from_start + i];
        if fc != to[to_start + i] {
            break;
        }
        if is_bs(fc) {
            last_common_sep = i as isize;
        }
        i += 1;
    }
    let slice = |a: usize, b: usize| -> String { to_o[a.min(to_o.len())..b.min(to_o.len())].iter().collect() };
    if i != length {
        if last_common_sep == -1 {
            return Some(to_orig);
        }
    } else {
        if to_len > length {
            if is_bs(to[to_start + i]) {
                // from 이 to 의 정확한 기준 경로
                return Some(slice(to_start + i + 1, to_o.len()));
            }
            if i == 2 {
                // from 이 드라이브 뿌리(C:\ → C:\foo)
                return Some(slice(to_start + i, to_o.len()));
            }
        }
        if from_len > length {
            if is_bs(from[from_start + i]) {
                last_common_sep = i as isize;
            } else if i == 2 {
                last_common_sep = 3;
            }
        }
        if last_common_sep == -1 {
            last_common_sep = 0;
        }
    }
    let mut out = String::new();
    let mut k = from_start as isize + last_common_sep + 1;
    while k <= from_end as isize {
        let ku = k as usize;
        if ku == from_end || is_bs(from[ku]) {
            out.push_str(if out.is_empty() { ".." } else { "\\.." });
        }
        k += 1;
    }
    let ts = (to_start as isize + last_common_sep) as usize;
    if !out.is_empty() {
        return Some(format!("{out}{}", slice(ts, to_end)));
    }
    let mut ts = ts;
    if to_o.get(ts).is_some_and(|&c| is_bs(c)) {
        ts += 1;
    }
    Some(slice(ts, to_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_함수들은_node_와_같다() {
        assert_eq!(posix_normalize("/a//b/./c/../d/"), "/a/b/d/");
        assert_eq!(posix_normalize("a/../.."), "..");
        assert_eq!(posix_normalize("/.."), "/");
        assert_eq!(posix_join(&["/home/u/repo", "a", "b"]), "/home/u/repo/a/b");
        assert_eq!(posix_dirname("/a/b/c"), "/a/b");
        assert_eq!(posix_dirname("/a"), "/");
        assert_eq!(posix_dirname("/"), "/");
        assert_eq!(posix_dirname("a"), ".");
        assert_eq!(posix_basename("/a/b/"), "b");
        assert_eq!(posix_relative("/r", "/r/a/b").as_deref(), Some("a/b"));
        assert_eq!(posix_relative("/r/a", "/r/b").as_deref(), Some("../b"));
        assert_eq!(posix_relative("/r", "/r").as_deref(), Some(""));
        assert_eq!(posix_relative("/r", "/r2/x").as_deref(), Some("../r2/x"));
        assert_eq!(posix_relative("r", "/x"), None);
    }

    #[test]
    fn win32_relative_은_node_와_같다() {
        // Node 22 `path.win32.relative` 로 얻은 값(골든 tests/golden/external/plan.json 이 더 많은 사례를 본다)
        assert_eq!(win32_relative("C:\\Repo", "C:\\Repo\\a").as_deref(), Some("a"));
        assert_eq!(win32_relative("C:\\Repo", "c:\\repo\\a").as_deref(), Some("a"));
        assert_eq!(win32_relative("C:\\Repo", "D:\\x").as_deref(), Some("D:\\x"));
        assert_eq!(win32_relative("C:\\a\\b", "C:\\a\\c").as_deref(), Some("..\\c"));
        assert_eq!(win32_relative("C:\\", "C:\\foo").as_deref(), Some("foo"));
        assert_eq!(win32_relative("C:\\a", "C:\\").as_deref(), Some(".."));
        assert_eq!(win32_relative("C:\\a", "C:\\a").as_deref(), Some(""));
        assert_eq!(win32_relative("\\\\srv\\sh\\a", "\\\\srv\\sh\\a\\b").as_deref(), Some("b"));
        assert_eq!(win32_relative("C:\\a\\b\\c", "C:\\a\\x").as_deref(), Some("..\\..\\x"));
        assert_eq!(win32_relative("\\\\srv\\sh", "\\\\srv\\sh\\x").as_deref(), Some("x"));
        assert_eq!(win32_relative("\\\\srv\\sh\\a", "\\\\srv\\o\\a").as_deref(), Some("..\\..\\o\\a"));
        assert_eq!(win32_relative("C:\\ab", "C:\\abc\\d").as_deref(), Some("..\\abc\\d"));
        assert_eq!(win32_relative("C:\\a\\", "C:\\a\\b\\").as_deref(), Some("b"));
        assert_eq!(posix_relative("/", "/x").as_deref(), Some("x"));
        assert_eq!(posix_relative("/a/b", "/").as_deref(), Some("../.."));
        assert_eq!(win32_relative("repo", "C:\\x"), None);
        assert_eq!(win32_relative("\\x", "C:\\x"), None);
    }
}
