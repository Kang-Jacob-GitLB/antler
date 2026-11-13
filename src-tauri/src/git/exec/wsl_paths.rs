//! WSL UNC 경로(`\\wsl.localhost\<distro>\...` / `\\wsl$\<distro>\...`)와 리눅스 경로 사이의 변환
//! (`src/main/executors/wslPaths.ts`). TS 는 정규식 `^[\\/]{2}(?:wsl\.localhost|wsl\$)(?:[\\/]+([^\\/]+))?(.*)$`(i 플래그)
//! 를 쓴다 — 여기서는 같은 규칙을 손으로 푼다(Rust regex 의 `\s`·`.` 와 JS 의 것이 달라서 정규식을 옮기지 않는다).

use crate::git::parsers::js_trim;

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// JS `.`(줄 종결자를 뺀 모든 문자)에 걸리지 않는 문자
fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// 맨 앞 `prefix`(ASCII)를 대소문자 무시로 떼어 낸다. JS `i`(u 없음)는 ASCII 만 접는다
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// `\\wsl.localhost\X\...` 또는 `\\wsl$\X\...` 형태인지(슬래시 방향 무관)
pub fn is_wsl_unc(p: &str) -> bool {
    wsl_from_unc(p).is_some()
}

/// WSL UNC 경로를 (배포판, 리눅스 경로)로 분해한다. 배포판 이름이 없으면 None. 루트는 `/`.
pub fn wsl_from_unc(p: &str) -> Option<(String, String)> {
    let s = js_trim(p);
    let mut chars = s.chars();
    if !is_sep(chars.next()?) || !is_sep(chars.next()?) {
        return None;
    }
    let rest = &s[2..];
    let rest = strip_prefix_ci(rest, "wsl.localhost").or_else(|| strip_prefix_ci(rest, "wsl$"))?;
    // (?:[\\/]+([^\\/]+))? — 구분자 하나 이상 뒤에 구분자가 아닌 문자가 하나 이상 있어야 배포판 이름이 잡힌다
    let after_seps = rest.trim_start_matches(is_sep);
    if after_seps.len() == rest.len() {
        return None;
    }
    let distro_end = after_seps.find(is_sep).unwrap_or(after_seps.len());
    let distro = &after_seps[..distro_end];
    if distro.is_empty() {
        return None;
    }
    let tail = &after_seps[distro_end..];
    // (.*)$ — `.` 는 줄 종결자와 맞지 않으므로 나머지에 있으면 정규식 전체가 실패한다
    if tail.chars().any(is_js_line_terminator) {
        return None;
    }
    let segments: Vec<&str> = tail.split(is_sep).filter(|s| !s.is_empty()).collect();
    Some((distro.to_string(), format!("/{}", segments.join("/"))))
}

/// 리눅스 경로를 `\\wsl.localhost\<distro>\...` 로 만든다.
pub fn unc_from_wsl(distro: &str, linux_path: &str) -> String {
    let mut parts = vec!["\\\\wsl.localhost"];
    parts.push(distro);
    parts.extend(linux_path.split('/').filter(|s| !s.is_empty()));
    parts.join("\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(p: &str) -> Option<(String, String)> {
        wsl_from_unc(p)
    }

    #[test]
    fn 슬래시_방향과_호스트_표기를_가리지_않는다() {
        let want = Some(("Ubuntu".to_string(), "/home/me/repo".to_string()));
        assert_eq!(split(r"\\wsl.localhost\Ubuntu\home\me\repo"), want);
        assert_eq!(split(r"\\wsl$\Ubuntu\home\me\repo"), want);
        assert_eq!(split("//wsl.localhost/Ubuntu/home/me/repo"), want);
        assert_eq!(split(r"\\WSL.LOCALHOST\Ubuntu\home\me\repo/"), want);
        assert_eq!(split(r"  \\wsl$\Ubuntu\home\me\repo  "), want);
    }

    #[test]
    fn 루트와_중복_구분자() {
        assert_eq!(split(r"\\wsl.localhost\Ubuntu"), Some(("Ubuntu".into(), "/".into())));
        assert_eq!(split(r"\\wsl.localhost\Ubuntu\"), Some(("Ubuntu".into(), "/".into())));
        assert_eq!(split(r"\\wsl.localhost\\\Ubuntu\\a\\\b"), Some(("Ubuntu".into(), "/a/b".into())));
    }

    #[test]
    fn 배포판이_없거나_wsl_경로가_아니면_none() {
        assert_eq!(split(r"\\wsl.localhost"), None);
        assert_eq!(split(r"\\wsl.localhost\"), None);
        assert_eq!(split(r"\\wsl.localhostX\Ubuntu\a"), None);
        assert_eq!(split(r"\\server\share\a"), None);
        assert_eq!(split(r"C:\repo"), None);
        assert_eq!(split("/home/me"), None);
        assert_eq!(split(""), None);
        assert!(!is_wsl_unc(r"\wsl.localhost\Ubuntu"));
        assert!(is_wsl_unc(r"\\wsl$\Ubuntu"));
    }

    #[test]
    fn 나머지에_줄바꿈이_있으면_정규식처럼_실패한다() {
        assert_eq!(split("\\\\wsl.localhost\\Ubuntu\\a\nb"), None);
        assert_eq!(split("\\\\wsl.localhost\\Ubuntu\\a\u{2028}b"), None);
        // 배포판 이름 안의 줄바꿈은 `[^\\/]` 가 받는다
        assert_eq!(split("\\\\wsl.localhost\\Ub\nuntu\\a"), Some(("Ub\nuntu".into(), "/a".into())));
    }

    #[test]
    fn 리눅스_경로에서_unc_로() {
        assert_eq!(unc_from_wsl("Ubuntu", "/home/me/repo"), r"\\wsl.localhost\Ubuntu\home\me\repo");
        assert_eq!(unc_from_wsl("Ubuntu", "/"), r"\\wsl.localhost\Ubuntu");
        assert_eq!(unc_from_wsl("Ubuntu", "//a///b/"), r"\\wsl.localhost\Ubuntu\a\b");
    }
}
