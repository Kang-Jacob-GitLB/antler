//! HTTP 응답 해석의 순수 부분: `Retry-After`·rate limit 헤더·`Link` 의 next·오류 본문의 메시지·상태 코드별 오류 분류.
//! 원본은 `src/main/jira/client.ts`(`parseRetryAfter`·`bodyMessage`·`httpError`)와 `src/main/github/client.ts`(`hasNextLink`·`parseRate`·`bodyMessage`·`httpError`)다.
//! 오류 문구는 TS 와 글자 그대로 같고, 비밀은 어디에도 싣지 않는다(본문 메시지는 길이를 자른다).
//!
//! 알려진 차이:
//! - [`parse_retry_after`] 의 날짜는 IMF-fixdate·RFC 850·asctime 만 받는다. TS 의 `Date.parse` 는 ISO 8601 같은 임의 형식도 받으므로 Rust 가 더 엄격하다(받으면 같은 값).
//!   asctime 은 시간대가 없어 TS 는 로컬 시간으로, Rust 는 HTTP 규격대로 UTC 로 푼다(서버가 보내는 형식이 아니라 골든 값 비교에서 뺀다).
//! - 본문 메시지를 UTF-16 200 단위로 자르다 서로게이트 쌍이 쪼개지면 Rust 는 U+FFFD 다(TS 는 짝 없는 서로게이트를 그대로 둔다).
use super::json::{self, js_number, js_number_opt, js_number_string, Json};
use crate::git::fetch::{js_trim, slice_utf16_lossy};
use crate::git::scope::is_js_space;
use crate::jira::JiraKind;

/// 사용자에게 그대로 보여 줘도 되는 오류. `code` 는 TS 의 `JiraErrorCode`·`GhErrorCode` 와 같은 문자열이다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub code: &'static str,
    pub status: u16,
    pub message: String,
}

// ── Link · rate limit ─────────────────────────────────────────────────────

/// TS `hasNextLink`: `/<[^>]*>\s*;\s*rel="?next"?/i` — `Link` 헤더에 rel="next" 가 있는가
pub fn has_next_link(link: Option<&str>) -> bool {
    let Some(link) = link else { return false };
    link.char_indices().filter(|&(_, c)| c == '<').any(|(at, _)| {
        // `[^>]*>` 는 `<` 뒤 첫 `>` 에서만 맞는다
        let after = &link[at + 1..];
        let Some(close) = after.find('>') else { return false };
        let rest = after[close + 1..].trim_start_matches(is_js_space);
        let Some(rest) = rest.strip_prefix(';') else { return false };
        let rest = rest.trim_start_matches(is_js_space);
        // `i` 플래그(유니코드 아님)는 ASCII 만 접는다
        let Some(rest) = strip_prefix_ascii_ci(rest, "rel=") else { return false };
        let rest = rest.strip_prefix('"').unwrap_or(rest);
        strip_prefix_ascii_ci(rest, "next").is_some()
    })
}

fn strip_prefix_ascii_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateInfo {
    pub remaining: f64,
    pub limit: f64,
    /// epoch seconds
    pub reset: f64,
}

/// rate limit 에 관한 응답 헤더(없으면 `None`)
#[derive(Clone, Copy, Debug, Default)]
pub struct RateHeaders<'a> {
    pub remaining: Option<&'a str>,
    pub limit: Option<&'a str>,
    pub reset: Option<&'a str>,
    pub retry_after: Option<&'a str>,
}

/// TS `parseRate`. `x-ratelimit-remaining` 이 없으면 `None`; limit·reset 이 없으면 `Number(null)` = 0 이다(TS 와 같다)
pub fn parse_rate(h: &RateHeaders<'_>) -> Option<RateInfo> {
    let (remaining, limit, reset) = (js_number_opt(h.remaining), js_number_opt(h.limit), js_number_opt(h.reset));
    if h.remaining.is_none() || !remaining.is_finite() || !limit.is_finite() || !reset.is_finite() {
        return None;
    }
    Some(RateInfo { remaining, limit, reset })
}

// ── Retry-After ───────────────────────────────────────────────────────────

/// TS `parseRetryAfter`: 초 또는 HTTP-date → 남은 초(올림). 해석할 수 없으면 `None`. `now_ms` 는 `Date.now()`
pub fn parse_retry_after(v: Option<&str>, now_ms: f64) -> Option<f64> {
    let v = v.filter(|v| !v.is_empty())?;
    let sec = js_number(v);
    if sec.is_finite() && sec >= 0.0 {
        return Some(sec.ceil());
    }
    let at = parse_http_date(v)?;
    let left = ((at - now_ms) / 1000.0).ceil();
    Some(if left > 0.0 { left } else { 0.0 })
}

const WEEKDAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAYS_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// HTTP-date 의 세 형식(RFC 9110): IMF-fixdate `Sun, 06 Nov 1994 08:49:37 GMT`, RFC 850 `Sunday, 06-Nov-94 08:49:37 GMT`, asctime `Sun Nov  6 08:49:37 1994`.
/// epoch ms. 요일은 이름만 보고 날짜와 맞는지는 보지 않는다(V8 도 같다). 두 자리 연도는 V8 처럼 0..49 → 2000년대, 50..99 → 1900년대다
pub fn parse_http_date(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(' ').collect();
    match parts.as_slice() {
        [wd, day, mon, year, time, "GMT"] if WEEKDAYS_SHORT.iter().any(|w| wd.strip_suffix(',') == Some(*w)) => build(day, mon, year, time),
        [wd, dmy, time, "GMT"] if WEEKDAYS_LONG.iter().any(|w| wd.strip_suffix(',') == Some(*w)) => {
            let mut it = dmy.split('-');
            let (day, mon, yy) = (it.next()?, it.next()?, it.next()?);
            if it.next().is_some() || yy.len() != 2 || !yy.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let yy: u32 = yy.parse().ok()?;
            build(day, mon, &(if yy < 50 { 2000 + yy } else { 1900 + yy }).to_string(), time)
        }
        _ => parse_asctime(s),
    }
}

fn parse_asctime(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(' ').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [wd, mon, day, time, year] if WEEKDAYS_SHORT.contains(wd) => build(day, mon, year, time),
        _ => None,
    }
}

fn two_digits(s: &str) -> Option<u32> {
    (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn build(day: &str, mon: &str, year: &str, time: &str) -> Option<f64> {
    if !(1..=2).contains(&day.len()) || !day.bytes().all(|b| b.is_ascii_digit()) || year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (d, y): (u32, i64) = (day.parse().ok()?, year.parse().ok()?);
    let m = MONTHS.iter().position(|n| *n == mon)? as u32 + 1;
    let mut t = time.split(':');
    let (hh, mm, ss) = (two_digits(t.next()?)?, two_digits(t.next()?)?, two_digits(t.next()?)?);
    if t.next().is_some() || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if d < 1 || d > dim {
        return None;
    }
    Some((days_from_civil(y, m, d) * 86_400 + i64::from(hh) * 3600 + i64::from(mm) * 60 + i64::from(ss)) as f64 * 1000.0)
}

/// 1970-01-01 부터의 일 수(Howard Hinnant 의 civil 알고리즘)
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * i64::from(if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ── 오류 본문 메시지 ─────────────────────────────────────────────────────

/// GitHub 오류 본문의 `message`(문자열일 때만, 200 단위까지). 해석 실패·없음은 빈 문자열
pub fn github_body_message(text: &str) -> String {
    match json::parse(text) {
        Ok(b) => b.get("message").and_then(Json::as_str).map(|m| slice_utf16_lossy(m, 200)).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Jira 오류 본문의 `errorMessages[0]` 또는 `message`, 없으면 필드별 오류(`errors`)의 문자열 값들을 ` / ` 로 이은 것.
/// 공백·제어 문자 덩어리는 한 칸으로 줄이고 200 단위까지만 남긴다. 해석 실패·없음은 빈 문자열
pub fn jira_body_message(text: &str) -> String {
    let Ok(b) = json::parse(text) else { return String::new() };
    let first = match b.get("errorMessages") {
        Some(Json::Arr(a)) => a.first().and_then(Json::as_str),
        _ => None,
    };
    let mut m: Option<String> = match first {
        Some(s) => Some(s.to_string()),
        None => b.get("message").and_then(Json::as_str).map(str::to_string),
    };
    if m.as_deref().is_none_or(str::is_empty) {
        let fields: Vec<&str> = match b.get("errors") {
            Some(e @ Json::Obj(_)) => e.entries_js_order().into_iter().filter_map(|(_, v)| v.as_str()).collect(),
            Some(Json::Arr(a)) => a.iter().filter_map(Json::as_str).collect(),
            _ => Vec::new(),
        };
        if !fields.is_empty() {
            m = Some(fields.join(" / "));
        }
    }
    let Some(m) = m else { return String::new() };
    // `[\s\u0000-\u001f\u007f]+` → 한 칸
    let mut squeezed = String::with_capacity(m.len());
    let mut in_run = false;
    for c in m.chars() {
        if is_js_space(c) || c <= '\u{1f}' || c == '\u{7f}' {
            if !in_run {
                squeezed.push(' ');
            }
            in_run = true;
        } else {
            squeezed.push(c);
            in_run = false;
        }
    }
    slice_utf16_lossy(js_trim(&squeezed), 200)
}

// ── 오류 분류 ─────────────────────────────────────────────────────────────

fn with_message(prefix: String, msg: &str) -> String {
    if msg.is_empty() {
        prefix
    } else {
        format!("{prefix}: {msg}")
    }
}

/// TS Jira `httpError`: 상태 코드별 분류와 문구. `not_found` 는 호출자가 정한 404 문장
pub fn jira_http_error(status: u16, retry_after: Option<&str>, body: &str, kind: JiraKind, not_found: Option<&str>, now_ms: f64) -> ApiError {
    let (code, message) = match status {
        401 => {
            let how = if kind == JiraKind::Cloud { "이메일·API 토큰을 확인하고" } else { "Personal Access Token 을 확인하고" };
            ("JIRA_AUTH", format!("Jira 인증이 거부되었습니다. {how} 다시 연결하세요"))
        }
        403 => ("JIRA_FORBIDDEN", with_message("Jira 접근이 거부되었습니다".into(), &jira_body_message(body))),
        404 => ("JIRA_NOT_FOUND", not_found.unwrap_or("요청한 항목을 찾을 수 없거나 볼 권한이 없습니다").to_string()),
        429 => {
            let m = match parse_retry_after(retry_after, now_ms) {
                Some(w) if w > 0.0 => format!("Jira 요청이 너무 잦습니다. {}초 후에 다시 시도하세요", js_number_string(w)),
                _ => "Jira 요청이 너무 잦습니다. 잠시 후 다시 시도하세요".to_string(),
            };
            ("JIRA_RATE_LIMIT", m)
        }
        s => ("JIRA_HTTP", with_message(format!("Jira API 오류({s})"), &jira_body_message(body))),
    };
    ApiError { code, status, message }
}

/// TS GitHub `httpError`. `format_time` 은 한도가 풀리는 시각(epoch 초)의 표시 문자열(TS 는 ko-KR 로컬 시각 — 시간대에 달려 있어 호출자가 넣는다)
pub fn github_http_error(status: u16, h: &RateHeaders<'_>, body: &str, signed_in: bool, format_time: &dyn Fn(f64) -> String) -> ApiError {
    let (code, message) = match status {
        401 => ("GH_AUTH", "GitHub 인증이 만료되었거나 잘못되었습니다. 다시 로그인하세요".to_string()),
        403 | 429 => {
            let rate = parse_rate(h);
            let retry_after = js_number_opt(h.retry_after);
            if let Some(rate) = rate.filter(|r| r.remaining == 0.0) {
                let hint = if signed_in { "" } else { " 로그인하면 한도가 크게 늘어납니다." };
                ("GH_RATE_LIMIT", format!("GitHub API 호출 한도를 초과했습니다. {} 이후에 다시 시도하세요.{hint}", format_time(rate.reset)))
            } else if retry_after.is_finite() && retry_after > 0.0 {
                ("GH_RATE_LIMIT", format!("GitHub 요청이 너무 잦습니다. {}초 후에 다시 시도하세요", js_number_string(retry_after.ceil())))
            } else if status == 429 {
                ("GH_RATE_LIMIT", "GitHub 요청이 너무 잦습니다. 잠시 후 다시 시도하세요".to_string())
            } else {
                ("GH_FORBIDDEN", with_message("GitHub 접근이 거부되었습니다".into(), &github_body_message(body)))
            }
        }
        404 => ("GH_NOT_FOUND", "저장소를 찾을 수 없거나 접근 권한이 없습니다(비공개 저장소면 로그인 필요)".to_string()),
        s => ("GH_HTTP", with_message(format!("GitHub API 오류({s})"), &github_body_message(body))),
    };
    ApiError { code, status, message }
}
