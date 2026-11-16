//! Jira REST 클라이언트(`src/main/jira/client.ts` 이식, PR 3 — 앱에 연결하지 않는다). GET · 상태 전환용 POST · 프로필 사진.
//! 자격 증명은 호출마다 받아 요청 헤더에만 싣는다(반환값·오류 문구·로그에 넣지 않는다). 오류 문구는 마지막에 `redact_secrets` 를 거친다.
//! 리다이렉트는 따라가지 않는다 — 다른 호스트(또는 같은 호스트의 http)로 인증 헤더가 새는 일을 원천 차단한다(3xx → `JIRA_REDIRECT`).
//!
//! TS 와 다른 점: 연결 실패 문구에 원인 문자열을 붙이지 않는다(`reqwest::Error` 의 문구에는 요청 주소가 실릴 수 있다 — 분류별 고정 문구만 쓴다).
//! 취소는 `CancellationToken` 이고 `NetError::Aborted` 다. 본문은 `net::json::parse`(관대한 해석)로 읽는다.
use std::time::Duration;

use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, Response};
use tokio_util::sync::CancellationToken;

use super::{auth_header, JiraKind};
use crate::git::fetch::js_trim;
#[cfg(feature = "test-hooks")]
use crate::net::http::{Profile, Rewrite};
use crate::net::http::{classify, header, now_ms, or_abort, read_capped, Failure, HttpClient, HttpInitError, NetError, ReadError, Redirects, REQUEST_TIMEOUT_SECS};
use crate::net::http_error::{jira_http_error, parse_retry_after};
use crate::net::json::{self, Json};
use crate::net::redact::{jira_secrets, SecretString};
use crate::net::js_value::stringify;
use crate::net::url_rules::{jira_request_url, ImageStep, ImageWalk};

pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
/// 응답이 상한을 넘었을 때의 메시지(호출자가 '이 요청만 건너뛰기'를 가려내는 데도 쓴다)
pub const JIRA_TOO_BIG: &str = "Jira 응답이 너무 큽니다";
/// 429 의 Retry-After 가 이 이하(초)면 한 번만 기다렸다 다시 시도한다
const AUTO_RETRY_MAX_SEC: f64 = 3.0;
/// 프로필 사진 응답 상한과 받는 형식(SVG 는 받지 않는다)
pub const IMAGE_MAX_BYTES: usize = 256 * 1024;
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

#[derive(Clone, Debug)]
pub struct JiraCredential {
    pub kind: JiraKind,
    /// 정규화된 사이트 주소(경로 포함, 끝 `/` 없음 — `validate::parse_site_url` 의 결과)
    pub site_url: String,
    pub email: String,
    pub token: SecretString,
}

#[derive(Clone, Copy, Default)]
pub struct GetOptions<'a> {
    /// 쿼리(값이 `None` 이면 건너뛴다)
    pub query: &'a [(&'a str, Option<&'a str>)],
    /// 404 일 때 보여 줄 문장(엔드포인트별: 이슈 / 검색 / 연결 확인)
    pub not_found: Option<&'a str>,
    /// 응답 본문 상한(기본 5MB). 큰 응답이 정상인 엔드포인트만 올린다
    pub max_bytes: Option<usize>,
    pub cancel: Option<&'a CancellationToken>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JiraImage {
    /// `IMAGE_TYPES` 중 하나
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct JiraClient {
    http: HttpClient,
    /// Retry-After 1초당 실제로 기다리는 시간(운영 1초)
    retry_unit: Duration,
}

impl JiraClient {
    pub fn new() -> Result<Self, HttpInitError> {
        Ok(Self { http: HttpClient::production(Redirects::None)?, retry_unit: Duration::from_secs(1) })
    }

    /// 테스트 전용(루프백 http·짧은 제한 시간·주소 치환·짧은 재시도 대기). 운영 코드에서 부르지 않는다
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn new_for_test(timeout: Duration, rewrite: Option<Rewrite>, retry_unit: Duration) -> Result<Self, HttpInitError> {
        Ok(Self { http: HttpClient::build(Redirects::None, Profile::Test { timeout, rewrite })?, retry_unit })
    }

    /// REST GET. `path` 는 `/rest/api/…` 처럼 `/` 로 시작하는 사이트 상대 경로(세그먼트는 호출자가 인코딩한다)
    pub async fn get(&self, cred: &JiraCredential, path: &str, opts: GetOptions<'_>) -> Result<Json, NetError> {
        self.get_inner(cred, path, opts).await.map_err(|e| e.sealed(&jira_secrets(&cred.email, &cred.token)))
    }

    /// REST POST(JSON 본문). 쓰기이므로 429 자동 재시도를 하지 않는다 — 요청이 이미 적용됐을 수 있어 중복 실행을 피한다.
    /// 2xx 의 빈 본문(204)은 `None`. `X-Atlassian-Token` 은 Data Center 의 XSRF 검사를 통과시킨다
    pub async fn post(&self, cred: &JiraCredential, path: &str, body: &Json, opts: GetOptions<'_>) -> Result<Option<Json>, NetError> {
        self.post_inner(cred, path, body, opts).await.map_err(|e| e.sealed(&jira_secrets(&cred.email, &cred.token)))
    }

    async fn get_inner(&self, cred: &JiraCredential, path: &str, opts: GetOptions<'_>) -> Result<Json, NetError> {
        let url = jira_request_url(&cred.site_url, path, opts.query).ok_or(NetError::InvalidPath)?;
        let limit = opts.max_bytes.unwrap_or(MAX_BODY_BYTES);
        for attempt in 0.. {
            let mut res = self.send(cred, Method::GET, &url, None, opts.cancel).await?;
            let status = res.status().as_u16();
            if status == 429 && attempt == 0 {
                if let Some(wait) = parse_retry_after(header(res.headers(), RETRY_AFTER.as_str()), now_ms()).filter(|w| *w <= AUTO_RETRY_MAX_SEC) {
                    // 본문은 필요 없지만 상한을 지켜 비운다(연결 재사용·메모리). 취소만 전파한다
                    let _ = or_abort(opts.cancel, read_capped(&mut res, limit)).await?;
                    or_abort(opts.cancel, tokio::time::sleep(self.retry_unit.mul_f64(wait))).await?;
                    continue;
                }
            }
            let bytes = read_body(&mut res, limit, opts.cancel, status).await?;
            let text = String::from_utf8_lossy(&bytes);
            if !res.status().is_success() {
                return Err(http_error(&res, status, &text, cred, opts.not_found));
            }
            return json::parse(&text).map_err(|_| NetError::api("JIRA_HTTP", Some(status), "Jira 응답을 해석할 수 없습니다"));
        }
        unreachable!("재시도는 한 번뿐이다")
    }

    async fn post_inner(&self, cred: &JiraCredential, path: &str, body: &Json, opts: GetOptions<'_>) -> Result<Option<Json>, NetError> {
        let url = jira_request_url(&cred.site_url, path, opts.query).ok_or(NetError::InvalidPath)?;
        let mut res = self.send(cred, Method::POST, &url, Some(stringify(body)), opts.cancel).await?;
        let status = res.status().as_u16();
        let bytes = read_body(&mut res, opts.max_bytes.unwrap_or(MAX_BODY_BYTES), opts.cancel, status).await?;
        let text = String::from_utf8_lossy(&bytes);
        if !res.status().is_success() {
            return Err(http_error(&res, status, &text, cred, opts.not_found));
        }
        if js_trim(&text).is_empty() {
            return Ok(None);
        }
        json::parse(&text).map(Some).map_err(|_| NetError::api("JIRA_HTTP", Some(status), "Jira 응답을 해석할 수 없습니다"))
    }

    /// 프로필 사진(avatarUrls) 한 장. 규칙은 `net::url_rules::ImageWalk` 가 정한다: 연결한 사이트 안의 주소(첫 홉)에만 인증을 붙이고 리다이렉트를 따라가지 않으며(Data Center 는 인증해야 사진을 준다),
    /// 사이트 밖은 Cloud 의 공개 사진 호스트만 https·인증 없이 받고 그 호스트들 사이의 리다이렉트만 따라간다(gravatar 의 기본 사진).
    /// 받을 수 없거나 허용한 이미지 형식이 아니면 `Ok(None)` — 취소만 `Err(Aborted)`
    pub async fn image(&self, cred: &JiraCredential, href: &str, cancel: Option<&CancellationToken>) -> Result<Option<JiraImage>, NetError> {
        let mut walk = ImageWalk::new(&cred.site_url, href);
        while let ImageStep::Fetch { url, with_auth } = walk.step() {
            let mut req = self.http.request(Method::GET, url.as_str()).header(ACCEPT, "image/*");
            if with_auth {
                // 인증은 사이트 안 첫 홉에만 — 리다이렉트로 온 주소에는 사이트 안이라도 붙이지 않는다(ImageWalk 가 보장한다)
                let Some(auth) = auth_value(cred) else { return Ok(None) };
                req = req.header(AUTHORIZATION, auth);
            }
            let Ok(mut res) = or_abort(cancel, req.send()).await? else { return Ok(None) };
            if res.status().is_redirection() {
                let loc = header(res.headers(), LOCATION.as_str()).map(str::to_owned);
                walk.on_redirect(loc.as_deref());
                continue;
            }
            let content_type = header(res.headers(), CONTENT_TYPE.as_str()).unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            if !res.status().is_success() || !IMAGE_TYPES.contains(&content_type.as_str()) {
                return Ok(None);
            }
            return Ok(match or_abort(cancel, read_capped(&mut res, IMAGE_MAX_BYTES)).await? {
                Ok(bytes) if !bytes.is_empty() => Some(JiraImage { content_type, bytes }),
                _ => None,
            });
        }
        Ok(None)
    }

    /// 요청을 보내 헤더를 받는다. 3xx 는 따라가지 않고 `JIRA_REDIRECT`
    async fn send(&self, cred: &JiraCredential, method: Method, url: &str, body: Option<String>, cancel: Option<&CancellationToken>) -> Result<Response, NetError> {
        let Some(auth) = auth_value(cred) else { return Err(connect_failed()) };
        let mut req: RequestBuilder = self.http.request(method, url).header(ACCEPT, "application/json").header(AUTHORIZATION, auth);
        if let Some(body) = body {
            req = req.header(CONTENT_TYPE, "application/json").header("X-Atlassian-Token", "no-check").body(body);
        }
        let res = match or_abort(cancel, req.send()).await? {
            Ok(r) => r,
            Err(e) => {
                return Err(match classify(&e) {
                    Failure::Timeout => timed_out(),
                    _ => connect_failed(),
                })
            }
        };
        if res.status().is_redirection() {
            return Err(NetError::api("JIRA_REDIRECT", Some(res.status().as_u16()), "Jira 가 다른 주소로 이동시키려 해 요청을 중단했습니다(사이트 주소를 확인하세요)"));
        }
        Ok(res)
    }
}

/// `Authorization` 값(민감 표시). 헤더에 쓸 수 없는 문자가 들어 있으면 `None`
fn auth_value(cred: &JiraCredential) -> Option<HeaderValue> {
    let mut v = HeaderValue::from_str(auth_header(cred.kind, &cred.email, &cred.token).expose()).ok()?;
    v.set_sensitive(true);
    Some(v)
}

fn timed_out() -> NetError {
    NetError::api("JIRA_TIMEOUT", None, format!("Jira 응답이 {REQUEST_TIMEOUT_SECS}초 안에 오지 않았습니다"))
}

fn connect_failed() -> NetError {
    NetError::api("JIRA_NETWORK", None, "Jira 에 연결할 수 없습니다")
}

async fn read_body(res: &mut Response, limit: usize, cancel: Option<&CancellationToken>, status: u16) -> Result<Vec<u8>, NetError> {
    match or_abort(cancel, read_capped(res, limit)).await? {
        Ok(bytes) => Ok(bytes),
        Err(ReadError::TooBig) => Err(NetError::api("JIRA_HTTP", Some(status), JIRA_TOO_BIG)),
        Err(ReadError::Failed(Failure::Timeout)) => Err(timed_out()),
        Err(ReadError::Failed(_)) => Err(NetError::api("JIRA_NETWORK", Some(status), "Jira 응답을 읽는 중 연결이 끊겼습니다")),
    }
}

/// 본문 메시지의 비밀은 자르기 전에 여기서 가린다(`get`·`post` 의 `sealed` 는 마지막 방어)
fn http_error(res: &Response, status: u16, text: &str, cred: &JiraCredential, not_found: Option<&str>) -> NetError {
    let secrets = jira_secrets(&cred.email, &cred.token);
    let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let e = jira_http_error(status, header(res.headers(), RETRY_AFTER.as_str()), text, cred.kind, not_found, now_ms(), &refs);
    NetError::api(e.code, Some(e.status), e.message)
}
