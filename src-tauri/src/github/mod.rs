//! GitHub(`src/main/github`) 이식(PR 1 순수 로직, PR 3 HTTP 클라이언트). 키체인·gh CLI 실행은 뒤 PR 의 몫이다.
//! - [`validate`]: renderer 가 보낸 IPC 인자 검증(`validate.ts`)
//! - [`device_rules`]: OAuth Device Flow 응답 해석(`deviceFlow.ts` 의 분기)
//! - [`gh_env`]: gh CLI 에 넘기는 환경 변수 거르기(`ghCli.ts` 의 `ghEnv`)
//! - [`api_map`]·[`checks_map`]·[`launchpad_map`]·[`accessible_repos`]: API 응답 JSON → 공유 타입 매퍼(PR 2. `api.ts`·`checks.ts`·`launchpad.ts`·`accessibleRepos.ts` 의 순수 부분)
//! - [`client`]: HTTP 클라이언트(PR 3. REST GET·GraphQL, ETag 캐시, core 한도 추적)
//! - [`service`]: 로그인 상태·세대·Device Flow·조회를 가진 서비스(PR 7. `service.ts`, 앱에는 연결하지 않는다). 하위: [`device_flow`](Device Flow 요청), [`queries`](`api.ts` 의 호출부),
//!   [`ports`](사이드카·외부 토큰 출처에 묻는 트레이트), [`error`](봉투 오류), [`avatars`](작성자 아바타 해석·캐시), [`checks`](체크 상태 조회·캐시), [`launchpad`](Launchpad 조회)
//!
//! 공통 해석(오류 분류·Link·rate limit·토큰 가리기)은 [`crate::net`] 이다.
pub mod accessible_repos;
pub mod api_map;
pub mod avatars;
pub mod checks;
pub mod checks_map;
pub mod client;
pub mod device_flow;
pub mod device_rules;
pub mod error;
pub mod gh_env;
pub mod launchpad;
pub mod launchpad_map;
pub mod ports;
pub mod queries;
pub mod service;
pub mod validate;
