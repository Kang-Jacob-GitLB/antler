//! 업데이트 판정의 순수 로직(`src/main/update/logic.ts`·`releaseNotes.ts` 이식 + Tauri 매니페스트 검사). 네트워크·파일·시계를 쓰지 않는다.
//!
//! 이식 규칙은 `net` 과 같다: 공백은 JS `\s`([`is_js_space`]), 길이는 UTF-16 단위, 숫자는 f64(`Number()`), URL 은 `net::url_rules`(WHATWG).
//! "Rust 가 허용하면 TS 도 허용한다"가 깨지지 않는 방향으로만 다르게 만든다 — 어긋나는 곳은 각 함수에 적었다.
//! TS 에 없는 것은 [`check_manifest`]·[`prerelease_manifest_url`]·[`is_safe_file_name`] 의 태그 결속 등이다(골든이 아니라 `tests/update.rs` 가 단언한다).
use std::collections::HashSet;
use std::future::Future;
use std::str::FromStr;

use serde::Serialize;
use serde_json::Value;

use crate::config::model::utf16_len;
use crate::git::fetch::{js_trim, slice_utf16_lossy};
use crate::git::scope::is_js_space;
use crate::net::js_date::parse_date_ms;
use crate::net::js_value::encode_uri_component;
use crate::net::json::{self, Json};
use crate::net::url_rules::{has_credentials, has_encoded_separator, has_hash, has_search, js_origin, parse_url};

pub const REPO_OWNER: &str = "Kang-Jacob-GitLB";
pub const REPO_NAME: &str = "antler";
pub const RELEASE_DOWNLOAD_PREFIX: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases/download/";
/// 정식 판의 업데이트 매니페스트(Tauri `latest.json`). 최신 **정식** 릴리스(초안·프리릴리스 제외)로 리다이렉트된다
pub const STABLE_MANIFEST_URL: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json";
/// 프리릴리스 판이 최근 릴리스 목록을 읽는 API 경로(`manual.ts` 의 `latestRelease`)
pub const RELEASE_LIST_PATH: &str = "/releases?per_page=10";
pub const RELEASES_PAGE: &str = "https://github.com/Kang-Jacob-GitLB/antler/releases";
/// 매니페스트 본문 상한(바이트)
pub const MAX_MANIFEST_BYTES: usize = 1_000_000;
/// 릴리스 API 응답 상한(UTF-16 단위, `MAX_API_BYTES`)
pub const MAX_API_BYTES: usize = 2_000_000;
/// 릴리스 노트 본문이 이보다 길면 자르고 '더보기'로 넘긴다
pub const MAX_NOTES_CHARS: usize = 100_000;
/// 매니페스트 서명(minisign `.sig` 의 base64) 길이 상한. 실제는 수백 바이트다
pub const MAX_SIGNATURE_LEN: usize = 16 * 1024;
/// Windows NSIS 설치본의 매니페스트 플랫폼 키와, 없을 때 쓰는 일반 키
pub const PLATFORM_KEY_NSIS: &str = "windows-x86_64-nsis";
pub const PLATFORM_KEY_GENERIC: &str = "windows-x86_64";

const GENERIC_ERROR: &str = "업데이트 정보가 올바르지 않습니다";

/// TS 에서 손으로 옮긴 정규식의 소스 조각(이름, 조각). 골든 `semver.json` 의 handWritten 과 글자 그대로 같아야 한다 —
/// TS 소스가 바뀌면 `tests/goldenUpdate.test.ts` 가 실패하고, 이 목록과 구현(손 판정)을 함께 다시 본다
pub const HAND_WRITTEN: &[(&str, &str)] = &[
    ("SEMVER_RE", r"/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z.-]+)?$/"),
    ("SAFE_FILE_NAME", r"/^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$/"),
    ("ENCODED_SEPARATOR", r"/%(2e|2f|5c)/i"),
    ("NUMERIC_IDENTIFIER", r"/^\d+$/"),
    ("SANITIZE_URL", r"/https?:\/\/\S+/g"),
    ("SANITIZE_GITHUB_TOKEN", r"/\b(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g"),
    ("SANITIZE_SECRET", r"/(token|authorization|bearer)\s*[:=]?\s*\S+/gi"),
    ("SANITIZE_PATH", r#"/[A-Za-z]:\\[^\s'"]+|\/(?:home|Users|tmp|var)\/[^\s'"]+/g"#),
    ("NOTES_TAG_VERSION", r"/^[0-9A-Za-z.+-]{1,64}$/"),
];

// ── semver ───────────────────────────────────────────────────────────

/// `parseSemver` 의 결과. 숫자는 JS `Number()` 와 같게 f64 다(20자리 넘는 구성 요소는 JS 처럼 정밀도를 잃고, 그 값끼리 비교한다)
#[derive(Clone, Debug, PartialEq)]
pub struct Semver {
    pub major: f64,
    pub minor: f64,
    pub patch: f64,
    pub pre: Vec<String>,
}

fn number(digits: &str) -> Option<f64> {
    // `0|[1-9]\d*` — 앞자리 0 은 받지 않는다
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    f64::from_str(digits).ok()
}

/// `^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z.-]+)?$` 를 앞뒤 공백(JS `trim`)을 지운 문자열에 적용.
/// 64 UTF-16 단위를 넘으면 `None`. 구성 요소가 `u64` 를 넘어도 받는다(f64)
pub fn parse_semver(v: &str) -> Option<Semver> {
    if utf16_len(v) > 64 {
        return None;
    }
    let t = js_trim(v);
    let t = t.strip_prefix('v').unwrap_or(t);
    let (core_pre, build) = match t.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (t, None),
    };
    if let Some(b) = build {
        if b.is_empty() || !b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-') {
            return None;
        }
    }
    let (core, pre) = match core_pre.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (core_pre, None),
    };
    let mut parts = core.split('.');
    let (major, minor, patch) = (number(parts.next()?)?, number(parts.next()?)?, number(parts.next()?)?);
    if parts.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(p) => {
            let ids: Vec<&str> = p.split('.').collect();
            if ids.iter().any(|i| i.is_empty() || !i.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')) {
                return None;
            }
            ids.into_iter().map(str::to_string).collect()
        }
    };
    Some(Semver { major, minor, patch, pre })
}

/// a<b → `Less`, a==b → `Equal`, a>b → `Greater`(semver 2.0 우선순위 규칙). 해석 불가면 `None`.
/// TS 와 같은 점: 앞자리 0 이 있는 숫자 식별자(`01` 대 `1`)는 `Number()` 로 같지만 문자열이 달라 **양방향 모두 `Greater`** 다(원본의 동작을 그대로 둔다)
pub fn compare_semver(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let x = parse_semver(a)?;
    let y = parse_semver(b)?;
    for (p, q) in [(x.major, y.major), (x.minor, y.minor), (x.patch, y.patch)] {
        if p != q {
            return Some(if p < q { Less } else { Greater });
        }
    }
    if x.pre.is_empty() || y.pre.is_empty() {
        return Some(match (x.pre.is_empty(), y.pre.is_empty()) {
            (true, true) => Equal,
            (false, _) => Less,
            _ => Greater,
        });
    }
    for i in 0..x.pre.len().max(y.pre.len()) {
        let (Some(p), Some(q)) = (x.pre.get(i), y.pre.get(i)) else {
            return Some(if x.pre.get(i).is_none() { Less } else { Greater });
        };
        if p == q {
            continue;
        }
        let (pn, qn) = (p.bytes().all(|c| c.is_ascii_digit()), q.bytes().all(|c| c.is_ascii_digit()));
        if pn && qn {
            let (pv, qv) = (f64::from_str(p).unwrap_or(f64::NAN), f64::from_str(q).unwrap_or(f64::NAN));
            return Some(if pv < qv { Less } else { Greater });
        }
        if pn != qn {
            return Some(if pn { Less } else { Greater });
        }
        // 식별자는 ASCII 라 바이트 순서가 JS 의 UTF-16 순서와 같다
        return Some(if p < q { Less } else { Greater });
    }
    Some(Equal)
}

pub fn is_prerelease(v: &str) -> bool {
    parse_semver(v).is_some_and(|s| !s.pre.is_empty())
}

/// candidate 가 알림 대상인가: current 보다 새 버전이고, 프리릴리스는 current 도 프리릴리스일 때만. 다운그레이드·같은 버전은 거부한다
pub fn is_offerable(current: &str, candidate: &str) -> bool {
    if compare_semver(candidate, current) != Some(std::cmp::Ordering::Greater) {
        return false;
    }
    !is_prerelease(candidate) || is_prerelease(current)
}

/// 앞의 `v` 를 한 번 뗀다(`replace(/^v/, '')`)
pub fn strip_v(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
}

/// 공백·제어 문자가 없고 semver 로 해석되는 버전(앞의 `v` 는 뗀다). 매니페스트·태그처럼 **그대로 URL 에 들어가는** 값에 쓴다
fn clean_version(raw: &str) -> Option<&str> {
    if raw.chars().any(|c| is_js_space(c) || c.is_control()) {
        return None;
    }
    let v = strip_v(raw);
    parse_semver(v).map(|_| v)
}

// ── URL 허용 목록 ────────────────────────────────────────────────────

const DOWNLOAD_HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// 내려받기 요청(리다이렉트 포함)에 허용하는 URL: https + 허용 호스트 + 기본 포트 + 자격 증명 없음
pub fn is_allowed_download_url(raw: &str) -> bool {
    let Some(u) = parse_url(raw) else { return false };
    u.scheme() == "https" && u.host_str().is_some_and(|h| DOWNLOAD_HOSTS.contains(&h)) && u.port().is_none() && !has_credentials(&u)
}

/// 첫 요청은 이 저장소의 릴리스 자산 URL 이어야 한다. URL 을 정규화한 뒤 접두사를 비교하고(`..` 로 다른 저장소 경로로 빠지는 것 방지),
/// 인코딩된 점·슬래시·백슬래시와 `..`·`.` 세그먼트·쿼리·해시는 거부한다. 정규화한 결과가 원문과 같아야 한다
pub fn is_release_asset_url(raw: &str) -> bool {
    if !is_allowed_download_url(raw) {
        return false;
    }
    if has_encoded_separator(raw) || raw.contains('\\') {
        return false;
    }
    let Some(u) = parse_url(raw) else { return false };
    if has_search(&u) || has_hash(&u) || u.path().split('/').any(|s| s == ".." || s == ".") {
        return false;
    }
    u.as_str().starts_with(RELEASE_DOWNLOAD_PREFIX) && u.as_str() == raw
}

/// 릴리스 노트 페이지 URL. 태그는 v + 버전
pub fn release_notes_url(version: &str) -> String {
    format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/tag/v{}", strip_v(version))
}

/// 자산 파일 이름으로 쓸 수 있는 안전한 이름인가(경로 구분자·제어 문자 없음): `^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$` 이고 `..` 없음
pub fn is_safe_file_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b.len() <= 201 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) && !n.contains("..")
}

// ── 매니페스트(latest.json) ──────────────────────────────────────────

/// [`check_manifest`] 가 통과시킨 업데이트. `url`·`signature` 는 검사를 마친 값이라 그대로 내려받기·서명 검증(PR 4)에 넘긴다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestOffer {
    /// 앞의 `v` 를 뗀 버전
    pub version: String,
    pub url: String,
    /// minisign `.sig` 파일 내용의 base64(`[A-Za-z0-9+/=]`)
    pub signature: String,
    /// 고른 플랫폼 키
    pub platform: &'static str,
}

/// Tauri `latest.json` 을 검사한다. `current` 는 지금 실행 중인 버전.
///
/// - `Ok(None)`: 새 버전이 아니다(같거나 낮음 — **다운그레이드는 여기서 걸러진다**) 또는 프리릴리스인데 `current` 가 정식이다
/// - `Err`: 본문이 너무 크거나 JSON·필드가 올바르지 않다. 새 버전일 때만 플랫폼 항목을 검사한다
///
/// 플랫폼 항목은 `windows-x86_64-nsis` 를 먼저 보고, **키 자체가 없을 때만** `windows-x86_64` 로 내려간다(있는데 틀린 항목은 오류다).
/// `url` 은 [`is_release_asset_url`] 을 통과하고, **버전의 태그 아래**(`…/releases/download/v<버전>/<파일>`)여야 한다 — 새 버전 번호에 다른 릴리스의 설치본을 붙이는 매니페스트를 거부한다.
/// 서명은 형식만 본다(실제 검증은 설치 직전 minisign). 매니페스트의 `notes` 등 다른 필드는 쓰지 않는다
pub fn check_manifest(text: &str, current: &str) -> Result<Option<ManifestOffer>, String> {
    if text.len() > MAX_MANIFEST_BYTES {
        return Err("응답이 너무 큽니다".to_string());
    }
    let root = json::parse(text).map_err(|_| GENERIC_ERROR.to_string())?;
    if !matches!(root, Json::Obj(_)) {
        return Err(GENERIC_ERROR.to_string());
    }
    let version = root.get("version").and_then(Json::as_str).and_then(clean_version).ok_or_else(|| GENERIC_ERROR.to_string())?;
    if !is_offerable(current, version) {
        return Ok(None);
    }
    let platforms = root.get("platforms").filter(|p| matches!(p, Json::Obj(_))).ok_or_else(|| GENERIC_ERROR.to_string())?;
    let (platform, entry) = [PLATFORM_KEY_NSIS, PLATFORM_KEY_GENERIC]
        .into_iter()
        .find_map(|k| platforms.get(k).map(|e| (k, e)))
        .ok_or_else(|| "이 환경에 맞는 업데이트가 없습니다".to_string())?;
    let url = entry.get("url").and_then(Json::as_str).ok_or_else(|| GENERIC_ERROR.to_string())?;
    let signature = entry.get("signature").and_then(Json::as_str).ok_or_else(|| GENERIC_ERROR.to_string())?;
    if signature.is_empty() || signature.len() > MAX_SIGNATURE_LEN || !signature.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
        return Err("업데이트 서명 정보가 올바르지 않습니다".to_string());
    }
    let tag_dir = format!("{RELEASE_DOWNLOAD_PREFIX}v{version}/");
    let file_ok = url.strip_prefix(&tag_dir).is_some_and(is_safe_file_name);
    if !is_release_asset_url(url) || !file_ok {
        return Err("업데이트 주소가 올바르지 않습니다".to_string());
    }
    Ok(Some(ManifestOffer { version: version.to_string(), url: url.to_string(), signature: signature.to_string(), platform }))
}

/// 프리릴리스 판이 쓸 매니페스트 URL: 릴리스 목록(`RELEASE_LIST_PATH` 의 응답)에서 첫 비초안 릴리스의 태그로 `…/releases/download/<태그>/latest.json` 을 만든다.
/// 정식 판은 [`STABLE_MANIFEST_URL`] 을 쓴다. `Ok(None)`: 릴리스가 없거나 태그가 `v<semver>` 꼴이 아니거나, 프리릴리스인데 `current` 가 정식이다.
/// 목록이 배열이 아니거나 JSON 이 아니면 `Err`
pub fn prerelease_manifest_url(list_text: &str, current: &str) -> Result<Option<String>, String> {
    if utf16_len(list_text) > MAX_API_BYTES {
        return Err("응답이 너무 큽니다".to_string());
    }
    let Json::Arr(list) = json::parse(list_text).map_err(|_| GENERIC_ERROR.to_string())? else { return Err(GENERIC_ERROR.to_string()) };
    let Some(rel) = list.iter().find(|r| r.get("draft") != Some(&Json::Bool(true))) else { return Ok(None) };
    let Some(tag) = rel.get("tag_name").and_then(Json::as_str) else { return Ok(None) };
    let Some(version) = clean_version(tag) else { return Ok(None) };
    if tag != format!("v{version}") || (rel.get("prerelease") == Some(&Json::Bool(true)) && !is_prerelease(current)) {
        return Ok(None);
    }
    Ok(Some(format!("{RELEASE_DOWNLOAD_PREFIX}v{version}/latest.json")))
}

// ── 나중에 / 건너뛰기 ────────────────────────────────────────────────

/// 자동 알림 억제 정책. snooze 는 메모리에만(앱을 다시 시작하면 풀림), skip 은 설정에 저장된 값을 호출할 때 받아 쓴다
/// (TS 는 생성자에 `() => skipped` 를 넘기지만, 여기서는 설정 읽기가 정책 밖에 있도록 인자로 받는다)
#[derive(Clone, Debug, Default)]
pub struct UpdatePolicy {
    snoozed: HashSet<String>,
}

impl UpdatePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snooze(&mut self, version: &str) {
        self.snoozed.insert(version.to_string());
    }

    /// `manual` = 사용자가 직접 확인(항상 보여 줌). 자동 확인은 나중에·건너뛴 버전을 숨긴다.
    /// 건너뛴 버전과 같거나 더 낮은 버전은 숨기고 더 새 버전이 나오면 다시 알린다. 해석할 수 없는 `skipped` 는 모든 버전을 숨기고, 빈 문자열은 건너뛴 버전이 없는 것이다
    pub fn should_notify(&self, version: &str, manual: bool, skipped: Option<&str>) -> bool {
        if manual {
            return true;
        }
        if self.snoozed.contains(version) {
            return false;
        }
        match skipped.filter(|s| !s.is_empty()) {
            Some(skip) => compare_semver(version, skip) == Some(std::cmp::Ordering::Greater),
            None => true,
        }
    }
}

// ── IPC 입력 검증 ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DismissAction {
    Later,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dismiss {
    pub action: DismissAction,
    /// 앞의 `v` 만 뗀 원문(공백은 그대로 — TS 와 같다. 지금 알리는 버전과 맞지 않으면 서비스가 거부한다)
    pub version: String,
}

/// `update:dismiss` 의 인자. 생략·`null` 은 문자열이 아니라서 거부된다
pub fn parse_dismiss(action: &Value, version: &Value) -> Result<Dismiss, String> {
    let action = match action.as_str() {
        Some("later") => DismissAction::Later,
        Some("skip") => DismissAction::Skip,
        _ => return Err("잘못된 입력입니다".to_string()),
    };
    match version.as_str() {
        Some(v) if parse_semver(v).is_some() => Ok(Dismiss { action, version: strip_v(v).to_string() }),
        _ => Err("잘못된 버전입니다".to_string()),
    }
}

// ── 오류 문구 ────────────────────────────────────────────────────────

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn run_end(chars: &[char], from: usize, ok: impl Fn(char) -> bool) -> usize {
    let mut e = from;
    while e < chars.len() && ok(chars[e]) {
        e += 1;
    }
    e
}

/// `https?:\/\/\S+` → URL 의 origin(풀 수 없으면 `(url)`)
fn replace_urls(chars: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let rest = &chars[i..];
        let prefix = if rest.starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/']) {
            8
        } else if rest.starts_with(&['h', 't', 't', 'p', ':', '/', '/']) {
            7
        } else {
            0
        };
        if prefix > 0 {
            let end = run_end(chars, i + prefix, |c| !is_js_space(c));
            if end > i + prefix {
                let u: String = chars[i..end].iter().collect();
                match parse_url(&u) {
                    Some(parsed) => out.push_str(&js_origin(&parsed)),
                    None => out.push_str("(url)"),
                }
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `\b(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b` → `***`
fn replace_github_tokens(chars: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if i == 0 || !is_word(chars[i - 1]) {
            let rest = &chars[i..];
            let prefix = if rest.len() >= 4 && rest[0] == 'g' && rest[1] == 'h' && matches!(rest[2], 'p' | 'o' | 'u' | 's' | 'r') && rest[3] == '_' {
                4
            } else if rest.starts_with(&['g', 'i', 't', 'h', 'u', 'b', '_', 'p', 'a', 't', '_']) {
                11
            } else {
                0
            };
            if prefix > 0 {
                let end = run_end(chars, i + prefix, is_word);
                if end - (i + prefix) >= 20 {
                    out.push_str("***");
                    i = end;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `(token|authorization|bearer)\s*[:=]?\s*\S+`(대소문자 무시) → `<키워드> ***`. 앞 경계(`\b`)가 없어 `mytoken x` 도 걸린다.
/// 정규식의 되돌림 규칙을 따른다: 키워드 뒤 공백·`:`·`=` 를 지나 `\S+` 가 시작할 자리가 있어야 하고, `token=` 처럼 값이 없으면 `=` 자체가 `\S+` 가 된다
fn replace_secrets(chars: &[char]) -> String {
    const KEYWORDS: [&str; 3] = ["token", "authorization", "bearer"];
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let kw = KEYWORDS.iter().find(|k| {
            let n = k.len();
            i + n <= chars.len() && chars[i..i + n].iter().zip(k.chars()).all(|(c, l)| c.is_ascii() && c.to_ascii_lowercase() == l)
        });
        if let Some(k) = kw {
            let q = i + k.len();
            let w = run_end(chars, q, is_js_space);
            let value_end = if w >= chars.len() {
                None
            } else if matches!(chars[w], ':' | '=') {
                let after = run_end(chars, w + 1, is_js_space);
                // `:` 뒤에 값이 있으면 그 값, 없으면 `:` 자체가 `\S+`
                Some(if after < chars.len() { run_end(chars, after, |c| !is_js_space(c)) } else { run_end(chars, w, |c| !is_js_space(c)) })
            } else {
                Some(run_end(chars, w, |c| !is_js_space(c)))
            };
            if let Some(end) = value_end {
                out.extend(chars[i..q].iter());
                out.push_str(" ***");
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `[A-Za-z]:\\[^\s'"]+|\/(?:home|Users|tmp|var)\/[^\s'"]+` → `(경로)`
fn replace_paths(chars: &[char]) -> String {
    let path_char = |c: char| !is_js_space(c) && c != '\'' && c != '"';
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let rest = &chars[i..];
        let start = if rest.len() >= 3 && rest[0].is_ascii_alphabetic() && rest[1] == ':' && rest[2] == '\\' {
            Some(i + 3)
        } else if rest[0] == '/' {
            ["home/", "Users/", "tmp/", "var/"].iter().find(|d| rest[1..].starts_with(&d.chars().collect::<Vec<_>>())).map(|d| i + 1 + d.len())
        } else {
            None
        };
        if let Some(s) = start {
            let end = run_end(chars, s, path_char);
            if end > s {
                out.push_str("(경로)");
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 사용자에게 보여 줄 오류 문구: URL 은 origin 만, 토큰·로컬 경로는 지우고 첫 줄만 200자(UTF-16)로 자른다.
/// 입력은 **우리 코드가 만든 오류 문구**다 — HTTP 클라이언트 오류의 `Display`(URL·경로가 든다)는 넘기지 않는다.
/// 알려진 차이: 200 자 경계가 서로게이트 쌍을 가르면 JS 는 반쪽을 남기고 Rust 는 `U+FFFD` 로 바꾼다. 풀 수 없는 URL 은 Rust 가 더 많다(`net::url_rules` 가 더 엄격하다)
pub fn sanitize_error(msg: &str) -> String {
    let c: Vec<char> = msg.chars().collect();
    let s = replace_urls(&c);
    let c: Vec<char> = s.chars().collect();
    let s = replace_github_tokens(&c);
    let c: Vec<char> = s.chars().collect();
    let s = replace_secrets(&c);
    let c: Vec<char> = s.chars().collect();
    let s = replace_paths(&c);
    let line = s.split('\n').next().unwrap_or("");
    if utf16_len(line) > 200 {
        format!("{}…", slice_utf16_lossy(line, 200))
    } else {
        line.to_string()
    }
}

// ── 릴리스 노트 ──────────────────────────────────────────────────────

/// `@shared/types` 의 `ReleaseNotes`
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNotes {
    pub version: String,
    pub exact: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub url: String,
}

/// `^[0-9A-Za-z.+-]{1,64}$`
fn is_tag_version(v: &str) -> bool {
    (1..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// GitHub 릴리스 API 응답 한 건의 해석(`getRelease`): 404 → `Ok(None)`, 403·429 → 한도 오류, 그 밖의 실패 → 상태 코드 오류.
/// 본문이 비었거나 2MB(UTF-16 단위)를 넘으면 오류, 객체가 아니면(배열 등) `None`. JSON 문법 오류의 문구는 V8 과 다르다
pub fn parse_release_response(status: u16, body: &str) -> Result<Option<Json>, String> {
    match status {
        404 => return Ok(None),
        403 | 429 => return Err("GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요".to_string()),
        s if !(200..300).contains(&s) => return Err(format!("릴리스 노트를 가져오지 못했습니다 (HTTP {s})")),
        _ => {}
    }
    if body.is_empty() || utf16_len(body) > MAX_API_BYTES {
        return Err("응답이 너무 큽니다".to_string());
    }
    match json::parse(body) {
        Ok(v @ Json::Obj(_)) => Ok(Some(v)),
        Ok(_) => Ok(None),
        Err(_) => Err("릴리스 정보를 읽지 못했습니다".to_string()),
    }
}

/// 현재 버전의 릴리스 노트. 그 태그(`/releases/tags/v<버전>`)의 릴리스가 없거나 초안이면 최신 정식 릴리스(`/releases/latest`)로 대신하고 `exact: false`.
/// 링크(`url`)는 항상 이 저장소의 릴리스 페이지로 만든다 — API 응답의 `html_url` 을 그대로 열지 않는다.
/// `get` 은 API 경로(`/releases/…`)를 받아 [`parse_release_response`] 가 해석한 응답을 돌려준다(HTTP 는 호출하는 쪽의 몫이다)
pub async fn fetch_release_notes<F, Fut>(current: &str, mut get: F) -> Result<ReleaseNotes, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Option<Json>, String>>,
{
    let version = strip_v(current);
    let mut rel = if is_tag_version(version) { get(format!("/releases/tags/v{}", encode_uri_component(version))).await? } else { None };
    let exact = rel.as_ref().is_some_and(|r| r.get("draft") != Some(&Json::Bool(true)));
    if !exact {
        rel = get("/releases/latest".to_string()).await?;
    }
    Ok(map_release_notes(version, rel.as_ref(), exact))
}

/// [`fetch_release_notes`] 의 매핑 부분. `version` 은 앞의 `v` 를 뗀 현재 버전
pub fn map_release_notes(version: &str, rel: Option<&Json>, exact: bool) -> ReleaseNotes {
    let fallback = || ReleaseNotes { version: version.to_string(), exact: false, title: None, body: String::new(), truncated: None, published_at: None, url: RELEASES_PAGE.to_string() };
    let Some(rel) = rel else { return fallback() };
    let Some(tag) = rel.get("tag_name").and_then(Json::as_str) else { return fallback() };
    let tag_version = strip_v(tag);
    let mut body = rel.get("body").and_then(Json::as_str).unwrap_or("").to_string();
    let truncated = utf16_len(&body) > MAX_NOTES_CHARS;
    if truncated {
        body = slice_utf16_lossy(&body, MAX_NOTES_CHARS);
    }
    ReleaseNotes {
        version: tag_version.to_string(),
        exact,
        title: rel.get("name").and_then(Json::as_str).filter(|n| !n.is_empty()).map(|n| slice_utf16_lossy(n, 200)),
        body,
        truncated: truncated.then_some(true),
        published_at: rel.get("published_at").and_then(Json::as_str).filter(|d| parse_date_ms(d).is_some()).map(str::to_string),
        url: if is_tag_version(tag_version) { release_notes_url(tag_version) } else { RELEASES_PAGE.to_string() },
    }
}
