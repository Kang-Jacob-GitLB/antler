//! 모든 git 호출에 공통인 전역 인자·환경변수(`src/main/executors/GitExecutor.ts`)와 실행 옵션.
//! 이 두 상수는 **이 모듈에만** 있고, 호출자는 환경변수를 바꿀 수 없다(`RunOpts` 에 env 필드가 없다).
//! 값과 순서는 `tests/golden/exec/env.json`(TS 가 만든 골든)과 같아야 한다.

use tokio_util::sync::CancellationToken;

/// 모든 git 호출에 붙는 전역 옵션.
/// `core.fsmonitor=false`: 저장소 설정의 fsmonitor 명령은 index 를 읽는 명령에서 실행된다(악성 저장소 RCE 경로).
pub const GIT_GLOBAL_ARGS: [&str; 8] = [
    "-c",
    "core.quotepath=false",
    "-c",
    "color.ui=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "core.fsmonitor=false",
];

/// 모든 git 호출에 넣는 환경변수(순서가 WSL argv 의 `env K=V…` 순서를 정한다).
///
/// 보안: 읽기 전용 명령이라도 partial clone(promisor) 저장소에서 없는 객체를 만나면 git 이 원격에서 객체를
/// 가져오려(lazy fetch) 하고, 그때 저장소 설정의 `core.sshCommand`·`remote.*.uploadpack` 을 실행한다.
/// `GIT_NO_LAZY_FETCH=1`(git 2.45.1+)과 `GIT_ALLOW_PROTOCOL=none`(옛 git 에서도 동작) 두 겹으로 막는다.
/// `GIT_NO_REPLACE_OBJECTS=1` 은 `refs/replace` 로 위조한 이력을 보여 주지 않는다.
pub const GIT_ENV: [(&str, &str); 9] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    // 경로가 pathspec magic(:(top), *, :! 등)으로 해석되지 않게 한다
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_ALLOW_PROTOCOL", "none"),
    ("GIT_PAGER", "cat"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
];

/// `run` 이 담는 stdout 상한(기본 64MB)
pub const DEFAULT_MAX_BUFFER: usize = 64 * 1024 * 1024;

/// stderr 는 앞부분만 4MB 까지 담는다(악성 저장소·서버가 끝없이 써서 메모리를 채우지 못하게). 파이프는 계속 비운다
pub const MAX_STDERR: usize = 4 * 1024 * 1024;

/// `git -C <repo> <전역 인자> <args…>` 의 인자 목록
pub fn build_git_args<S: AsRef<str>>(repo_path: &str, args: &[S]) -> Vec<String> {
    let mut v = Vec::with_capacity(2 + GIT_GLOBAL_ARGS.len() + args.len());
    v.push("-C".to_string());
    v.push(repo_path.to_string());
    v.extend(GIT_GLOBAL_ARGS.iter().map(|s| (*s).to_string()));
    v.extend(args.iter().map(|s| s.as_ref().to_string()));
    v
}

/// 호출 옵션. **env 필드가 없다** — 호출자가 방어용 환경변수를 덮어쓰지 못하게 한다.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// 취소되면 프로세스 트리를 죽이고 `Aborted` 로 끝난다
    pub cancel: Option<CancellationToken>,
    /// 이 바이트 수를 넘으면 실패시킨다(`run` 만, 기본 [`DEFAULT_MAX_BUFFER`])
    pub max_buffer: Option<usize>,
}

impl RunOpts {
    pub fn with_cancel(cancel: CancellationToken) -> Self {
        RunOpts { cancel: Some(cancel), max_buffer: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// 종료 코드. 신호로 죽었거나 Windows 의 0xFFFFFFFF(wsl.exe 자체 실패)는 -1
    pub code: i32,
}

/// 실행할 프로세스. 실행기가 만들고 `process` 모듈만 읽는다
#[derive(Debug, Clone)]
pub(super) struct Spec {
    pub program: String,
    pub argv: Vec<String>,
    /// 부모 환경에 덧씌울 값
    pub env: Vec<(String, String)>,
    /// 오류 메시지에 쓸 git 인자(`-C`·전역 인자 제외)
    pub git_args: Vec<String>,
    /// WSL 이면 wsl.exe 자신의 실패를 stdout 메시지로 해석한다
    pub wsl: bool,
}

pub(super) fn to_strings<S: AsRef<str>>(args: &[S]) -> Vec<String> {
    args.iter().map(|s| s.as_ref().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_git_args_는_dash_c_저장소_전역_인자_args_순서다() {
        let v = build_git_args("/r", &["log", "-z"]);
        assert_eq!(&v[..2], ["-C", "/r"]);
        assert_eq!(&v[2..10], GIT_GLOBAL_ARGS);
        assert_eq!(&v[10..], ["log", "-z"]);
    }
}
