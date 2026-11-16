//! GitHub OAuth Device Flow 의 응답 해석(`src/main/github/deviceFlow.ts` 의 분기 이식). 요청을 보내는 쪽(reqwest)과 대기(sleep)는 뒤 PR 의 몫이고,
//! 여기는 "응답 한 건을 받아 다음에 무엇을 할지"만 정한다 — 간격·백오프·slow_down·오류 문구가 TS 와 같다(골든 `device.json`).
//!
//! TS 와 다른 곳(Rust 가 더 엄격하다): 필드 타입이 틀린 응답(문자열이 아닌 `access_token`·`device_code`, 숫자가 아닌 `interval`·`expires_in`)을 TS 는 JS 의 truthiness·형 변환으로
//! 받아들이기도 하지만 Rust 는 없는 것으로 본다(`device_code` 등이 없으면 시작 실패, `interval`·`expires_in` 은 기본값). GitHub 가 보내는 형식이 아니다.
use crate::net::json::{self, Json};
use crate::net::redact::SecretString;

pub const MISSING_CLIENT_ID_MESSAGE: &str = "GitHub OAuth Client ID 가 설정되지 않았습니다. GitHub 설정(Developer settings → OAuth Apps)에서 OAuth App 을 만들고 \"Enable Device Flow\" 를 켠 뒤 Client ID 를 설정에 입력하세요. 또는 개인 액세스 토큰(PAT)으로 로그인할 수 있습니다.";
pub const EXPIRED_MESSAGE: &str = "인증 시간이 만료되었습니다. 다시 시도하세요";
/// 일시 오류 뒤 다음 시도까지 간격에 더하는 대기(회당 늘리고 상한을 둔다)
pub const TRANSIENT_BACKOFF_STEP_MS: f64 = 2000.0;
pub const TRANSIENT_BACKOFF_MAX_MS: f64 = 20_000.0;
/// slow_down 응답을 받으면 간격에 더하는 초
pub const SLOW_DOWN_STEP_S: f64 = 5.0;

/// 로그인 실패. `Message` 는 사용자에게 보여 줄 한국어 문장, `TypeError` 는 TS 에서 `null` 본문에 속성을 읽다 던지는 `TypeError` 다(해석할 수 없는 응답)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceError {
    Message(String),
    TypeError,
}

fn msg<T>(m: impl Into<String>) -> Result<T, DeviceError> {
    Err(DeviceError::Message(m.into()))
}

/// 사용자 코드 발급 응답에서 얻는 값. `device_code` 는 main(Rust)에만 둔다
#[derive(Clone, Debug)]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: f64,
    /// 초(최소 1)
    pub interval: f64,
    pub device_code: SecretString,
}

/// 비어 있지 않은 문자열 필드
fn text<'a>(b: &'a Json, key: &str) -> Option<&'a str> {
    b.get(key).and_then(Json::as_str).filter(|s| !s.is_empty())
}

/// `x ?? default`: 숫자면 그 값, 없음·null·그 밖은 기본값
fn num_or(b: &Json, key: &str, default: f64) -> f64 {
    b.get(key).and_then(Json::as_f64).unwrap_or(default)
}

/// `POST /login/device/code` 응답(상태 코드·본문 문자열) 해석. 5xx·JSON 이 아닌 본문은 일시 오류 문구로 실패한다(TS 의 `TransientError`)
pub fn interpret_start(client_id: &str, status: u16, body: &str) -> Result<DeviceStart, DeviceError> {
    if client_id.is_empty() {
        return msg(MISSING_CLIENT_ID_MESSAGE);
    }
    if status >= 500 {
        return msg(format!("GitHub 서버 오류({status})"));
    }
    let Ok(b) = json::parse(body) else { return msg("GitHub 응답을 해석할 수 없습니다") };
    if b == Json::Null {
        return Err(DeviceError::TypeError);
    }
    match b.get("error").and_then(Json::as_str) {
        Some("device_flow_disabled") => return msg("이 OAuth App 은 Device Flow 가 꺼져 있습니다. GitHub 의 OAuth App 설정에서 \"Enable Device Flow\" 를 켜세요"),
        Some("incorrect_client_credentials" | "unauthorized_client") => return msg("Client ID 가 올바르지 않습니다. 설정에 입력한 값을 확인하세요"),
        _ => {}
    }
    let (Some(device_code), Some(user_code), Some(verification_uri)) = (text(&b, "device_code"), text(&b, "user_code"), text(&b, "verification_uri")) else {
        let desc = text(&b, "error_description").map(|d| format!(": {d}")).unwrap_or_default();
        return msg(format!("GitHub 로그인을 시작하지 못했습니다{desc}"));
    };
    Ok(DeviceStart {
        user_code: user_code.to_string(),
        verification_uri: verification_uri.to_string(),
        expires_in: num_or(&b, "expires_in", 900.0),
        interval: num_or(&b, "interval", 5.0).max(1.0),
        device_code: SecretString::new(device_code),
    })
}

/// 폴링 응답 한 건의 결과
#[derive(Debug)]
pub enum PollStep {
    /// 승인 대기 중 — 같은 간격으로 계속
    Pending,
    /// 간격을 늘리고 계속(새 간격은 [`PollState::interval`])
    SlowDown,
    /// 일시 오류(5xx·JSON 이 아닌 본문·네트워크) — 백오프를 늘려 계속
    Transient,
    Token(SecretString),
    Fail(DeviceError),
}

/// 폴링 상태: 현재 간격(초)과 연속 일시 오류 횟수. 첫 요청도 간격 뒤에 보낸다
#[derive(Clone, Debug)]
pub struct PollState {
    pub interval: f64,
    failures: u32,
}

impl PollState {
    pub fn new(interval: f64) -> Self {
        Self { interval, failures: 0 }
    }

    /// 다음 요청 전 대기(ms): 간격 + 일시 오류 백오프(회당 2초, 상한 20초)
    pub fn delay_ms(&self) -> f64 {
        self.interval * 1000.0 + (f64::from(self.failures) * TRANSIENT_BACKOFF_STEP_MS).min(TRANSIENT_BACKOFF_MAX_MS)
    }

    /// 요청 직전의 만료 검사: 대기를 마친 시각이 세션 만료 시각 이상이면 끝낸다
    pub fn expired(now_ms: f64, expires_at_ms: f64) -> bool {
        now_ms >= expires_at_ms
    }

    /// 일시 오류(네트워크·시간 초과)를 겪었다
    pub fn on_transient(&mut self) -> PollStep {
        self.failures += 1;
        PollStep::Transient
    }

    /// `POST /login/oauth/access_token` 응답(상태 코드·본문 문자열) 해석. 오류(`error` 필드)도 200 으로 오므로 상태 코드가 아니라 본문으로 판단한다
    pub fn on_response(&mut self, status: u16, body: &str) -> PollStep {
        if status >= 500 {
            return self.on_transient();
        }
        let Ok(b) = json::parse(body) else { return self.on_transient() };
        self.failures = 0;
        if b == Json::Null {
            return PollStep::Fail(DeviceError::TypeError);
        }
        if let Some(t) = text(&b, "access_token") {
            return PollStep::Token(SecretString::new(t));
        }
        match b.get("error").and_then(Json::as_str) {
            Some("authorization_pending") => PollStep::Pending,
            Some("slow_down") => {
                self.interval = match b.get("interval").and_then(Json::as_f64) {
                    Some(i) if i != 0.0 && i > self.interval => i,
                    _ => self.interval + SLOW_DOWN_STEP_S,
                };
                PollStep::SlowDown
            }
            Some("expired_token") => PollStep::Fail(DeviceError::Message(EXPIRED_MESSAGE.into())),
            Some("access_denied") => PollStep::Fail(DeviceError::Message("GitHub 에서 승인이 거부되었습니다".into())),
            other => {
                let tail = match (text(&b, "error_description"), other) {
                    (Some(d), _) => format!(": {d}"),
                    (None, Some(e)) if !e.is_empty() => format!(" ({e})"),
                    _ => String::new(),
                };
                PollStep::Fail(DeviceError::Message(format!("GitHub 로그인에 실패했습니다{tail}")))
            }
        }
    }
}
