//! 모든 git 호출에 공통인 전역 인자·환경변수(`src/main/executors/GitExecutor.ts`)와 실행 옵션.
//! 이 두 상수는 **이 모듈에만** 있고, 호출자는 환경변수를 마음대로 바꿀 수 없다(`RunOpts` 에 일반 env 필드가 없다 —
//! fetch 가 덮어쓰는 여섯 개는 닫힌 타입 [`FetchEnv`] 로만 준다).
//! 값과 순서는 `tests/golden/exec/env.json`(TS 가 만든 골든)과 같아야 한다.

use tokio_util::sync::CancellationToken;

use crate::git::fetch::guard::fetch_env;

/// 모든 git 호출에 붙는 전역 옵션.
/// `core.fsmonitor=false`: 저장소 설정의 fsmonitor 명령은 index 를 읽는 명령에서 실행된다(악성 저장소 RCE 경로).
/// `core.protectNTFS=true`: 저장소 로컬 설정이 false 로 꺼 NTFS 위험 경로(`.git.`·`::$INDEX_ALLOCATION`·8.3) 보호를 풀지 못하게 고정한다.
/// 모든 플랫폼의 git 이 인식하고 모르는 core 키는 무시하므로 WSL·SSH 의 Linux git 에도 무해하다.
/// `diff.submodule=short`: 저장소 설정이 `diff` 로 두면 porcelain diff 가 서브모듈 안에서 자식 git diff 를 띄워 그 저장소의 diff.external 이 실행될 수 있다.
/// `submodule.recurse=false`: 저장소 설정이 서브모듈 재귀를 켜 읽기 명령이 서브모듈 안으로 들어가지 못하게 한다.
pub const GIT_GLOBAL_ARGS: [&str; 14] = [
    "-c",
    "core.quotepath=false",
    "-c",
    "color.ui=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.protectNTFS=true",
    "-c",
    "diff.submodule=short",
    "-c",
    "submodule.recurse=false",
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

/// fetch 가 [`GIT_ENV`] 위에 덮어쓰는 환경변수. **일반 env 가 아니라 닫힌 타입이다** — 키는 [`fetch_env`] 의 상수이고 자유 문자열은
/// `GIT_SSH_COMMAND` 의 값뿐이라, 호출자가 방어용 환경변수(`GIT_ALLOW_PROTOCOL` 등)를 임의로 바꿀 수 없다.
/// TS 의 `RunOpts.env`(자유 객체)에 대응하지만, 여기서는 fetch 용 여섯 개만 표현할 수 있다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchEnv {
    /// `GIT_ALLOW_PROTOCOL` 에 file 을 넣는다(수동 fetch + CVE-2024-32004 가 고쳐진 git 에서만)
    pub allow_file: bool,
    /// `GIT_SSH_COMMAND` 값
    pub ssh_command: String,
}

/// [`GIT_ENV`] 에 fetch 환경을 덮어쓴 목록. JS `{ ...GIT_ENV, ...fetchEnv }` 의 순서와 같다 — 이미 있는 키는 제자리에서 값만 바뀌고,
/// 새 키는 fetch 환경의 순서대로 뒤에 붙는다(WSL argv 의 `env K=V…` 순서가 이것이다)
pub fn merged_env(fetch: Option<&FetchEnv>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = GIT_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    if let Some(f) = fetch {
        for (k, v) in fetch_env(&f.ssh_command, f.allow_file) {
            match env.iter_mut().find(|(ek, _)| ek == k) {
                Some(slot) => slot.1 = v,
                None => env.push((k.to_string(), v)),
            }
        }
    }
    env
}

/// 호출 옵션. 환경변수는 **닫힌 [`FetchEnv`] 로만** 바꿀 수 있다(`run` 에서만 쓰이고 `stream` 은 무시한다).
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// 취소되면 프로세스 트리를 죽이고 `Aborted` 로 끝난다
    pub cancel: Option<CancellationToken>,
    /// 이 바이트 수를 넘으면 실패시킨다(`run` 만, 기본 [`DEFAULT_MAX_BUFFER`])
    pub max_buffer: Option<usize>,
    /// fetch 전용 환경(`run` 만)
    pub fetch_env: Option<FetchEnv>,
}

impl RunOpts {
    pub fn with_cancel(cancel: CancellationToken) -> Self {
        RunOpts { cancel: Some(cancel), ..RunOpts::default() }
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
        assert_eq!(&v[2..16], GIT_GLOBAL_ARGS);
        assert_eq!(&v[16..], ["log", "-z"]);
    }

    #[test]
    fn 전역_인자가_core_protect_ntfs_를_true_로_고정한다() {
        assert!(GIT_GLOBAL_ARGS.windows(2).any(|w| w == ["-c", "core.protectNTFS=true"]));
    }

    #[test]
    fn 전역_인자가_서브모듈_diff_와_재귀를_고정한다() {
        assert!(GIT_GLOBAL_ARGS.windows(2).any(|w| w == ["-c", "diff.submodule=short"]));
        assert!(GIT_GLOBAL_ARGS.windows(2).any(|w| w == ["-c", "submodule.recurse=false"]));
    }
}
