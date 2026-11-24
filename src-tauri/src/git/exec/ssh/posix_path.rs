//! Node `path.posix` 의 작은 이식(macOS·Linux 에서 설정 값의 경로 정규화). Windows 규칙은 [`super::win32_path`] 다.
//!
//! 홈이 `/` 로 시작하면 이 규칙을 쓴다(`config.rs` 의 `PathStyle`). 그렇지 않으면 골든 사양인 win32 규칙이다.

/// `path.posix.normalize`
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if out.is_empty() && !absolute {
        out.push('.');
    }
    if !out.is_empty() && trailing {
        out.push('/');
    }
    if absolute {
        format!("/{out}")
    } else {
        out
    }
}

/// `path.posix.join`
pub fn join(parts: &[&str]) -> String {
    let joined: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if joined.is_empty() {
        return ".".to_string();
    }
    normalize(&joined.join("/"))
}

/// `path.posix.isAbsolute`
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// `path.posix.dirname`
pub fn dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        None => ".".to_string(),
        Some(i) => {
            let dir = trimmed[..i].trim_end_matches('/');
            if dir.is_empty() {
                "/".to_string()
            } else {
                dir.to_string()
            }
        }
    }
}

/// `path.posix.basename`(확장자 인자 없음)
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        None => trimmed.to_string(),
        Some(i) => trimmed[i + 1..].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_는_점과_겹친_슬래시를_정리한다() {
        assert_eq!(normalize("/Users/me/.ssh/id_rsa"), "/Users/me/.ssh/id_rsa");
        assert_eq!(normalize("/Users//me/./.ssh/../.ssh/id_rsa"), "/Users/me/.ssh/id_rsa");
        assert_eq!(normalize("/../a"), "/a");
        assert_eq!(normalize("../a/./b/"), "../a/b/");
        assert_eq!(normalize("a/.."), ".");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize(""), ".");
    }

    #[test]
    fn join_과_경로_조각() {
        assert_eq!(join(&["/Users/me", ".ssh", "conf.d"]), "/Users/me/.ssh/conf.d");
        assert_eq!(join(&["/Users/me/", "", "x"]), "/Users/me/x");
        assert_eq!(join(&[]), ".");
        assert!(is_absolute("/a"));
        assert!(!is_absolute("a/b"));
        assert_eq!(dirname("/Users/me/.ssh/conf.d/*.conf"), "/Users/me/.ssh/conf.d");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("a"), ".");
        assert_eq!(basename("/Users/me/.ssh/conf.d/*.conf"), "*.conf");
        assert_eq!(basename("/a/b/"), "b");
    }
}
