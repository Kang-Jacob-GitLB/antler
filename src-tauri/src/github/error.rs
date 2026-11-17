//! GitHub 서비스가 돌려주는 오류(`handlers.ts` 의 `handle` 과 같은 코드 순서): 취소는 `ABORTED`, GitHub API 오류는 `GH_*`, 그 밖(검증·저장·해석 실패)은 `ERROR`.
use crate::net::http::NetError;
use crate::net::js_value::MapError;
use crate::net::redact::redact_secrets;

/// IPC 봉투의 `code`·`message`. `message` 는 사용자에게 그대로 보여 줘도 되는 문장이며 [`GhError::sealed`] 로 비밀이 가려진다
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhError {
    pub code: &'static str,
    pub status: Option<u16>,
    pub message: String,
}

pub type Res<T> = Result<T, GhError>;

impl GhError {
    /// TS 의 평범한 `Error`(봉투 코드 `ERROR`)
    pub fn plain(message: impl Into<String>) -> Self {
        Self { code: "ERROR", status: None, message: message.into() }
    }

    pub fn aborted() -> Self {
        Self { code: "ABORTED", status: None, message: "aborted".to_string() }
    }

    pub fn is_aborted(&self) -> bool {
        self.code == "ABORTED"
    }

    /// 응답이 TS 라면 `TypeError` 가 났을 모양이다(문구는 TS 의 엔진 메시지 대신 고정 문장)
    pub fn bad_response() -> Self {
        Self::plain("GitHub 응답 형식이 올바르지 않습니다")
    }

    /// 문구에서 비밀을 가린다(마지막 단계)
    #[must_use]
    pub fn sealed(mut self, secrets: &[String]) -> Self {
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        self.message = redact_secrets(&self.message, &refs);
        self
    }
}

impl From<NetError> for GhError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::Api { code, status, message } => Self { code, status, message },
            NetError::InvalidPath => Self::plain("invalid api path"),
            NetError::Aborted => Self::aborted(),
        }
    }
}

impl From<MapError> for GhError {
    fn from(_: MapError) -> Self {
        Self::bad_response()
    }
}

impl std::fmt::Display for GhError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GhError {}
