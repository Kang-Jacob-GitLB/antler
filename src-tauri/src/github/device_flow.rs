//! GitHub OAuth Device Flow 의 요청 쪽(`src/main/github/deviceFlow.ts` 이식, Phase 4 PR 7). 응답을 보고 다음에 무엇을 할지는 [`super::device_rules`](PR 1)가 정하고,
//! 여기는 요청을 보내고(리다이렉트를 따르지 않는다 — 3xx 는 해석할 수 없는 본문이라 일시 오류다) 대기·취소·만료 시각을 다룬다.
//!
//! - 대기(`Sleeper`)와 시각(`Clock`)은 주입한다. 취소는 `CancellationToken` 이고 대기·요청 어느 쪽에서든 곧바로 `ABORTED` 다.
//! - 본문은 1MB 까지만 읽는다(TS 는 상한이 없다). 상한을 넘거나 읽다 끊기면 해석할 수 없는 응답이라 일시 오류로 센다.
//! - `device_code` 는 이 모듈과 서비스 안에만 있다(`SecretString`). renderer 로 가는 값은 [`GhDeviceCode`]뿐이고 오류 문구는 `device_code` 로 가려진다.
//!
//! TS 와 다른 점: 간격·만료의 상한은 [`super::device_rules`] 에 적은 대로다. 3xx 응답은 따라가지 않고 일시 오류다(TS `fetch` 는 따라간다).
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Method;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::device_rules::{interpret_start, DeviceError, PollState, PollStep, EXPIRED_MESSAGE, MISSING_CLIENT_ID_MESSAGE};
use super::error::{GhError, Res};
#[cfg(feature = "test-hooks")]
use crate::net::http::{Profile, Rewrite};
use crate::net::http::{classify, or_abort, read_capped, Failure, HttpClient, HttpInitError, Redirects};
use crate::jira::service::{Clock, Sleeper};
use crate::net::redact::SecretString;

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// 시험이 요청 주소를 바꿀 때 떼어 내는 접두사(운영 빌드에서는 쓰지 않는다)
#[cfg(feature = "test-hooks")]
const ORIGIN: &str = "https://github.com";
const SCOPE: &str = "repo read:org";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// 응답 본문 상한(TS 에는 없다)
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// 폴링에 필요한 서버 쪽 값. `device_code` 는 main(Rust)에만 둔다(renderer 에는 [`GhDeviceCode`]만 넘긴다)
#[derive(Clone, Debug)]
pub struct DeviceSession {
    pub device_code: SecretString,
    /// 초
    pub interval: f64,
    /// epoch ms
    pub expires_at_ms: f64,
}

/// `gh:deviceStart` 의 결과 — 사용자가 브라우저에서 `user_code` 를 입력한다(`device_code` 는 없다)
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhDeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    /// 만료까지 초
    #[serde(serialize_with = "ser_num")]
    pub expires_in: f64,
    /// 폴링 간격(초)
    #[serde(serialize_with = "ser_num")]
    pub interval: f64,
}

/// 정수로 나갈 수 있는 값은 정수로(`900` 이지 `900.0` 이 아니게)
pub(crate) fn ser_num<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.fract() == 0.0 && v.is_finite() && v.abs() < 9.0e15 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

/// 요청 하나가 실패한 이유
enum PostFail {
    /// 재시도할 만한 일시 오류(네트워크·시간 초과·본문을 못 읽음) — 문구는 시작 실패 때 사용자에게 보인다
    Transient(&'static str),
    Aborted,
}

pub struct DeviceFlow {
    http: HttpClient,
    sleeper: Arc<dyn Sleeper>,
    clock: Arc<dyn Clock>,
}

impl DeviceFlow {
    pub fn new(sleeper: Arc<dyn Sleeper>, clock: Arc<dyn Clock>) -> Result<Self, HttpInitError> {
        Ok(Self { http: HttpClient::production(Redirects::None)?, sleeper, clock })
    }

    /// 테스트 전용: `https://github.com` 으로 가는 요청을 `mock_base`(`http://127.0.0.1:포트`)로 보낸다. 루프백 http 를 허용하고 제한 시간을 줄인다. **운영 코드에서 부르지 않는다**
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn new_for_test(mock_base: &str, timeout: Duration, sleeper: Arc<dyn Sleeper>, clock: Arc<dyn Clock>) -> Result<Self, HttpInitError> {
        let to = mock_base.trim_end_matches('/').to_string();
        let rewrite: Rewrite = Arc::new(move |url: &str| match url.strip_prefix(ORIGIN) {
            Some(rest) => format!("{to}{rest}"),
            None => url.to_string(),
        });
        let http = HttpClient::build(Redirects::None, Profile::Test { timeout, rewrite: Some(rewrite) })?;
        Ok(Self { http, sleeper, clock })
    }

    /// 사용자 코드를 발급받는다
    pub async fn start(&self, client_id: &str, cancel: &CancellationToken) -> Res<(GhDeviceCode, DeviceSession)> {
        if client_id.is_empty() {
            return Err(GhError::plain(MISSING_CLIENT_ID_MESSAGE));
        }
        let payload = body_json(&StartBody { client_id, scope: SCOPE });
        let (status, body) = match self.post(DEVICE_CODE_URL, &payload, cancel).await {
            Ok(v) => v,
            Err(PostFail::Aborted) => return Err(GhError::aborted()),
            Err(PostFail::Transient(m)) => return Err(GhError::plain(m)),
        };
        let d = interpret_start(client_id, status, &body).map_err(device_error)?;
        let session = DeviceSession { device_code: d.device_code.clone(), interval: d.interval, expires_at_ms: self.clock.now_ms() + d.expires_in * 1000.0 };
        let code = GhDeviceCode { user_code: d.user_code, verification_uri: d.verification_uri, expires_in: d.expires_in, interval: d.interval };
        Ok((code, session))
    }

    /// 승인될 때까지 폴링해 access token 을 돌려준다. 간격을 지키고(첫 요청도 interval 뒤), authorization_pending 은 계속, slow_down 은 간격 +5초,
    /// 만료·거부는 한국어 오류로 끝낸다. 일시 오류는 세션이 만료될 때까지 조금씩 늘려 가며 다시 시도한다
    pub async fn poll(&self, client_id: &str, session: &DeviceSession, cancel: &CancellationToken) -> Res<SecretString> {
        let secrets = Zeroizing::new(vec![session.device_code.expose().to_string()]);
        self.poll_inner(client_id, session, cancel).await.map_err(|e| e.sealed(&secrets))
    }

    async fn poll_inner(&self, client_id: &str, session: &DeviceSession, cancel: &CancellationToken) -> Res<SecretString> {
        let mut state = PollState::new(session.interval);
        loop {
            let wait = Duration::from_millis(state.delay_ms() as u64);
            or_abort(Some(cancel), self.sleeper.sleep(wait)).await.map_err(|_| GhError::aborted())?;
            if PollState::expired(self.clock.now_ms(), session.expires_at_ms) {
                return Err(GhError::plain(EXPIRED_MESSAGE));
            }
            let payload = body_json(&PollBody { client_id, device_code: session.device_code.expose(), grant_type: GRANT_TYPE });
            let step = match self.post(ACCESS_TOKEN_URL, &payload, cancel).await {
                Ok((status, body)) => state.on_response(status, &body),
                Err(PostFail::Transient(_)) => state.on_transient(),
                Err(PostFail::Aborted) => return Err(GhError::aborted()),
            };
            match step {
                PollStep::Pending | PollStep::SlowDown | PollStep::Transient => {}
                PollStep::Token(t) => return Ok(t),
                PollStep::Fail(e) => return Err(device_error(e)),
            }
        }
    }

    /// POST(JSON). 오류(`error` 필드)도 200 으로 오므로 상태 코드가 아니라 본문으로 판단한다 — 5xx 는 본문을 읽지 않는다
    async fn post(&self, url: &str, payload: &Zeroizing<String>, cancel: &CancellationToken) -> Result<(u16, Zeroizing<String>), PostFail> {
        let req = self.http.request(Method::POST, url).header(ACCEPT, "application/json").header(CONTENT_TYPE, "application/json").body(payload.as_str().to_owned());
        let mut res = match or_abort(Some(cancel), req.send()).await {
            Err(_) => return Err(PostFail::Aborted),
            Ok(Err(e)) => return Err(PostFail::Transient(if classify(&e) == Failure::Timeout { "GitHub 응답이 없습니다(시간 초과)" } else { "GitHub 에 연결할 수 없습니다" })),
            Ok(Ok(r)) => r,
        };
        let status = res.status().as_u16();
        if status >= 500 {
            return Ok((status, Zeroizing::new(String::new())));
        }
        match or_abort(Some(cancel), read_capped(&mut res, MAX_BODY_BYTES)).await {
            Err(_) => Err(PostFail::Aborted),
            // 성공 응답에는 access token 이 들어 있다 — 바이트·문자열 모두 버릴 때 덮는다
            Ok(Ok(bytes)) => {
                let bytes = Zeroizing::new(bytes);
                Ok((status, Zeroizing::new(String::from_utf8_lossy(&bytes).into_owned())))
            }
            Ok(Err(_)) => Err(PostFail::Transient("GitHub 응답을 해석할 수 없습니다")),
        }
    }
}

/// 요청 본문(키는 `json!` 의 기본 정렬과 같은 알파벳순)
#[derive(Serialize)]
struct StartBody<'a> {
    client_id: &'a str,
    scope: &'a str,
}

#[derive(Serialize)]
struct PollBody<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

/// 본문 문자열. `device_code` 가 들어 있어 버릴 때 덮는다(`serde_json::Value` 를 거치지 않아 중간 복사본이 없다)
fn body_json(b: &impl Serialize) -> Zeroizing<String> {
    Zeroizing::new(serde_json::to_string(b).unwrap_or_default())
}

fn device_error(e: DeviceError) -> GhError {
    match e {
        DeviceError::Message(m) => GhError::plain(m),
        DeviceError::TypeError => GhError::bad_response(),
    }
}
