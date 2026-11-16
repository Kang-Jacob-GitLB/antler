//! Jira(`src/main/jira`)의 순수 로직 이식(PR 1). 네트워크·키체인은 뒤 PR 의 몫이다.
//! - [`validate`]: renderer 가 보낸 IPC 인자 검증(`validate.ts`)
//! - 이 파일: 자격 증명 종류와 `Authorization` 헤더(`client.ts` 의 `authHeader`)
//!
//! 공통 해석(오류 분류·Retry-After·사이트 안 판정)은 [`crate::net`] 이다.
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::net::redact::SecretString;

pub mod validate;

/// Jira 배포 종류. `cloud` 는 이메일:API 토큰 Basic, `server`(Data Center)는 PAT Bearer
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JiraKind {
    Cloud,
    Server,
}

/// `Authorization` 헤더 값(TS `authHeader`). Cloud 는 `Basic base64(email:token)`, Data Center 는 `Bearer <token>`
pub fn auth_header(kind: JiraKind, email: &str, token: &SecretString) -> SecretString {
    SecretString::new(match kind {
        JiraKind::Cloud => format!("Basic {}", STANDARD.encode(format!("{email}:{}", token.expose()))),
        JiraKind::Server => format!("Bearer {}", token.expose()),
    })
}
