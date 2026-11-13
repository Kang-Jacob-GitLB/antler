//! `src/main/executors` 의 Rust 이식(Local·WSL. SSH 는 Phase 5). 아직 앱에서 호출하지 않는다.
//!
//! - [`GIT_GLOBAL_ARGS`]·[`GIT_ENV`] 는 `common.rs` 한 곳에만 있고 `RunOpts` 에는 env 필드가 없다.
//! - git 을 띄우는 `Command::new` 는 `process.rs` 한 곳뿐이다(`src-tauri/clippy.toml` 의 disallowed-methods 가 강제한다).
//! - [`ExecError`] 의 종류는 IPC envelope 의 code(GIT / ABORTED / ERROR)로 매핑된다.

mod common;
mod error;
mod local;
mod process;
pub mod queue;
mod wsl;
pub mod wsl_paths;

pub use common::{build_git_args, RunOpts, RunResult, DEFAULT_MAX_BUFFER, GIT_ENV, GIT_GLOBAL_ARGS, MAX_STDERR};
pub use error::ExecError;
pub use local::LocalExecutor;
pub use process::GitStream;
pub use wsl::{wsl_argv, WslExecutor, WSL_SPAWN_ENV};

/// 환경별 git 실행. 저장소가 있는 환경 안에서 `git -C <repo> …` 를 실행한다
/// (repo 경로는 그 환경의 원래 형식이며 변환하지 않는다).
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
