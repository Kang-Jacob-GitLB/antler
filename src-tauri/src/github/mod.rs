//! GitHub(`src/main/github`)의 순수 로직 이식(PR 1). 네트워크·키체인·gh CLI 실행은 뒤 PR 의 몫이다.
//! - [`validate`]: renderer 가 보낸 IPC 인자 검증(`validate.ts`)
//! - [`device_rules`]: OAuth Device Flow 응답 해석(`deviceFlow.ts` 의 분기)
//! - [`gh_env`]: gh CLI 에 넘기는 환경 변수 거르기(`ghCli.ts` 의 `ghEnv`)
//! - [`api_map`]·[`checks_map`]·[`launchpad_map`]·[`accessible_repos`]: API 응답 JSON → 공유 타입 매퍼(PR 2. `api.ts`·`checks.ts`·`launchpad.ts`·`accessibleRepos.ts` 의 순수 부분)
//!
//! 공통 해석(오류 분류·Link·rate limit·토큰 가리기)은 [`crate::net`] 이다.
pub mod accessible_repos;
pub mod api_map;
pub mod checks_map;
pub mod device_rules;
pub mod gh_env;
pub mod launchpad_map;
pub mod validate;
