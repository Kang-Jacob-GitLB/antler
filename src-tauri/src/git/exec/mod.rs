//! `src/main/executors` 의 Rust 이식(Local·WSL·SSH). SSH 는 Phase 5 — 순수 로직(PR 1)·russh 실행기 `run`(PR 2a)·`stream`·`GitExecutor::Ssh`(PR 2b). 라우터가 `SshPool` 로 호출한다(PR 3).
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
// SSH(순수 로직 = Phase 5 PR 1, russh 실행기 = PR 2, 라우터 연결 = PR 3 — `SshPool`). 통합 테스트(tests/ssh_golden.rs)가 별도 크레이트라 pub 이다
pub mod ssh;
mod wsl;
pub mod wsl_paths;

use common::to_strings;
use ssh::SshExecutor;

pub use common::{build_git_args, merged_env, FetchEnv, RunOpts, RunResult, DEFAULT_MAX_BUFFER, GIT_ENV, GIT_GLOBAL_ARGS, MAX_STDERR};
pub use error::ExecError;
pub use local::LocalExecutor;
pub use process::{set_jobs_disabled, shutdown_kills, GitStream, ProcessStream};
pub use wsl::{wsl_argv, wsl_argv_with, WslExecutor, WSL_SPAWN_ENV};

/// 환경별 git 실행. 저장소가 있는 환경 안에서 `git -C <repo> …` 를 실행한다
/// (repo 경로는 그 환경의 원래 형식이며 변환하지 않는다).
#[derive(Clone)]
pub enum GitExecutor {
    Local(LocalExecutor),
    Wsl(WslExecutor),
    Ssh(SshExecutor),
}

impl GitExecutor {
    pub fn kind(&self) -> &'static str {
        match self {
            GitExecutor::Local(_) => "local",
            GitExecutor::Wsl(_) => "wsl",
            GitExecutor::Ssh(_) => "ssh",
        }
    }

    /// 정리돼 영구히 거부하는 실행기인가(SSH 만 해당한다). 이런 실행기를 쥔 쪽(fetch 서비스)은 버리고 새로 받아야 한다
    pub fn is_closed(&self) -> bool {
        matches!(self, GitExecutor::Ssh(e) if e.is_disposed())
    }

    /// 같은 git 을 가리키는 실행기끼리 같은 키(git 버전 캐시용): 로컬은 실행 파일, WSL 은 배포판, SSH 는 호스트
    pub fn cache_key(&self) -> String {
        match self {
            GitExecutor::Local(e) => format!("local:{}", e.git_binary()),
            GitExecutor::Wsl(e) => format!("wsl:{}", e.distro()),
            GitExecutor::Ssh(e) => format!("ssh:{}", e.host_spec()),
        }
    }

    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        match self {
            GitExecutor::Local(e) => e.run(repo_path, args, opts).await,
            GitExecutor::Wsl(e) => e.run(repo_path, args, opts).await,
            GitExecutor::Ssh(e) => e.run(repo_path, args, opts).await,
        }
    }

    /// stdout 을 바이트 스트림으로 준다. Local·WSL 은 자식 프로세스를 띄우는 CreateProcess 가 느려(부하가 있으면 초 단위) blocking 풀에서 띄우고,
    /// SSH 는 원격 명령이 시작될 때까지 기다린다. 띄우는 도중 future 를 버려도 결과 스트림이 버려지며 프로세스·원격 명령이 정리된다.
    /// SSH 스트림은 선읽기 상한(16MB)을 넘으면 원격을 죽이고 읽기가 [`ssh::is_read_ahead_exceeded`] 오류로 끝난다(Local·WSL 에는 없다).
    pub async fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        match self {
            GitExecutor::Local(e) => {
                let (e, repo, args, opts) = (e.clone(), repo_path.to_string(), to_strings(args), opts.clone());
                blocking_stream(move || e.stream(&repo, &args, &opts)).await
            }
            GitExecutor::Wsl(e) => {
                let (e, repo, args, opts) = (e.clone(), repo_path.to_string(), to_strings(args), opts.clone());
                blocking_stream(move || e.stream(&repo, &args, &opts)).await
            }
            GitExecutor::Ssh(e) => Ok(GitStream::Ssh(e.stream(repo_path, args, opts).await?)),
        }
    }
}

/// 자식 프로세스를 띄우는 동기 함수를 blocking 풀에서 돌린다
async fn blocking_stream(f: impl FnOnce() -> Result<GitStream, ExecError> + Send + 'static) -> Result<GitStream, ExecError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ExecError::Other { message: e.to_string() })?
}
