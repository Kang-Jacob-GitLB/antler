//! renderer 가 보낸 Jira IPC 인자 검증(`src/main/jira/validate.ts` 이식). IPC 인자는 신뢰하지 않는다.
//! 오류 문구·검사 순서는 TS 와 같고 골든 `validate.json` 이 비교한다. 길이는 UTF-16 단위, 공백 제거는 JS `trim` 이다.
use std::collections::HashSet;

use serde_json::Value;

use super::JiraKind;
use crate::git::commit::utf16_len;
use crate::git::fetch::js_trim;
use crate::net::json::{arg_number, field, has_control, is_integer, is_objectish};
use crate::net::redact::SecretString;
use crate::net::url_rules::{has_credentials, has_hash, has_search, js_origin, parse_url};

const MAX_TOKEN_LEN: usize = 1024;
const MAX_EMAIL_LEN: usize = 254;
const MAX_URL_LEN: usize = 2048;
pub const MAX_JQL_LEN: usize = 2000;
/// 목록은 한 번에 이 개수씩, 페이지는 이 횟수까지만 이어 받는다(최대 MAX_PAGES * PAGE_SIZE 건)
pub const PAGE_SIZE: u32 = 25;
pub const MAX_PAGES: u32 = 8;
pub const MAX_WORKLOG_USERS: usize = 30;
pub const MAX_WORKLOG_DAYS: i64 = 62;
const DAY_MS: i64 = 86_400_000;

type Res<T> = Result<T, String>;

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(msg.into())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SiteOptions {
    /// 테스트용 가짜 서버(127.0.0.1·localhost)에 한해 http 를 허용한다. 실제 사용에서는 항상 false.
    /// 필드는 비공개다 — [`SiteOptions::insecure_loopback_for_test`] 로만 켤 수 있고, 운영 코드(`src/**`)에서 그것을 부르지 않는다는 것을 `tests/net.rs` 의 소스 검사가 강제한다
    allow_insecure_loopback: bool,
}

impl SiteOptions {
    /// 테스트 전용: 루프백 http 를 허용한다. **운영 코드에서 부르지 않는다**
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn insecure_loopback_for_test() -> Self {
        Self { allow_insecure_loopback: true }
    }
}

/// 사이트 주소를 정규화한다: https 만(자격 증명·쿼리·해시 거부), 끝의 '/' 제거. Data Center 의 컨텍스트 경로(/jira)는 유지한다.
/// 반환값은 origin + 경로이며 API 경로를 그대로 이어 붙일 수 있다
pub fn parse_site_url(raw: &Value, opts: SiteOptions) -> Res<String> {
    let bad = || "Jira 사이트 주소가 올바르지 않습니다".to_string();
    let Some(raw) = raw.as_str() else { return Err(bad()) };
    if js_trim(raw).is_empty() || utf16_len(raw) > MAX_URL_LEN || has_control(raw) {
        return Err(bad());
    }
    let Some(u) = parse_url(js_trim(raw)) else { return err("Jira 사이트 주소가 올바르지 않습니다(예: https://회사.atlassian.net)") };
    let loopback = matches!(u.host_str(), Some("127.0.0.1" | "localhost"));
    if !(u.scheme() == "https" || (u.scheme() == "http" && opts.allow_insecure_loopback && loopback)) {
        return err("Jira 사이트 주소는 https:// 로 시작해야 합니다");
    }
    if has_credentials(&u) {
        return err("Jira 사이트 주소에 사용자 정보를 넣을 수 없습니다");
    }
    if has_search(&u) || has_hash(&u) {
        return err("Jira 사이트 주소에 쿼리·해시를 넣을 수 없습니다");
    }
    Ok(format!("{}{}", js_origin(&u), u.path().trim_end_matches('/')))
}

#[derive(Clone, Debug)]
pub struct ConnectInput {
    pub kind: JiraKind,
    pub site_url: String,
    pub email: String,
    pub token: SecretString,
}

fn is_visible_ascii(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

pub fn parse_connect_input(v: &Value, opts: SiteOptions) -> Res<ConnectInput> {
    let kind = match field(v, "kind").and_then(Value::as_str) {
        Some("cloud") => JiraKind::Cloud,
        Some("server") => JiraKind::Server,
        _ => return err("잘못된 연결 정보입니다"),
    };
    static NULL: Value = Value::Null;
    let site_url = parse_site_url(field(v, "siteUrl").unwrap_or(&NULL), opts)?;
    let Some(token) = field(v, "token").and_then(Value::as_str) else { return err("토큰이 올바르지 않습니다") };
    let token = js_trim(token);
    if token.is_empty() || utf16_len(token) > MAX_TOKEN_LEN || !is_visible_ascii(token) {
        return err("토큰 형식이 올바르지 않습니다(공백 없이 1024자 이하)");
    }
    let mut email = String::new();
    if kind == JiraKind::Cloud {
        email = field(v, "email").and_then(Value::as_str).map(|e| js_trim(e).to_string()).unwrap_or_default();
        // Basic 인증의 사용자 이름이므로 ':' 은 쓸 수 없다
        if email.is_empty() || utf16_len(&email) > MAX_EMAIL_LEN || has_control(&email) || email.contains(':') {
            return err("이메일이 올바르지 않습니다");
        }
    }
    Ok(ConnectInput { kind, site_url, email, token: SecretString::new(token) })
}

/// `^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$`
pub fn parse_issue_key(v: &Value) -> Res<String> {
    let ok = v.as_str().is_some_and(|s| {
        let Some((prefix, num)) = s.split_once('-') else { return false };
        let p = prefix.as_bytes();
        (2..=20).contains(&p.len())
            && p[0].is_ascii_uppercase()
            && p[1..].iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
            && (1..=9).contains(&num.len())
            && num.bytes().all(|b| b.is_ascii_digit())
    });
    if ok {
        Ok(v.as_str().unwrap_or_default().to_string())
    } else {
        err("잘못된 이슈 키입니다")
    }
}

/// 전환 id 는 Jira 가 정한 숫자 문자열이다: `^\d{1,10}$`
pub fn parse_transition_id(v: &Value) -> Res<String> {
    match v.as_str() {
        Some(s) if (1..=10).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) => Ok(s.to_string()),
        _ => err("잘못된 전환입니다"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    pub jql: String,
    pub page: u32,
    pub cursor: Option<String>,
}

pub fn parse_search_query(v: &Value) -> Res<SearchQuery> {
    let Some(jql) = field(v, "jql").and_then(Value::as_str) else { return err("잘못된 조회 조건입니다") };
    let jql = js_trim(jql);
    if jql.is_empty() || utf16_len(jql) > MAX_JQL_LEN || has_control(jql) {
        return err("JQL 이 올바르지 않습니다(2000자 이하)");
    }
    let page = match field(v, "page") {
        None => 1,
        Some(p) => match arg_number(p) {
            Some(x) if is_integer(x) && (1.0..=f64::from(MAX_PAGES)).contains(&x) => x as u32,
            _ => return err("잘못된 페이지입니다"),
        },
    };
    // `^[!-~]{1,2000}$`
    let cursor = match field(v, "cursor") {
        None => None,
        Some(Value::String(c)) if (1..=2000).contains(&c.len()) && c.bytes().all(|b| (0x21..=0x7e).contains(&b)) => Some(c.clone()),
        Some(_) => return err("잘못된 페이지 표지입니다"),
    };
    Ok(SearchQuery { jql: jql.to_string(), page, cursor })
}

/// Cloud accountId(`712020:uuid` 꼴 포함)와 Data Center 사용자 이름이 들어올 수 있는 최대 범위: `^[A-Za-z0-9._@:+-]{1,128}$`
pub fn is_user_id(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b':' | b'+' | b'-'))
}

/// 1970-01-01 부터의 일 수(유효한 날짜일 때만 쓴다)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// 'YYYY-MM-DD' 이면서 실재하는 날짜만(2월 30일 거부). UTC 자정의 epoch ms
fn day_ms(v: Option<&Value>) -> Res<i64> {
    let bad = || "날짜가 올바르지 않습니다(YYYY-MM-DD)".to_string();
    let s = v.and_then(Value::as_str).ok_or_else(bad)?;
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return Err(bad());
    }
    let (y, m, d): (i64, i64, i64) = (s[0..4].parse().map_err(|_| bad())?, s[5..7].parse().map_err(|_| bad())?, s[8..10].parse().map_err(|_| bad())?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if y < 1970 || !(1..=12).contains(&m) || d < 1 || d > dim {
        return err("존재하지 않는 날짜입니다");
    }
    Ok(days_from_civil(y, m, d) * DAY_MS)
}

/// JQL 문자열 값. 엄격 allowlist 를 통과한 값(날짜·사용자 id)만 받아 큰따옴표로 감싼다 — 따옴표·역슬래시가 들어올 수 없다
pub fn jql_quote(v: &str) -> Res<String> {
    if !is_user_id(v) {
        return err("JQL 값이 올바르지 않습니다");
    }
    Ok(format!("\"{v}\""))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorklogQuery {
    pub from: String,
    pub to: String,
    pub users: Vec<String>,
    pub include_me: bool,
}

pub fn parse_worklog_query(v: &Value) -> Res<WorklogQuery> {
    if !is_objectish(v) {
        return err("잘못된 워크로그 조회 조건입니다");
    }
    let from = day_ms(field(v, "from"))?;
    let to = day_ms(field(v, "to"))?;
    if from > to {
        return err("시작일이 종료일보다 늦습니다");
    }
    if (to - from) / DAY_MS + 1 > MAX_WORKLOG_DAYS {
        return err(format!("조회 범위는 최대 {MAX_WORKLOG_DAYS}일입니다"));
    }
    let users_err = || format!("사용자는 최대 {MAX_WORKLOG_USERS}명까지 고를 수 있습니다");
    let users = field(v, "users").and_then(Value::as_array).filter(|a| a.len() <= MAX_WORKLOG_USERS).ok_or_else(users_err)?;
    if !users.iter().all(|u| u.as_str().is_some_and(is_user_id)) {
        return err("사용자 id 가 올바르지 않습니다");
    }
    let include_me = match field(v, "includeMe") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return err("includeMe 가 올바르지 않습니다"),
    };
    let mut seen = HashSet::new();
    let users: Vec<String> = users.iter().filter_map(Value::as_str).filter(|u| seen.insert(*u)).map(str::to_string).collect();
    let from_s = field(v, "from").and_then(Value::as_str).unwrap_or_default().to_string();
    let to_s = field(v, "to").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(WorklogQuery { from: from_s, to: to_s, users, include_me })
}

/// 사용자 검색어: 1~100자, 제어문자 금지
pub fn parse_user_search(v: &Value) -> Res<String> {
    let t = v.as_str().map_or("", js_trim);
    if t.is_empty() || utf16_len(t) > 100 || has_control(t) {
        return err("검색어가 올바르지 않습니다(1~100자)");
    }
    Ok(t.to_string())
}

/// jira:openExternal 허용 대상: https(테스트에서는 허용된 루프백 http), origin 이 연결한 사이트와 정확히 같고 자격 증명이 없는 URL 만. 정규화된 href 를 돌려준다
pub fn allowed_jira_url(raw: &Value, site_url: Option<&str>, opts: SiteOptions) -> Option<String> {
    let raw = raw.as_str().filter(|s| utf16_len(s) <= MAX_URL_LEN)?;
    let site_url = site_url.filter(|s| !s.is_empty())?;
    let u = parse_url(raw)?;
    let site = parse_url(site_url)?;
    if has_credentials(&u) || js_origin(&u) != js_origin(&site) {
        return None;
    }
    if u.scheme() != "https" && !(opts.allow_insecure_loopback && u.scheme() == "http") {
        return None;
    }
    Some(u.to_string())
}
