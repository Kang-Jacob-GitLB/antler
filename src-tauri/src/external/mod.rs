//! 외부 도구로 열기(Phase 6b PR 3): 에디터·터미널·파일 관리자 실행 계획과 실행기. PR 9 에서 `config_host`(`external:open`·`external:pickEditorExe`)가 연결했다 — 프로세스는 [`launch`] 한 곳만 띄우고 `lib.rs` 가 주입한다.
//!
//! - [`paths`]: Node `path` 의 win32·posix 판(`relative` 포함).
//! - [`plan`]: 경로 검증·실행 인자 구성의 순수 함수(`plan.ts`).
//! - [`opener`]: [`opener::ExternalOpener`] — 파일 시스템·환경·프로세스 실행·파일 관리자를 [`opener::OpenerDeps`] 로 주입받는다(`opener.ts`).
//! - [`link`]: `code:openLink` 의 주소 허용(`link.ts`).
//! - [`launch`]: 실제 프로세스 실행·파일 관리자·파일 시스템의 운영 구현. **에디터·터미널 프로세스를 띄우는 유일한 곳**이다(소스 검사: tests/external.rs).

pub mod launch;
pub mod link;
pub mod opener;
pub mod paths;
pub mod plan;
