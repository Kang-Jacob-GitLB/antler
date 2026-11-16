//! URL 판정과 요청 주소 조립. `url` 크레이트는 WHATWG 규칙이라 Node 의 `new URL` 과 같은 결과를 내는 것이 기본이고,
//! 어긋나는 입력은 골든 격자(`tests/golden/net/urls.json`)가 잡는다 — 허용하는 쪽이 TS 보다 넓으면 테스트가 실패한다.
//!
//! JS `URL` 과 맞추는 곳: `origin` 은 [`js_origin`](불투명 origin 은 `"null"`), `search`·`hash` 는 비어 있으면 `''` 라
//! `https://h/?` 의 빈 쿼리·`#` 의 빈 해시는 "있는 것"이 아니다([`has_search`]·[`has_hash`]), `username`·`password` 는 [`has_credentials`].
use url::Url;

/// UTS 46 의 처리 방식에 따라 결과가 갈리는 문자(ẞ). Node(ada)는 `ss`, `url` 크레이트(idna)는 `xn--zca`(ß)로 만들어 다른 서버를 가리킨다
const IDNA_DEVIATION: [char; 1] = ['\u{1e9e}'];

/// IDN 호스트에서 받는 비 ASCII 문자의 범위(Unicode 판이 바뀌어도 새로 배정된 문자가 없는 오래된 블록). Node 22 는 Unicode 16, `url` 크레이트의 idna 는 더 새 판이라
/// (예: U+A7CE) 새로 배정된 문자가 든 호스트를 Rust 만 받는다 — 판 차이를 막으려고 이 밖의 문자는 거부한다.
/// 골든 `urls.json` 의 `idna` 가 이 범위의 모든 코드 포인트를 TS 와 같은 호스트로 푸는지 대조한다
pub const IDNA_SAFE_RANGES: &[(u32, u32)] = &[
    (0x00A1, 0x024F), // Latin-1 보충·확장 A·B
    (0x0250, 0x02AF), // IPA
    (0x0300, 0x036F), // 결합 부호
    (0x0370, 0x03FF), // 그리스
    (0x0400, 0x052F), // 키릴·키릴 보충
    (0x0530, 0x058F), // 아르메니아
    (0x0590, 0x05FF), // 히브리
    (0x0600, 0x06FF), // 아랍
    (0x0E00, 0x0E7F), // 태국
    (0x1100, 0x11FF), // 한글 자모
    (0x1E00, 0x1FFF), // 라틴 확장 추가·그리스 확장
    (0x3000, 0x303F), // CJK 기호·문장 부호(。 포함)
    (0x3040, 0x30FF), // 히라가나·가타카나
    (0x3400, 0x4DBF), // CJK 확장 A
    (0x4E00, 0x9FFF), // CJK 통합 한자
    (0xAC00, 0xD7A3), // 한글 음절
    (0xFF00, 0xFFEF), // 전각·반각 형태
];

/// [`IDNA_SAFE_RANGES`] 안에서도 Node(Unicode 16)는 거부하는데 idna 는 받는 문자 — 골든 전수 대조(`idna_허용_범위의_모든_문자가_ts_와_같은_호스트로_풀린다`)가 찾은 것.
/// idna 크레이트를 올려 새로 어긋나는 문자가 생기면 그 테스트가 실패하니 여기에 더하거나 범위를 줄인다
pub const IDNA_SAFE_EXCLUDED: &[u32] = &[0x04C0, 0x0558, 0x058B, 0x058C, 0x05C8, 0x05C9, 0x115F, 0x1160, 0xFFA0];

fn idna_safe(c: char) -> bool {
    c.is_ascii() || (IDNA_SAFE_RANGES.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32))) && !IDNA_SAFE_EXCLUDED.contains(&(c as u32)))
}

/// 입력 문자열에서 호스트가 든 부분(스킴 뒤 `/`·`\` 를 건너뛰고 다음 `/`·`\`·`?`·`#` 까지). 스킴이 없으면 처음부터
fn raw_authority(raw: &str) -> &str {
    let t = raw.trim_start_matches(|c: char| c <= ' ');
    let after = match t.find(':') {
        Some(i) if t[..i].bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) => &t[i + 1..],
        _ => t,
    };
    let after = after.trim_start_matches(['/', '\\']);
    &after[..after.find(['/', '\\', '?', '#']).unwrap_or(after.len())]
}

/// RFC 3492 punycode 해독(`xn--` 뒤의 부분). 잘못된 입력은 `None`
pub fn punycode_decode(input: &str) -> Option<Vec<char>> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    fn adapt(delta: u32, points: u32, first: bool) -> u32 {
        let mut delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE - TMIN + 1) * delta / (delta + 38)
    }
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (0x80u32, 0u32, 72u32);
    let mut bytes = rest.bytes().peekable();
    while bytes.peek().is_some() {
        let old_i = i;
        let (mut w, mut k) = (1u32, BASE);
        loop {
            let digit = match bytes.next()? {
                b @ b'a'..=b'z' => b - b'a',
                b @ b'A'..=b'Z' => b - b'A',
                b @ b'0'..=b'9' => b - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
            if digit < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - old_i, len, old_i == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, char::from_u32(n)?);
        i += 1;
    }
    Some(out)
}

/// `new URL(raw)` 가 성공하면 그 결과(상대 주소는 실패).
///
/// TS 보다 엄격한 곳(알려진 차이): (1) IDN 호스트의 `xn--` 레이블은 해독해서 비 ASCII 문자가 [`IDNA_SAFE_RANGES`] 안에 있을 때만 받는다 —
/// 잘못된 punycode(`xn--github-`)와 Node(Unicode 16)가 모르는 새 문자를 거부한다(Rust 만 받으면 TS 가 거부하는 호스트를 허용하게 된다),
/// (2) [`IDNA_DEVIATION`] 문자가 든 주소의 호스트가 IDN 이면 거부한다. 호스트가 ASCII 면 경로의 그 문자는 상관없다.
/// (3) 호스트 부분에 비 ASCII 바이트의 퍼센트 이스케이프(`%D3%80`)가 있으면 거부한다(해독한 문자를 입력 쪽에서 검사할 수 없다),
/// (4) `blob:` 는 받지 않는다(origin 이 안쪽 주소의 것이라 호스트 검사를 비켜 간다 — 이 앱은 쓰지 않는다).
/// 허용 범위 안의 문자는 골든(`idna`)에서 TS 와 같은 호스트로 풀린다
pub fn parse_url(raw: &str) -> Option<Url> {
    let u = Url::parse(raw).ok()?;
    host_ok(&u, raw).then_some(u)
}

/// 파싱한 URL 의 호스트가 TS(Node)와 같게 풀린다고 믿을 수 있는가. `raw` 는 파싱한 원문(리다이렉트면 `Location`)
fn host_ok(u: &Url, raw: &str) -> bool {
    if u.scheme() == "blob" {
        return false;
    }
    // 파서는 탭·개행을 지우고 읽는다 — 같은 모양으로 맞춘 뒤 본다
    let clean: String = raw.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let auth = raw_authority(&clean);
    // 입력에 쓴 비 ASCII 문자가 모두 허용 범위여야 한다 — idna 가 새 Unicode 판에서 다른 문자로 바꿔 받는 문자(U+04C0 → U+04CF 등)를 TS 는 거부한다.
    // 결과 호스트(해독한 `xn--` 레이블)만 보면 이 입력 쪽 문자가 가려진다
    if !auth.chars().all(idna_safe) {
        return false;
    }
    if auth.as_bytes().windows(3).any(|w| w[0] == b'%' && matches!(w[1], b'8'..=b'9' | b'a'..=b'f' | b'A'..=b'F') && w[2].is_ascii_hexdigit()) {
        return false;
    }
    if let Some(host) = u.domain() {
        for label in host.split('.') {
            if let Some(p) = label.strip_prefix("xn--") {
                let ok = punycode_decode(p).is_some_and(|cs| cs.iter().any(|c| !c.is_ascii()) && cs.iter().all(|&c| idna_safe(c)));
                if !ok || clean.contains(IDNA_DEVIATION) {
                    return false;
                }
            }
        }
    }
    true
}

/// JS `url.origin`: 튜플 origin 은 `scheme://host[:port]`, 그 밖(file 등)은 `"null"`
pub fn js_origin(u: &Url) -> String {
    u.origin().ascii_serialization()
}

/// JS `url.username || url.password`
pub fn has_credentials(u: &Url) -> bool {
    !u.username().is_empty() || u.password().is_some_and(|p| !p.is_empty())
}

/// JS `url.search` 가 빈 문자열이 아닌가(`?` 만 있으면 빈 문자열이다)
pub fn has_search(u: &Url) -> bool {
    u.query().is_some_and(|q| !q.is_empty())
}

/// JS `url.hash` 가 빈 문자열이 아닌가(`#` 만 있으면 빈 문자열이다)
pub fn has_hash(u: &Url) -> bool {
    u.fragment().is_some_and(|f| !f.is_empty())
}

/// 경로에 인코딩된 `/`·`\`·`.`(`%2f`·`%5c`·`%2e`, 대소문자 무시)가 있는가 — 앞단 프록시가 풀어 사이트 경로 밖으로 보낼 수 있다
fn has_encoded_separator(path: &str) -> bool {
    path.as_bytes().windows(3).any(|w| w[0] == b'%' && matches!([w[1].to_ascii_lowercase(), w[2].to_ascii_lowercase()], [b'2', b'f'] | [b'5', b'c'] | [b'2', b'e']))
}

/// TS `inSite`: 주소가 연결한 사이트 안(같은 origin, 사이트 경로 아래)인가. 사이트 주소가 URL 이 아니면 `false`(TS 는 던진다 — 더 엄격한 쪽이다)
pub fn in_site(url: &Url, site_url: &str) -> bool {
    let Some(site) = parse_url(site_url) else { return false };
    let base = site.path().trim_end_matches('/');
    if has_encoded_separator(url.path()) {
        return false;
    }
    js_origin(url) == js_origin(&site) && (base.is_empty() || url.path() == base || url.path().strip_prefix(base).is_some_and(|rest| rest.starts_with('/')))
}

/// TS `PUBLIC_AVATAR_HOST_RE` = `^(?:secure\.gravatar\.com|i[0-3]\.wp\.com|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.atl-paas\.net)$` 의 손 판정(대소문자 구분)
pub fn is_public_avatar_host(host: &str) -> bool {
    if host == "secure.gravatar.com" {
        return true;
    }
    if let Some(n) = host.strip_prefix('i').and_then(|r| r.strip_suffix(".wp.com")) {
        if matches!(n, "0" | "1" | "2" | "3") {
            return true;
        }
    }
    host.strip_suffix(".atl-paas.net").is_some_and(|prefix| prefix.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')))
}

/// 프로필 사진 요청 한 번(홉)의 판정
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageHop {
    /// 요청하지 않는다
    Reject,
    /// 연결한 사이트 안의 주소 — 인증을 붙이고 리다이렉트를 따라가지 않는다(첫 홉만)
    Own,
    /// 공개 사진 호스트(https·기본 포트) — 인증 없이, 그 호스트들 사이의 리다이렉트만 따라간다
    Public,
}

/// TS `JiraClient.image` 의 홉별 판정: 자격 증명이 든 주소는 거부, 사이트 안은 첫 홉에서만 인정, 그 밖은 공개 사진 호스트만
pub fn image_hop(url: &Url, site_url: &str, hop: usize) -> ImageHop {
    if has_credentials(url) {
        return ImageHop::Reject;
    }
    if hop == 0 && in_site(url, site_url) {
        return ImageHop::Own;
    }
    if url.scheme() == "https" && url.port().is_none() && url.host_str().is_some_and(is_public_avatar_host) {
        ImageHop::Public
    } else {
        ImageHop::Reject
    }
}

/// 리다이렉트 `Location` 을 현재 주소 기준으로 푼다(`new URL(loc, url)`). `Location` 이 호스트를 바꾸면(절대 주소·`//호스트`) [`parse_url`] 과 같은 호스트 검사를 거친다
pub fn resolve_location(base: &Url, loc: &str) -> Option<Url> {
    let joined = base.join(loc).ok()?;
    if (joined.host_str() != base.host_str() || joined.scheme() != base.scheme()) && !host_ok(&joined, loc) {
        return None;
    }
    Some(joined)
}

/// `url.searchParams.set(k, v)` 를 차례로 한 결과를 URL 에 반영한다: 같은 키는 첫 자리의 값을 바꾸고 나머지를 지우며,
/// 하나라도 설정하면 쿼리 전체를 `application/x-www-form-urlencoded`(공백은 `+`)로 다시 쓴다. 비어 있으면 쿼리를 없앤다
pub fn set_query_params(url: &mut Url, params: &[(&str, &str)]) {
    if params.is_empty() {
        return;
    }
    let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    for (k, v) in params {
        match pairs.iter().position(|(pk, _)| pk == k) {
            Some(first) => {
                pairs[first].1 = (*v).to_string();
                let mut i = first + 1;
                while i < pairs.len() {
                    if pairs[i].0 == *k {
                        pairs.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
            None => pairs.push(((*k).to_string(), (*v).to_string())),
        }
    }
    let q = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(&pairs).finish();
    url.set_query(if q.is_empty() { None } else { Some(&q) });
}

fn apply_query(url: &mut Url, query: &[(&str, Option<&str>)]) {
    let set: Vec<(&str, &str)> = query.iter().filter_map(|(k, v)| v.map(|v| (*k, v))).collect();
    set_query_params(url, &set);
}

pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// GitHub REST 요청 주소(TS `GitHubClient.get`): `/` 로 시작하는 경로만, origin 이 `https://api.github.com` 이어야 한다. `None` 이면 TS 는 `invalid api path` 를 던진다.
/// 값이 `None` 인 쿼리는 건너뛴다(TS 의 `undefined`)
pub fn github_request_url(path: &str, query: &[(&str, Option<&str>)]) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let mut url = parse_url(&format!("{GITHUB_API_BASE}{path}"))?;
    if js_origin(&url) != GITHUB_API_BASE {
        return None;
    }
    apply_query(&mut url, query);
    Some(url.to_string())
}

/// Jira REST 요청 주소(TS `JiraClient.target`): 사이트 상대 경로(`/` 로 시작, `//` 아님), origin 이 사이트와 같아야 한다. `None` 이면 TS 는 던진다
pub fn jira_request_url(site_url: &str, path: &str, query: &[(&str, Option<&str>)]) -> Option<String> {
    if !path.starts_with('/') || path.starts_with("//") {
        return None;
    }
    let site = parse_url(site_url)?;
    let mut url = parse_url(&format!("{site_url}{path}"))?;
    if js_origin(&url) != js_origin(&site) {
        return None;
    }
    apply_query(&mut url, query);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 공개_사진_호스트는_정규식과_같이_판정한다() {
        for h in ["secure.gravatar.com", "i0.wp.com", "i3.wp.com", "a.atl-paas.net", "a-b.c1.atl-paas.net", "x.atl-paas.net.atl-paas.net"] {
            assert!(is_public_avatar_host(h), "{h}");
        }
        for h in ["i4.wp.com", "ii.wp.com", "i.wp.com", ".atl-paas.net", "a..atl-paas.net", "atl-paas.net", "A.atl-paas.net", "a.atl-paas.net.", "secure.gravatar.com.", "gravatar.com", "i0.wp.com/x", "a_b.atl-paas.net"] {
            assert!(!is_public_avatar_host(h), "{h}");
        }
    }

    #[test]
    fn punycode_는_rfc_3492_대로_해독한다() {
        assert_eq!(punycode_decode("9ca").map(|c| c.into_iter().collect::<String>()).as_deref(), Some("é"));
        assert_eq!(punycode_decode("wgv").map(|c| c.into_iter().collect::<String>()).as_deref(), Some("日"));
        assert_eq!(punycode_decode("mnchen-3ya").map(|c| c.into_iter().collect::<String>()).as_deref(), Some("münchen"));
        assert_eq!(punycode_decode("!"), None);
        assert_eq!(punycode_decode("9"), None, "끝나지 않은 숫자열은 거부한다");
    }

    #[test]
    fn 쿼리_설정은_첫_자리를_바꾸고_공백은_더하기다() {
        let mut u = Url::parse("https://h/p?a=1&b=2&a=3").unwrap();
        set_query_params(&mut u, &[("a", "x y"), ("c", "é")]);
        assert_eq!(u.as_str(), "https://h/p?a=x+y&b=2&c=%C3%A9");
    }
}
