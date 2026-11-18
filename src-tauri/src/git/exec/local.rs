//! 앱이 도는 OS 에서 git 을 직접 실행한다(`src/main/executors/LocalExecutor.ts`).
//! TS 의 `envFor`(GitHub 원격 전용 저장소의 환경 덮어쓰기)는 이 실행기가 아니라 `github.rs` 의 `GithubExecutor` 가 맡는다(Phase 6b PR 2).

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
            env_remove: Vec::new(),
            cwd: None,
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

/// 사용자 git 설정을 읽을 때 부모 환경에서 지우는 이름인가: 이미 격리된 값(`GIT_CONFIG_COUNT`·`_PARAMETERS`·`_KEY_n`·`_VALUE_n`)이나
/// 어떤 저장소의 설정이 섞이지 않게 하는 `GIT_(DIR|WORK_TREE|COMMON_DIR|INDEX_FILE|OBJECT_DIRECTORY)`(`githubCache.ts` `readNetworkConfig`).
/// 이름은 대소문자를 구분하지 않는다 — Windows 환경 변수가 그렇고, TS 의 정규식은 `Git_Config_Count` 를 놓친다(TS 보다 엄격)
pub(crate) fn is_inherited_git_selector(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    let numbered = |prefix: &str| n.strip_prefix(prefix).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    matches!(n.as_str(), "GIT_CONFIG_COUNT" | "GIT_CONFIG_PARAMETERS" | "GIT_DIR" | "GIT_WORK_TREE" | "GIT_COMMON_DIR" | "GIT_INDEX_FILE" | "GIT_OBJECT_DIRECTORY")
        || numbered("GIT_CONFIG_KEY_")
        || numbered("GIT_CONFIG_VALUE_")
}

/// 사용자(시스템·전역) git 설정 목록을 읽는다 — `git config --list --show-scope -z`(저장소 없이, 임시 폴더에서, 10초 제한, 출력 1MB 상한).
/// 실행기 밖에 프로세스를 늘리지 않으려고 이 모듈이 `Spec` 의 닫힌 변형(환경 일부 제거·작업 폴더)으로 돌린다. 읽지 못하면(git 없음·옛 git·시간 초과) `None`.
pub(crate) async fn read_user_config_list(git_binary: &str) -> Option<String> {
    let spec = Spec {
        program: git_binary.to_string(),
        argv: ["config", "--list", "--show-scope", "-z"].iter().map(|s| (*s).to_string()).collect(),
        env: Vec::new(),
        env_remove: std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).filter(|k| is_inherited_git_selector(k)).collect(),
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        git_args: vec!["config".to_string()],
        wsl: false,
    };
    let opts = RunOpts { max_buffer: Some(1024 * 1024), ..RunOpts::default() };
    // 시간이 지나면 future 를 버린다 — 자식 가드가 프로세스 트리를 죽인다
    let out = tokio::time::timeout(USER_CONFIG_TIMEOUT, run_process(&spec, &opts)).await.ok()?.ok()?;
    (out.code == 0).then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

const USER_CONFIG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use super::super::common::GIT_ENV;
    use super::*;

    #[test]
    fn 사용자_설정을_읽을_때_지우는_환경_이름() {
        for n in ["GIT_CONFIG_COUNT", "git_config_count", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_KEY_0", "Git_Config_Value_12", "GIT_DIR", "git_work_tree", "GIT_COMMON_DIR", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY"] {
            assert!(is_inherited_git_selector(n), "{n}");
        }
        // 사용자가 정한 설정 파일 위치와 무관한 이름은 남긴다
        for n in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_KEY_", "GIT_CONFIG_KEY_x", "GIT_CONFIG", "HOME", "GIT_DIRECTORY"] {
            assert!(!is_inherited_git_selector(n), "{n}");
        }
    }

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
