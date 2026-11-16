//! 앱이 도는 OS 에서 git 을 직접 실행한다(`src/main/executors/LocalExecutor.ts`).
//! TS 의 `envFor`(GitHub 원격 전용 저장소의 환경 덮어쓰기)는 옮기지 않았다 — 그 저장소는 사이드카가 계속 처리한다.

use super::common::{build_git_args, merged_env, to_strings, FetchEnv, RunOpts, RunResult, Spec};
use super::error::ExecError;
use super::process::{run_process, stream_process, GitStream};

#[derive(Clone)]
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

    /// 실행 파일(캐시 키)
    pub fn git_binary(&self) -> &str {
        &self.git_binary
    }

    fn spec<S: AsRef<str>>(&self, repo_path: &str, args: &[S], fetch: Option<&FetchEnv>) -> Spec {
        Spec {
            program: self.git_binary.clone(),
            argv: build_git_args(repo_path, args),
            env: merged_env(fetch),
            git_args: to_strings(args),
            wsl: false,
        }
    }

    /// stdout 을 전부 모아 돌려준다(상한 `opts.max_buffer`, 기본 64MB). 종료 코드가 0 이 아니어도 `Ok` — 호출자가 본다.
    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        run_process(&self.spec(repo_path, args, opts.fetch_env.as_ref()), opts).await
    }

    /// stdout 을 바이트 스트림으로 준다. 비정상 종료는 EOF 뒤 [`GitStream::finish`] 가 알린다.
    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        stream_process(&self.spec(repo_path, args, None), opts)
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::GIT_ENV;
    use super::*;

    #[test]
    fn spec_은_공통_인자와_환경만_쓴다() {
        let s = LocalExecutor::default().spec("/r", &["log"], None);
        assert_eq!(s.program, "git");
        assert_eq!(s.argv, build_git_args("/r", &["log"]));
        assert_eq!(s.git_args, ["log"]);
        assert_eq!(s.env.len(), GIT_ENV.len());
        assert!(s.env.contains(&("GIT_NO_LAZY_FETCH".to_string(), "1".to_string())));
        assert!(!s.wsl);
    }

    #[test]
    fn fetch_환경은_공통_환경_위에_덮어쓴다() {
        let f = FetchEnv { allow_file: true, ssh_command: "ssh -x".into() };
        let s = LocalExecutor::default().spec("/r", &["fetch"], Some(&f));
        let get = |k: &str| s.env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str());
        assert_eq!(get("GIT_ALLOW_PROTOCOL"), Some("https:http:ssh:git:file"));
        assert_eq!(get("GIT_ASKPASS"), Some(""));
        assert_eq!(get("GIT_SSH_COMMAND"), Some("ssh -x"));
        // 덮어쓰지 않는 방어 값은 그대로다
        assert_eq!(get("GIT_NO_LAZY_FETCH"), Some("1"));
        assert_eq!(get("LC_ALL"), Some("C.UTF-8"));
    }
}
