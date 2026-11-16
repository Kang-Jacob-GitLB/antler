//! HTTP 코어(PR 3): reqwest 클라이언트를 **이 파일에서만** 만든다(clippy.toml 이 `Client::new`·`Client::builder`·`reqwest::get`·`danger_accept_*` 를 막고, 소스 검사 테스트가
//! 다른 파일의 `reqwest::Client` 생성을 막는다). 운영 설정은 한 곳에 모여 있다: https 만, 프록시 환경 변수 무시, 요청당 15초(본문까지), User-Agent `Antler`.
//!
//! - 리다이렉트: [`Redirects::None`](Jira·Device Flow — 3xx 는 호출자가 오류로 처리), [`Redirects::SameOrigin`](GitHub API — origin 이 정확히 같고 홉 수 이하일 때만 따라간다).
//! - 본문은 [`read_capped`] 로만 읽는다(Content-Length 선검사 + chunk 누계). 취소는 [`or_abort`].
//! - `reqwest::Error` 의 `Display` 는 쓰지 않는다. 요청 주소·헤더가 문구에 실릴 수 있어서다 — [`classify`] 가 `is_timeout`·`is_connect`·`is_body` 로 분류만 하고 문구는 호출자의 고정 문구다.
//!
//! 테스트 전용 주입(`*_for_test`, `#[doc(hidden)]`)은 루프백 http 허용·짧은 제한 시간·요청 주소 치환뿐이다. 운영 코드에서 부르지 않는다는 것을
//! `tests/net.rs` 의 소스 검사가 강제한다. 주소 치환은 "https://secure.gravatar.com/…" 같은 **판정은 원래 주소로 하고 연결만 루프백으로** 보내기 위한 것이다(실제 인터넷에 나가지 않고 경로를 시험한다).
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use reqwest::{Client, Method, RequestBuilder, Response};
use tokio_util::sync::CancellationToken;

use super::redact::redact_secrets;
use super::url_rules::js_origin;

/// 요청 하나(연결·헤더·본문 끝까지)의 제한 시간
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 오류 문구에 쓰는 제한 시간(초). 테스트가 제한 시간을 줄여도 사용자 문구는 운영 값이다
pub const REQUEST_TIMEOUT_SECS: u64 = 15;
pub const USER_AGENT: &str = "Antler";

/// 리다이렉트 정책
#[derive(Clone, Debug)]
pub enum Redirects {
    /// 따르지 않는다(3xx 응답이 그대로 돌아온다)
    None,
    /// origin(`scheme://host[:port]`)이 정확히 `origin` 인 주소로만, 최대 `max_hops` 번 따라간다. 넘거나 다른 origin 이면 3xx 응답이 그대로 돌아온다
    SameOrigin { origin: String, max_hops: usize },
}

/// 클라이언트를 만들 수 없음(TLS 초기화 실패 등). 원인 문구는 싣지 않는다
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpInitError;

impl std::fmt::Display for HttpInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP 클라이언트를 만들 수 없습니다")
    }
}

impl std::error::Error for HttpInitError {}

/// 테스트가 요청 주소를 바꾸는 함수(원래 주소 → 실제로 연결할 주소)
pub type Rewrite = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// 클라이언트를 만드는 방식. [`Profile::Test`] 는 `*_for_test` 생성자만 쓴다
pub enum Profile {
    Production,
    Test { timeout: Duration, rewrite: Option<Rewrite> },
}

/// reqwest 클라이언트 한 개와 요청 만들기. 복제는 값싸다(내부 `Arc`)
#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    rewrite: Option<Rewrite>,
}

impl HttpClient {
    /// 운영 클라이언트: https 만, 프록시 무시, 15초, UA `Antler`
    pub fn production(redirects: Redirects) -> Result<Self, HttpInitError> {
        Self::build(redirects, Profile::Production)
    }

    /// 테스트 전용: 루프백 http 를 허용하고 제한 시간을 줄이며 요청 주소를 바꿀 수 있다. **운영 코드에서 부르지 않는다**
    #[doc(hidden)]
    pub fn new_for_test(redirects: Redirects, timeout: Duration, rewrite: Option<Rewrite>) -> Result<Self, HttpInitError> {
        Self::build(redirects, Profile::Test { timeout, rewrite })
    }

    // reqwest 클라이언트를 만드는 유일한 곳이다(clippy.toml 의 disallowed-methods 예외)
    #[allow(clippy::disallowed_methods)]
    pub(crate) fn build(redirects: Redirects, profile: Profile) -> Result<Self, HttpInitError> {
        let (https_only, timeout, rewrite) = match profile {
            Profile::Production => (true, REQUEST_TIMEOUT, None),
            Profile::Test { timeout, rewrite } => (false, timeout, rewrite),
        };
        let policy = match redirects {
            Redirects::None => Policy::none(),
            Redirects::SameOrigin { origin, max_hops } => Policy::custom(move |attempt| {
                // `previous()` 는 지금까지 요청한 주소들이라 N 번째 리다이렉트를 따를지 정할 때 길이가 N 이다
                if attempt.previous().len() > max_hops || js_origin(attempt.url()) != origin {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }),
        };
        let client = Client::builder()
            .tls_backend_native()
            .https_only(https_only)
            .no_proxy()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .redirect(policy)
            .build()
            .map_err(|_| HttpInitError)?;
        Ok(Self { client, rewrite })
    }

    /// 요청 만들기. 주소가 잘못돼도 여기서는 실패하지 않고 `send` 에서 실패한다
    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        match &self.rewrite {
            Some(f) => self.client.request(method, f(url)),
            None => self.client.request(method, url),
        }
    }
}

// ── 오류 분류 ─────────────────────────────────────────────────────────────

/// `reqwest::Error` 를 문구 없이 분류한 것
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// 제한 시간 초과(연결·응답·본문 어디서든)
    Timeout,
    /// 연결·TLS 핸드셰이크·요청 전송 실패(응답 전)
    Connect,
    /// 응답 본문을 읽다가 끊김
    Body,
}

/// 시간 초과를 먼저 본다 — 본문 도중의 시간 초과는 `is_body` 와 `is_timeout` 이 함께 참일 수 있다
pub fn classify(e: &reqwest::Error) -> Failure {
    if e.is_timeout() {
        Failure::Timeout
    } else if e.is_connect() {
        Failure::Connect
    } else if e.is_body() || e.is_decode() {
        Failure::Body
    } else {
        Failure::Connect
    }
}

// ── 취소 ──────────────────────────────────────────────────────────────────

/// 호출자가 요청을 취소했다(TS `AbortError`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aborted;

/// `fut` 를 실행하되 `cancel` 이 취소되면 즉시 버리고(연결도 닫힌다) `Aborted`. 이미 취소돼 있으면 `fut` 를 시작하지 않는다
pub async fn or_abort<T>(cancel: Option<&CancellationToken>, fut: impl Future<Output = T>) -> Result<T, Aborted> {
    let Some(cancel) = cancel else { return Ok(fut.await) };
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(Aborted),
        v = fut => Ok(v),
    }
}

// ── 본문 읽기 ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// 상한을 넘었다(더 읽지 않고 연결을 닫는다)
    TooBig,
    Failed(Failure),
}

/// 본문을 읽으며 바이트를 센다. `Content-Length` 가 상한을 넘으면 읽지 않고, chunked 처럼 길이를 모르면 누계가 상한을 넘는 순간 중단한다 — 거대 응답을 메모리에 다 받지 않는다
pub async fn read_capped(res: &mut Response, limit: usize) -> Result<Vec<u8>, ReadError> {
    if res.content_length().is_some_and(|n| n > limit as u64) {
        return Err(ReadError::TooBig);
    }
    let mut out: Vec<u8> = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                if out.len() + chunk.len() > limit {
                    return Err(ReadError::TooBig);
                }
                out.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(out),
            Err(e) => return Err(ReadError::Failed(classify(&e))),
        }
    }
}

/// 응답 헤더 한 개(UTF-8 문자열로 읽히는 것만)
pub fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// 지금 시각(epoch ms) — `Date.now()`
pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

// ── 오류 봉투 ─────────────────────────────────────────────────────────────

/// 클라이언트가 돌려주는 오류. `Api` 의 문구는 사용자에게 그대로 보여 줘도 되는 한국어 문장이며, 만들어진 뒤 [`NetError::sealed`] 로 비밀이 가려진다
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    Api { code: &'static str, status: Option<u16>, message: String },
    /// TS 의 `Error('invalid api path')` — 호출자(내부)의 실수
    InvalidPath,
    /// 호출자가 취소했다
    Aborted,
}

impl NetError {
    pub fn api(code: &'static str, status: Option<u16>, message: impl Into<String>) -> Self {
        Self::Api { code, status, message: message.into() }
    }

    /// 오류 문구에서 비밀을 가린다(마지막 단계 — 서버 본문 메시지가 토큰을 되돌려 줘도 새지 않게)
    #[must_use]
    pub fn sealed(self, secrets: &[String]) -> Self {
        match self {
            Self::Api { code, status, message } => {
                let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
                Self::Api { code, status, message: redact_secrets(&message, &refs) }
            }
            other => other,
        }
    }
}

impl From<Aborted> for NetError {
    fn from(_: Aborted) -> Self {
        Self::Aborted
    }
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api { message, .. } => f.write_str(message),
            Self::InvalidPath => f.write_str("invalid api path"),
            Self::Aborted => f.write_str("aborted"),
        }
    }
}

impl std::error::Error for NetError {}
