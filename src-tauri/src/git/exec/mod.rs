//! `src/main/executors` 의 Rust 이식(Local·WSL. SSH 는 Phase 5 — 순수 로직만 `ssh` 에 있다). 아직 앱에서 호출하지 않는다.
//!
//! - [`GIT_GLOBAL_ARGS`]·[`GIT_ENV`] 는 `common.rs` 한 곳에만 있고 `RunOpts` 에는 일반 env 필드가 없다(fetch 가 덮어쓰는 값은 닫힌 [`FetchEnv`] 로만, `run` 에서만).
//! - git 을 띄우는 `Command::new` 는 `process.rs` 한 곳뿐이다(`src-tauri/clippy.toml` 의 disallowed-methods 가 강제한다). gh CLI 를 띄우는 `github/gh_cli.rs` 만 예외다.
//! - [`ExecError`] 의 종류는 IPC envelope 의 code(GIT / ABORTED / ERROR)로 매핑된다.

mod common;
mod error;
// gh CLI 실행(github/gh_cli.rs)도 같은 Job Object 를 쓴다
#[cfg(windows)]
pub(crate) mod job;
mod local;
mod process;
pub mod queue;
// SSH 의 순수 로직(설정·known_hosts·원격 명령 인용, Phase 5 PR 1). 아직 앱에서 호출하지 않는다 — 통합 테스트(tests/ssh_golden.rs)가 별도 크레이트라 pub 이다
pub mod ssh;
mod wsl;
pub mod wsl_paths;

pub use common::{build_git_args, merged_env, FetchEnv, RunOpts, RunResult, DEFAULT_MAX_BUFFER, GIT_ENV, GIT_GLOBAL_ARGS, MAX_STDERR};
pub use error::ExecError;
pub use local::LocalExecutor;
pub use process::{set_jobs_disabled, shutdown_kills, GitStream};
pub use wsl::{wsl_argv, wsl_argv_with, WslExecutor, WSL_SPAWN_ENV};

/// 환경별 git 실행. 저장소가 있는 환경 안에서 `git -C <repo> …` 를 실행한다
/// (repo 경로는 그 환경의 원래 형식이며 변환하지 않는다).
#[derive(Clone)]
pub enum GitExecutor {
    Local(LocalExecutor),
    Wsl(WslExecutor),
}

impl GitExecutor {
    pub fn kind(&self) -> &'static str {
        match self {
            GitExecutor::Local(_) => "local",
            GitExecutor::Wsl(_) => "wsl",
        }
    }

    /// 같은 git 을 가리키는 실행기끼리 같은 키(git 버전 캐시용): 로컬은 실행 파일, WSL 은 배포판
    pub fn cache_key(&self) -> String {
        match self {
            GitExecutor::Local(e) => format!("local:{}", e.git_binary()),
            GitExecutor::Wsl(e) => format!("wsl:{}", e.distro()),
        }
    }

    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        match self {
            GitExecutor::Local(e) => e.run(repo_path, args, opts).await,
            GitExecutor::Wsl(e) => e.run(repo_path, args, opts).await,
        }
    }

    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        match self {
            GitExecutor::Local(e) => e.stream(repo_path, args, opts),
            GitExecutor::Wsl(e) => e.stream(repo_path, args, opts),
        }
    }
}
