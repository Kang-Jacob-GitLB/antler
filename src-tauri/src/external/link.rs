//! `code:openLink` 허용 대상(`src/main/external/link.ts`): https 주소이고 자격 증명(username/password)이 없는 것만. 정규화된 href 를 돌려준다.
//! URL 해석은 `net::url_rules::parse_url` 이다 — TS(`new URL`)보다 IDN 이 엄격하다(그 함수의 문서가 알려진 차이를 적는다).

use crate::net::url_rules::{has_credentials, parse_url};

/// 주소 문자열의 길이 상한(JS `length` 와 같은 UTF-16 단위)
const MAX_LINK_LEN: usize = 4096;

pub fn allowed_link_url(raw: &str) -> Option<String> {
    if raw.encode_utf16().count() > MAX_LINK_LEN {
        return None;
    }
    let u = parse_url(raw)?;
    if u.scheme() != "https" || has_credentials(&u) {
        return None;
    }
    Some(u.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 자격_증명_없는_https_만_허용한다() {
        assert_eq!(allowed_link_url("https://example.com/a?b=1#c").as_deref(), Some("https://example.com/a?b=1#c"));
        assert_eq!(allowed_link_url("https://example.com").as_deref(), Some("https://example.com/"));
        for bad in ["http://example.com", "file:///etc/passwd", "javascript:alert(1)", "https://user@example.com", "https://user:pw@example.com", "not a url", ""] {
            assert_eq!(allowed_link_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn 길이는_utf16_단위로_센다() {
        let pad = |n: usize| format!("https://e.com/{}", "a".repeat(n));
        assert!(allowed_link_url(&pad(4096 - 14)).is_some());
        assert!(allowed_link_url(&pad(4096 - 13)).is_none());
        // 이모지는 2단위: 글자 수(2056)는 4096 보다 한참 적어도 단위로는 넘친다(14 + 2*2041 = 4096 까지는 통과)
        assert!(allowed_link_url(&format!("https://e.com/{}", "😀".repeat(2041))).is_some());
        assert!(allowed_link_url(&format!("https://e.com/{}", "😀".repeat(2042))).is_none());
    }
}
