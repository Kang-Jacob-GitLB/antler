//! Node `path.win32` 의 `normalize`·`join`·`isAbsolute`·`dirname`·`basename` 이식(Node 22.17.1 의 `lib/path.js`).
//!
//! TS 설정 파서(`config.ts`)는 `node:path` 를 쓰고 Antler 는 Windows 에서 돈다 — 그래서 `~/.ssh/config` 의 경로 값은 win32 규칙으로 정규화된다.
//! Rust 는 플랫폼에 상관없이 **항상 win32 규칙**을 쓴다(골든이 플랫폼마다 달라지지 않게). macOS·Linux 지원(Phase 6)에서 posix 규칙이 필요해지면
//! 이 모듈을 건드리지 말고 호출부에서 고른다.
//!
//! 문자열은 UTF-16 코드 단위로 다룬다(JS `length`·`slice` 와 같다 — `normalizeString` 의 `lastSegmentLength` 가 단위 수다).
//! Node 판이 올라가면 보안 수정(예전 CVE-2024-36139·CVE-2025-27210 의 예약 이름 처리)이 들어올 수 있으니, 골든(`tests/golden/ssh/config.json`)이
//! 실패하면 `lib/path.js` 의 `win32` 쪽과 다시 대조한다.

const SEP_FWD: u16 = b'/' as u16;
const SEP_BACK: u16 = b'\\' as u16;
const COLON: u16 = b':' as u16;
const DOT: u16 = b'.' as u16;

/// `COM1`~`COM9`·`LPT1`~`LPT9`·`CON`·`PRN`·`AUX`·`NUL` (Node 의 `WINDOWS_RESERVED_NAMES`)
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn is_sep(c: u16) -> bool {
    c == SEP_FWD || c == SEP_BACK
}

fn is_device_root(c: u16) -> bool {
    (u16::from(b'A')..=u16::from(b'Z')).contains(&c) || (u16::from(b'a')..=u16::from(b'z')).contains(&c)
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn text(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

fn index_of(u: &[u16], c: u16, from: usize) -> Option<usize> {
    (from..u.len()).find(|&i| u[i] == c)
}

/// JS `path.slice(0, colonIndex)` — `colonIndex` 가 -1(콜론 없음)이면 **끝 한 글자를 뺀 앞부분**이다. Node 가 그렇게 부르므로(`isWindowsReservedName(path, -1)`) 그대로 옮긴다
fn is_reserved_name(path: &[u16], colon_index: Option<usize>) -> bool {
    let end = colon_index.unwrap_or_else(|| path.len().saturating_sub(1));
    let device = text(&path[..end]).to_uppercase();
    RESERVED.contains(&device.as_str())
}

/// `.` · `..` 조각을 정리한다(Node 의 `normalizeString`)
fn normalize_string(path: &[u16], allow_above_root: bool) -> Vec<u16> {
    let mut res: Vec<u16> = Vec::new();
    let mut last_segment_length: isize = 0;
    let mut last_slash: isize = -1;
    let mut dots: isize = 0;
    let mut code: u16 = 0;
    let mut i: usize = 0;
    while i <= path.len() {
        if i < path.len() {
            code = path[i];
        } else if is_sep(code) {
            break;
        } else {
            code = SEP_FWD;
        }
        let ii = i as isize;
        if is_sep(code) {
            if last_slash == ii - 1 || dots == 1 {
                // NOOP
            } else if dots == 2 {
                if res.len() < 2 || last_segment_length != 2 || res[res.len() - 1] != DOT || res[res.len() - 2] != DOT {
                    if res.len() > 2 {
                        let last_slash_index = res.len() as isize - last_segment_length - 1;
                        if last_slash_index == -1 {
                            res.clear();
                            last_segment_length = 0;
                        } else {
                            res.truncate(last_slash_index as usize);
                            let prev = res.iter().rposition(|&c| c == SEP_BACK).map_or(-1, |p| p as isize);
                            last_segment_length = res.len() as isize - 1 - prev;
                        }
                        last_slash = ii;
                        dots = 0;
                        i += 1;
                        continue;
                    } else if !res.is_empty() {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = ii;
                        dots = 0;
                        i += 1;
                        continue;
                    }
                }
                if allow_above_root {
                    if !res.is_empty() {
                        res.push(SEP_BACK);
                    }
                    res.extend([DOT, DOT]);
                    last_segment_length = 2;
                }
            } else {
                let seg = &path[(last_slash + 1) as usize..i];
                if !res.is_empty() {
                    res.push(SEP_BACK);
                }
                res.extend_from_slice(seg);
                last_segment_length = ii - last_slash - 1;
            }
            last_slash = ii;
            dots = 0;
        } else if code == DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
        i += 1;
    }
    res
}

/// `path.win32.normalize`
pub fn normalize(path: &str) -> String {
    let p = units(path);
    let len = p.len();
    if len == 0 {
        return ".".to_string();
    }
    let mut root_end = 0usize;
    let mut device: Option<Vec<u16>> = None;
    let mut is_absolute = false;
    let code = p[0];
    if len == 1 {
        return if code == SEP_FWD { "\\".to_string() } else { path.to_string() };
    }
    if is_sep(code) {
        is_absolute = true;
        if is_sep(p[1]) {
            let mut j = 2usize;
            let mut last = j;
            while j < len && !is_sep(p[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part = p[last..j].to_vec();
                last = j;
                while j < len && is_sep(p[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_sep(p[j]) {
                        j += 1;
                    }
                    if j == len {
                        // UNC 루트뿐이다
                        return format!("\\\\{}\\{}\\", text(&first_part), text(&p[last..]));
                    }
                    if j != last {
                        device = Some(units(&format!("\\\\{}\\{}", text(&first_part), text(&p[last..j]))));
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if let Some(colon) = index_of(&p, COLON, 0).filter(|&c| c > 0) {
        if is_device_root(code) && colon == 1 {
            device = Some(p[..2].to_vec());
            root_end = 2;
            if len > 2 && is_sep(p[2]) {
                is_absolute = true;
                root_end = 3;
            }
        } else if is_reserved_name(&p, Some(colon)) {
            device = Some(p[..=colon].to_vec());
            root_end = colon + 1;
        }
    }

    let mut tail: Vec<u16> = if root_end < len { normalize_string(&p[root_end..], !is_absolute) } else { Vec::new() };
    if tail.is_empty() && !is_absolute {
        tail = vec![DOT];
    }
    if !tail.is_empty() && is_sep(p[len - 1]) {
        tail.push(SEP_BACK);
    }
    if !is_absolute && device.is_none() && p.contains(&COLON) {
        // 상대 경로가 드라이브 절대 경로로 보이게 되면 `.\` 를 붙인다(CVE-2024-36139)
        if tail.len() >= 2 && is_device_root(tail[0]) && tail[1] == COLON {
            return format!(".\\{}", text(&tail));
        }
        let mut index = index_of(&p, COLON, 0);
        while let Some(ix) = index {
            if ix == len - 1 || is_sep(p[ix + 1]) {
                return format!(".\\{}", text(&tail));
            }
            index = index_of(&p, COLON, ix + 1);
        }
    }
    let colon_index = index_of(&p, COLON, 0);
    if is_reserved_name(&p, colon_index) {
        return format!(".\\{}{}", device.as_deref().map(text).unwrap_or_default(), text(&tail));
    }
    match device {
        None => {
            if is_absolute {
                format!("\\{}", text(&tail))
            } else {
                text(&tail)
            }
        }
        Some(d) => {
            if is_absolute {
                format!("{}\\{}", text(&d), text(&tail))
            } else {
                format!("{}{}", text(&d), text(&tail))
            }
        }
    }
}

/// `path.win32.join`
pub fn join(parts: &[&str]) -> String {
    let mut joined: Option<Vec<u16>> = None;
    let mut first_part: Vec<u16> = Vec::new();
    for arg in parts {
        let a = units(arg);
        if a.is_empty() {
            continue;
        }
        match joined.as_mut() {
            None => {
                first_part = a.clone();
                joined = Some(a);
            }
            Some(j) => {
                j.push(SEP_BACK);
                j.extend(a);
            }
        }
    }
    let Some(mut joined) = joined else {
        return ".".to_string();
    };
    // 이어 붙인 결과가 슬래시 둘로 시작하면 normalize 가 UNC 로 오해한다 — 첫 조각이 분명히 UNC 가 아니면 하나로 줄인다
    let mut needs_replace = true;
    let mut slash_count = 0usize;
    if is_sep(first_part[0]) {
        slash_count += 1;
        let first_len = first_part.len();
        if first_len > 1 && is_sep(first_part[1]) {
            slash_count += 1;
            if first_len > 2 {
                if is_sep(first_part[2]) {
                    slash_count += 1;
                } else {
                    needs_replace = false;
                }
            }
        }
    }
    if needs_replace {
        while slash_count < joined.len() && is_sep(joined[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            let mut v = vec![SEP_BACK];
            v.extend_from_slice(&joined[slash_count..]);
            joined = v;
        }
    }
    normalize(&text(&joined))
}

/// `path.win32.isAbsolute`
pub fn is_absolute(path: &str) -> bool {
    let p = units(path);
    if p.is_empty() {
        return false;
    }
    is_sep(p[0]) || (p.len() > 2 && is_device_root(p[0]) && p[1] == COLON && is_sep(p[2]))
}

/// `path.win32.dirname`
pub fn dirname(path: &str) -> String {
    let p = units(path);
    let len = p.len();
    if len == 0 {
        return ".".to_string();
    }
    let code = p[0];
    if len == 1 {
        return if is_sep(code) { path.to_string() } else { ".".to_string() };
    }
    let mut root_end: isize = -1;
    let mut offset = 0usize;
    if is_sep(code) {
        root_end = 1;
        offset = 1;
        if is_sep(p[1]) {
            let mut j = 2usize;
            let mut last = j;
            while j < len && !is_sep(p[j]) {
                j += 1;
            }
            if j < len && j != last {
                last = j;
                while j < len && is_sep(p[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_sep(p[j]) {
                        j += 1;
                    }
                    if j == len {
                        return path.to_string();
                    }
                    if j != last {
                        root_end = j as isize + 1;
                        offset = j + 1;
                    }
                }
            }
        }
    } else if is_device_root(code) && p[1] == COLON {
        root_end = if len > 2 && is_sep(p[2]) { 3 } else { 2 };
        offset = root_end as usize;
    }
    let mut end: isize = -1;
    let mut matched_slash = true;
    let mut i = len as isize - 1;
    while i >= offset as isize {
        if is_sep(p[i as usize]) {
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
        if root_end == -1 {
            return ".".to_string();
        }
        end = root_end;
    }
    text(&p[..end as usize])
}

/// `path.win32.basename`(확장자 인자 없음)
pub fn basename(path: &str) -> String {
    let p = units(path);
    let mut start = 0usize;
    let mut end: isize = -1;
    let mut matched_slash = true;
    if p.len() >= 2 && is_device_root(p[0]) && p[1] == COLON {
        start = 2;
    }
    let mut i = p.len() as isize - 1;
    while i >= start as isize {
        if is_sep(p[i as usize]) {
            if !matched_slash {
                start = i as usize + 1;
                break;
            }
        } else if end == -1 {
            matched_slash = false;
            end = i + 1;
        }
        i -= 1;
    }
    if end == -1 {
        return String::new();
    }
    text(&p[start..end as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_는_node_win32_와_같다() {
        // Node 22.17.1 `path.win32.normalize` 로 얻은 값
        assert_eq!(normalize(""), ".");
        assert_eq!(normalize("/"), "\\");
        assert_eq!(normalize("a/b/../c"), "a\\c");
        assert_eq!(normalize("C:\\a\\..\\..\\b"), "C:\\b");
        assert_eq!(normalize("C:a"), "C:a");
        assert_eq!(normalize("//server/share/x/"), "\\\\server\\share\\x\\");
        assert_eq!(normalize("//server/share"), "\\\\server\\share\\");
        assert_eq!(normalize("a:b"), "a:b");
        assert_eq!(normalize("CON:x"), ".\\CON:x");
    }

    #[test]
    fn join_dirname_basename_isabsolute() {
        assert_eq!(join(&["C:\\h", ".ssh", "a"]), "C:\\h\\.ssh\\a");
        assert_eq!(join(&["", ""]), ".");
        assert_eq!(join(&["//server", "share"]), "\\\\server\\share\\");
        assert_eq!(dirname("C:\\a\\b.conf"), "C:\\a");
        assert_eq!(dirname("C:\\a"), "C:\\");
        assert_eq!(dirname("a"), ".");
        assert_eq!(basename("C:\\a\\b*.conf"), "b*.conf");
        assert_eq!(basename("C:\\a\\"), "a");
        assert!(is_absolute("C:\\a") && is_absolute("\\a") && !is_absolute("C:a") && !is_absolute(""));
    }
}
