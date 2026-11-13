//! 앱이 도는 OS 에서 git 을 직접 실행한다(`src/main/executors/LocalExecutor.ts`).
//! TS 의 `envFor`(GitHub 원격 전용 저장소의 환경 덮어쓰기)는 옮기지 않았다 — 그 저장소는 사이드카가 계속 처리한다.

use super::common::{build_git_args, to_strings, RunOpts, RunResult, Spec, GIT_ENV};
use super::error::ExecError;
use super::process::{run_process, stream_process, GitStream};

pub struct LocalExecutor {
    git_binary: String,
}

impl Default for LocalExecutor {
    fn default() -> Self {
        LocalExecutor { git_binary: "git".to_string() }
    }
}

impl LocalExecutor {
    pub fn new(git_binary: impl Into<String>) -> Self {
        LocalExecutor { git_binary: git_binary.into() }
    }

    fn spec<S: AsRef<str>>(&self, repo_path: &str, args: &[S]) -> Spec {
        Spec {
            program: self.git_binary.clone(),
            argv: build_git_args(repo_path, args),
            env: GIT_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            git_args: to_strings(args),
            wsl: false,
        }
    }

    /// stdout 을 전부 모아 돌려준다(상한 `opts.max_buffer`, 기본 64MB). 종료 코드가 0 이 아니어도 `Ok` — 호출자가 본다.
    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        run_process(&self.spec(repo_path, args), opts).await
    }

    /// stdout 을 바이트 스트림으로 준다. 비정상 종료는 EOF 뒤 [`GitStream::finish`] 가 알린다.
    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        stream_process(&self.spec(repo_path, args), opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_은_공통_인자와_환경만_쓴다() {
        let s = LocalExecutor::default().spec("/r", &["log"]);
        assert_eq!(s.program, "git");
        assert_eq!(s.argv, build_git_args("/r", &["log"]));
        assert_eq!(s.git_args, ["log"]);
        assert_eq!(s.env.len(), GIT_ENV.len());
        assert!(s.env.contains(&("GIT_NO_LAZY_FETCH".to_string(), "1".to_string())));
        assert!(!s.wsl);
    }
}
