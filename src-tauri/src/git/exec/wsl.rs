//! WSL 배포판 안에서 git 을 실행한다(`src/main/executors/WslExecutor.ts`). repo 경로는 리눅스 경로(`/home/me/repo`).
//!
//! `wsl.exe --exec` 는 리눅스 셸을 거치지 않으므로 인자 인용·인젝션 문제가 없다.
//! `--cd ~` 로 배포판 사용자 홈에서 시작하므로 repo 경로는 홈 기준 상대 경로(`proj`)도 된다.
//! Windows 환경변수는 WSLENV 없이는 WSL 로 넘어가지 않아 [`GIT_ENV`] 를 `env` 명령으로 전달한다.
//!
//! 취소: `taskkill /T` 가 wsl.exe 를 죽이면 리눅스 쪽 git 도 끝나는지는 `tests/exec.rs` 의 WSL 통합 테스트에서 실측한다.

use super::common::{build_git_args, to_strings, RunOpts, RunResult, Spec, GIT_ENV};
use super::error::ExecError;
use super::process::{run_process, stream_process, GitStream};
use crate::git::parsers::js_trim;

/// wsl.exe 가 내보내는 자체 메시지의 인코딩을 UTF-8 로 고정한다. 기본은 UTF-16LE 지만 사용자가 `WSL_UTF8=1` 을
/// 설정해 두었을 수도 있어, 어느 쪽인지 추측하지 않도록 항상 지정한다(파싱은 두 인코딩 모두 받아들인다).
/// wsl.exe **자신**의 환경이다 — 리눅스 쪽 git 의 환경은 [`wsl_argv`] 의 `env` 로 준다.
pub const WSL_SPAWN_ENV: [(&str, &str); 1] = [("WSL_UTF8", "1")];

/// wsl.exe 에 넘길 argv: `-d <distro> --cd ~ --exec env K=V… git -C <repo> <전역 인자> <args…>`
pub fn wsl_argv<S: AsRef<str>>(distro: &str, repo_path: &str, args: &[S]) -> Vec<String> {
    let mut v: Vec<String> = ["-d", distro, "--cd", "~", "--exec", "env"].iter().map(|s| (*s).to_string()).collect();
    v.extend(GIT_ENV.iter().map(|(k, val)| format!("{k}={val}")));
    v.push("git".to_string());
    v.extend(build_git_args(repo_path, args));
    v
}

/// wsl.exe 자체 실패 종료 코드(0xFFFFFFFF — `i32` 로는 -1)
fn is_wsl_failure(code: i32) -> bool {
    code == -1
}

/// UTF-16LE(짝수 바이트 뒤 NUL 이 많은 경우)인지 대략 판별해 디코딩한다. BOM·`\r`·NUL 은 뺀다
fn decode_wsl_text(buf: &[u8]) -> String {
    let window = buf.len().min(64);
    let zeros = (1..window).step_by(2).filter(|&i| buf[i] == 0).count();
    let utf16 = buf.len() >= 4 && zeros * 4 >= window;
    let text = if utf16 {
        let units = buf.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p));
        char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect::<String>()
    } else {
        String::from_utf8_lossy(buf).into_owned()
    };
    text.chars().filter(|&c| c != '\u{FEFF}' && c != '\0' && c != '\r').collect()
}

fn wsl_message(buf: &[u8]) -> String {
    let text = decode_wsl_text(buf);
    let text = js_trim(&text);
    if text.is_empty() {
        "WSL 실행에 실패했습니다(배포판 이름과 WSL 상태를 확인하세요)".to_string()
    } else {
        format!("WSL: {text}")
    }
}

/// wsl.exe 자체가 실패했는데(배포판 없음·WSL 서비스 중지) stderr 가 비어 있으면, stdout 에 나온 WSL 메시지로 오류를 만든다
pub(super) fn failure_error(code: i32, stderr: &str, stdout_head: &[u8], git_args: &[String]) -> Option<ExecError> {
    if is_wsl_failure(code) && js_trim(stderr).is_empty() {
        let msg = wsl_message(stdout_head);
        return Some(ExecError::Git { message: msg.clone(), code, stderr: msg, args: git_args.to_vec() });
    }
    None
}

pub struct WslExecutor {
    distro: String,
}

impl WslExecutor {
    pub fn new(distro: impl Into<String>) -> Self {
        WslExecutor { distro: distro.into() }
    }

    fn spec<S: AsRef<str>>(&self, repo_path: &str, args: &[S]) -> Spec {
        Spec {
            program: "wsl.exe".to_string(),
            argv: wsl_argv(&self.distro, repo_path, args),
            env: WSL_SPAWN_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            git_args: to_strings(args),
            wsl: true,
        }
    }

    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        let r = run_process(&self.spec(repo_path, args), opts).await?;
        // wsl.exe 자체가 실패하면 메시지를 stdout 으로 내고 stderr 는 비어 있다
        if is_wsl_failure(r.code) && js_trim(&r.stderr).is_empty() {
            let msg = wsl_message(&r.stdout);
            return Ok(RunResult { stdout: Vec::new(), stderr: msg, code: r.code });
        }
        Ok(r)
    }

    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        stream_process(&self.spec(repo_path, args), opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn argv_는_exec_env_git_dash_c_형태다() {
        let argv = wsl_argv("Ubuntu", "/home/me/내 repo", &["log", "-z"]);
        assert_eq!(&argv[..6], ["-d", "Ubuntu", "--cd", "~", "--exec", "env"]);
        assert!(argv.contains(&"LC_ALL=C.UTF-8".to_string()));
        assert!(argv.contains(&"GIT_LITERAL_PATHSPECS=1".to_string()));
        let git_at = argv.iter().position(|a| a == "git").unwrap();
        assert_eq!(&argv[git_at..git_at + 3], ["git", "-C", "/home/me/내 repo"]);
        assert_eq!(&argv[argv.len() - 2..], ["log", "-z"]);
    }

    #[test]
    fn wsl_메시지는_utf16le_도_utf8_도_받는다() {
        assert_eq!(wsl_message(&utf16le("\u{FEFF}There is no distribution with the supplied name.\r\n")), "WSL: There is no distribution with the supplied name.");
        assert_eq!(wsl_message(b"Wsl/Service/WSL_E_DISTRO_NOT_FOUND\r\n"), "WSL: Wsl/Service/WSL_E_DISTRO_NOT_FOUND");
        assert!(wsl_message(b"").starts_with("WSL 실행에 실패했습니다"));
    }

    #[test]
    fn wsl_자체_실패만_메시지로_바꾼다() {
        let args = vec!["log".to_string()];
        let e = failure_error(-1, "  \n", b"no distro\r\n", &args).unwrap();
        assert_eq!(e, ExecError::Git { message: "WSL: no distro".into(), code: -1, stderr: "WSL: no distro".into(), args: args.clone() });
        // stderr 가 있으면 git 이 낸 오류다
        assert!(failure_error(-1, "fatal: x", b"", &args).is_none());
        assert!(failure_error(128, "", b"", &args).is_none());
    }
}
