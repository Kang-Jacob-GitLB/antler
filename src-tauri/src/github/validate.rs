//! renderer 가 보낸 GitHub IPC 인자 검증(`src/main/github/validate.ts` 이식). IPC 인자는 신뢰하지 않는다.
//! 입력은 JSON 을 거친 `serde_json::Value` 이고, 오류 문구·검사 순서는 TS 와 같다(골든 `validate.json` 이 비교한다).
//! 길이는 UTF-16 단위, 공백 제거는 JS `trim`(U+FEFF 포함·U+0085 제외)이다.
use std::collections::HashSet;

use serde_json::Value;

use crate::git::commit::utf16_len;
use crate::git::fetch::{js_to_lowercase, js_trim};
use crate::net::json::{arg_number, field, has_control, is_integer, is_objectish, is_safe_integer};
use crate::net::redact::SecretString;

const MAX_FILTER_LEN: usize = 100;
const MAX_PAT_LEN: usize = 255;
const MAX_ID_LEN: usize = 200;
const MAX_PAGE: f64 = 1000.0;
const MAX_AVATAR_AUTHORS: usize = 200;
const MAX_EMAIL_LEN: usize = 254;
const MAX_CHECK_SHAS: usize = 100;

type Res<T> = Result<T, String>;

fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(msg.into())
}

pub fn parse_number(n: &Value) -> Res<u32> {
    match arg_number(n) {
        Some(x) if is_integer(x) && x > 0.0 && x < 1e9 => Ok(x as u32),
        _ => err("잘못된 번호입니다"),
    }
}

pub fn parse_id(v: &Value, what: &str) -> Res<String> {
    match v.as_str() {
        Some(s) if !s.is_empty() && utf16_len(s) <= MAX_ID_LEN && !has_control(s) => Ok(s.to_string()),
        _ => err(format!("잘못된 {what}입니다")),
    }
}

/// 빈 값(없음·null·빈 문자열)은 `None`
fn parse_filter(v: Option<&Value>, what: &str) -> Res<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) if utf16_len(s) <= MAX_FILTER_LEN && !has_control(s) => Ok(Some(s.clone())),
        _ => err(format!("잘못된 {what}입니다")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhListQuery {
    pub state: String,
    pub label: Option<String>,
    pub assignee: Option<String>,
    pub page: u32,
}

/// 목록 질의를 검증하고 정규화한다(빈 필터는 제거, page 기본값 1)
pub fn parse_list_query(q: &Value) -> Res<GhListQuery> {
    let state = field(q, "state").and_then(Value::as_str).filter(|s| matches!(*s, "open" | "closed" | "all"));
    let Some(state) = state else { return err("잘못된 조회 조건입니다") };
    let label = parse_filter(field(q, "label"), "라벨")?;
    let assignee = parse_filter(field(q, "assignee"), "담당자")?;
    let page = match field(q, "page") {
        None => 1,
        Some(p) => match arg_number(p) {
            Some(x) if is_integer(x) && (1.0..=MAX_PAGE).contains(&x) => x as u32,
            _ => return err("잘못된 페이지입니다"),
        },
    };
    Ok(GhListQuery { state: state.to_string(), label, assignee, page })
}

/// 보이는 ASCII(`\x21-\x7e`)만, 비어 있지 않게
fn is_visible_ascii(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// PAT: 앞뒤 공백을 자르고, 보이는 ASCII 만(공백·제어 문자 없음) 255자까지
pub fn parse_pat(v: &Value) -> Res<SecretString> {
    let Some(s) = v.as_str() else { return err("토큰이 올바르지 않습니다") };
    parse_pat_str(s)
}

/// [`parse_pat`] 의 문자열 판 — 밖의 출처(gh CLI)가 준 토큰을 `Value` 로 한 번 더 복사하지 않고 검사한다
pub fn parse_pat_str(s: &str) -> Res<SecretString> {
    let t = js_trim(s);
    if t.is_empty() || utf16_len(t) > MAX_PAT_LEN || !is_visible_ascii(t) {
        return err("토큰 형식이 올바르지 않습니다(공백 없이 255자 이하)");
    }
    Ok(SecretString::new(t))
}

/// `^(?:[0-9a-f]{40}|[0-9a-f]{64})$` 를 `i` 로(ASCII 만 접는다)
fn is_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarQuery {
    pub email: String,
    pub sha: String,
}

/// gh:avatars 의 authors 인자: 최대 200건, 이메일 254자 이하(제어 문자 없음), sha 는 40/64자리 16진수.
/// 같은 이메일(대소문자 무시)은 처음 것만 남긴다. 하나라도 형식이 틀리면 거부한다
pub fn parse_avatar_authors(v: &Value) -> Res<Vec<AvatarQuery>> {
    let Some(items) = v.as_array().filter(|a| a.len() <= MAX_AVATAR_AUTHORS) else { return err("잘못된 작성자 목록입니다") };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if !is_objectish(item) {
            return err("잘못된 작성자 항목입니다");
        }
        let email = match field(item, "email").and_then(Value::as_str) {
            Some(e) if utf16_len(e) >= 3 && utf16_len(e) <= MAX_EMAIL_LEN && !has_control(e) => e,
            _ => return err("잘못된 작성자 이메일입니다"),
        };
        let sha = match field(item, "sha").and_then(Value::as_str) {
            Some(s) if is_sha(s) => s,
            _ => return err("잘못된 커밋 해시입니다"),
        };
        if !seen.insert(js_to_lowercase(email)) {
            continue;
        }
        out.push(AvatarQuery { email: email.to_string(), sha: sha.to_ascii_lowercase() });
    }
    Ok(out)
}

/// 워크플로 실행·잡 ID: 양의 안전 정수(실제 ID 는 10자리를 넘는다)
pub fn parse_run_id(v: &Value) -> Res<u64> {
    match arg_number(v) {
        Some(x) if is_safe_integer(x) && x > 0.0 => Ok(x as u64),
        _ => err("잘못된 실행 ID입니다"),
    }
}

/// 커밋 해시 하나(40/64자리 16진수), 소문자로 정규화
pub fn parse_sha(v: &Value) -> Res<String> {
    match v.as_str() {
        Some(s) if is_sha(s) => Ok(s.to_ascii_lowercase()),
        _ => err("잘못된 커밋 해시입니다"),
    }
}

/// gh:commitChecks 의 sha 목록: 최대 100건, 중복 제거(처음 순서)
pub fn parse_check_shas(v: &Value) -> Res<Vec<String>> {
    let Some(items) = v.as_array().filter(|a| a.len() <= MAX_CHECK_SHAS) else { return err("잘못된 커밋 목록입니다") };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let sha = parse_sha(item)?;
        if seen.insert(sha.clone()) {
            out.push(sha);
        }
    }
    Ok(out)
}

/// 목록 페이지 번호(1 이상)
pub fn parse_page(v: &Value) -> Res<u32> {
    match arg_number(v) {
        Some(x) if is_integer(x) && (1.0..=MAX_PAGE).contains(&x) => Ok(x as u32),
        _ => err("잘못된 페이지입니다"),
    }
}

/// gh:openExternal 허용 대상: origin 이 정확히 https://github.com 인 URL 만(자격 증명·다른 호스트 거부). 정규화된 href 를 돌려준다
pub fn allowed_external_url(raw: &Value) -> Option<String> {
    use crate::net::url_rules::{has_credentials, js_origin, parse_url};
    let raw = raw.as_str().filter(|s| utf16_len(s) <= 2048)?;
    let u = parse_url(raw)?;
    if u.scheme() != "https" || js_origin(&u) != "https://github.com" || has_credentials(&u) {
        return None;
    }
    Some(u.to_string())
}
