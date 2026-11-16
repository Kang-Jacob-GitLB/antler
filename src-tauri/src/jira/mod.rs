//! Jira(`src/main/jira`) 이식(PR 1 순수 로직, PR 3 HTTP 클라이언트). 키체인은 뒤 PR 의 몫이다.
//! - [`validate`]: renderer 가 보낸 IPC 인자 검증(`validate.ts`)
//! - [`adf`]·[`api_map`]: ADF → HTML 변환과 API 응답 → 공유 타입 매퍼(PR 2. `adf.ts`·`api.ts` 의 순수 부분)
//! - [`client`]: HTTP 클라이언트(PR 3. GET·POST·프로필 사진, 429 재시도, 본문 상한)
//! - [`service`]: 연결 상태·세대(epoch)·날짜 필드·프로필 사진 캐시를 가진 서비스(PR 5. `service.ts` + `api.ts` 의 호출부, 앱에는 연결하지 않는다)
//! - 이 파일: 자격 증명 종류와 `Authorization` 헤더(`client.ts` 의 `authHeader`)
//!
//! 공통 해석(오류 분류·Retry-After·사이트 안 판정)은 [`crate::net`] 이다.
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::net::redact::SecretString;

pub mod adf;
pub mod api_map;
pub mod client;
pub mod service;
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
