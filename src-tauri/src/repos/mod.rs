//! 저장소 추가 흐름의 순수 로직(Phase 6b PR 4a — `src/main/services/RepoService.ts`·`repoScan.ts` 의 이식). 앱에는 연결하지 않는다(연결은 PR 8·9).
//!
//! - [`env`]: 입력의 환경·경로 해석(`resolve_env`·`remote_relative`)과 저장 전 SSH 연결 테스트(풀 `hold`, 호스트 키 TOFU 금지 안내).
//! - [`scan`]: 폴더 안 저장소 찾기(로컬·WSL 공유 경로 `scan_local_repos`, SSH 목록 `scan_from_listing`).
pub mod env;
pub mod scan;
