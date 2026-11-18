//! WSL 배포판 안에서 git 을 실행한다(`src/main/executors/WslExecutor.ts`). repo 경로는 리눅스 경로(`/home/me/repo`).
//!
//! `wsl.exe --exec` 는 리눅스 셸을 거치지 않으므로 인자 인용·인젝션 문제가 없다.
//! `--cd ~` 로 배포판 사용자 홈에서 시작하므로 repo 경로는 홈 기준 상대 경로(`proj`)도 된다.
//! Windows 환경변수는 WSLENV 없이는 WSL 로 넘어가지 않아 [`GIT_ENV`] 를 `env` 명령으로 전달한다.
//!
//! 취소: Job Object 종료가 wsl.exe 를 죽이면 리눅스 쪽 git 도 끝나는지는 `tests/exec.rs` 의 WSL 통합 테스트에서 실측한다.

use super::common::{build_git_args, merged_env, to_strings, FetchEnv, RunOpts, RunResult, Spec};
use super::error::ExecError;
use super::process::{run_process, stream_process, GitStream};
use crate::git::fetch::js_trim as js_trim_exact;
use crate::git::parsers::js_trim;
use std::time::Duration;

/// wsl.exe 가 내보내는 자체 메시지의 인코딩을 UTF-8 로 고정한다. 기본은 UTF-16LE 지만 사용자가 `WSL_UTF8=1` 을
/// 설정해 두었을 수도 있어, 어느 쪽인지 추측하지 않도록 항상 지정한다(파싱은 두 인코딩 모두 받아들인다).
/// wsl.exe **자신**의 환경이다 — 리눅스 쪽 git 의 환경은 [`wsl_argv`] 의 `env` 로 준다.
pub const WSL_SPAWN_ENV: [(&str, &str); 1] = [("WSL_UTF8", "1")];

/// wsl.exe 에 넘길 argv: `-d <distro> --cd ~ --exec env K=V… git -C <repo> <전역 인자> <args…>`
pub fn wsl_argv<S: AsRef<str>>(distro: &str, repo_path: &str, args: &[S]) -> Vec<String> {
    wsl_argv_with(distro, repo_path, args, None)
}

/// [`wsl_argv`] 에 fetch 환경을 얹은 것. `env K=V…` 순서는 JS `{ ...GIT_ENV, ...fetchEnv }` 와 같다([`merged_env`])
pub fn wsl_argv_with<S: AsRef<str>>(distro: &str, repo_path: &str, args: &[S], fetch: Option<&FetchEnv>) -> Vec<String> {
    let mut v: Vec<String> = ["-d", distro, "--cd", "~", "--exec", "env"].iter().map(|s| (*s).to_string()).collect();
    v.extend(merged_env(fetch).into_iter().map(|(k, val)| format!("{k}={val}")));
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

/// `wsl.exe -l -q` 의 argv(고정). 배포판 이름은 인자로 들어가지 않는다
pub const WSL_LIST_ARGV: [&str; 2] = ["-l", "-q"];
/// 배포판 목록 조회의 제한 시간(TS `LIST_TIMEOUT_MS`)
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
/// 레지스트리 열거 상한 — 배포판이 수백 개일 리 없다(깨진 키가 무한히 돌지 않게)
#[cfg(windows)]
const MAX_REGISTRY_SUBKEYS: u32 = 256;

/// `wsl.exe -l -q` 출력(UTF-16LE·BOM·`\r`·NUL 이 섞임, 또는 `WSL_UTF8=1` 의 UTF-8)에서 사용자 배포판 이름만 뽑는다(TS `parseWslList`).
/// 공백 제거는 JS `trim` 이다(`crate::git::fetch::js_trim` — `parsers::js_trim` 은 U+0085 를 지워 다르다)
pub fn parse_wsl_list(buf: &[u8]) -> Vec<String> {
    decode_wsl_text(buf).split('\n').map(js_trim_exact).filter(|s| !s.is_empty() && !s.starts_with("docker-desktop")).map(str::to_string).collect()
}

/// `reg query …\Lxss /s /v DistributionName` 출력에서 배포판 이름만 뽑는다(TS `parseLxssQuery`).
/// 운영 코드는 reg.exe 를 띄우지 않고 레지스트리를 직접 읽으므로 이 함수는 출력 형식을 해석하는 골든(TS 와의 대조) 용이다.
/// 정규식 `^\s*DistributionName\s+REG_SZ\s+(.*?)\s*$` 를 손으로 푼 것이다 — `.` 는 줄 끝 문자와 맞지 않는다
pub fn parse_lxss_query(text: &str) -> Vec<String> {
    use crate::git::fetch::is_line_terminator;
    use crate::git::scope::is_js_space;
    // `split(/\r?\n/)` — 짝 없는 `\r` 은 줄 안에 남는다
    let value_of = |line: &str| -> Option<String> {
        let rest = line.trim_start_matches(is_js_space).strip_prefix("DistributionName")?;
        let after = rest.trim_start_matches(is_js_space);
        if after.len() == rest.len() {
            return None;
        }
        let rest = after.strip_prefix("REG_SZ")?;
        let after = rest.trim_start_matches(is_js_space);
        if after.len() == rest.len() {
            return None;
        }
        let value = after.trim_end_matches(is_js_space);
        (!value.chars().any(is_line_terminator)).then(|| value.to_string())
    };
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).filter_map(value_of).filter(|s| !s.is_empty()).collect()
}

/// 등록된 배포판 이름: 레지스트리 `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss\{GUID}\DistributionName`(읽기만 한다 — 프로세스를 띄우지 않는다).
/// Windows 가 아니거나 키가 없으면 빈 목록
#[cfg(windows)]
pub fn registered_distro_names() -> Vec<String> {
    use windows::core::{w, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ};

    let mut root = HKEY::default();
    // SAFETY: 인자는 모두 이 함수의 지역 값이고, 성공했을 때만 `root` 를 쓰며 마지막에 닫는다
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss"), None, KEY_READ, &mut root) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0..MAX_REGISTRY_SUBKEYS {
        // 하위 키 이름은 GUID(38자)다 — 버퍼보다 긴 이름은 우리가 만든 키가 아니므로 건너뛴다
        let mut key = [0u16; 128];
        let mut key_len = key.len() as u32;
        // SAFETY: `key`·`key_len` 은 서로 맞는 크기의 지역 버퍼다
        let r = unsafe { RegEnumKeyExW(root, index, Some(PWSTR(key.as_mut_ptr())), &mut key_len, None, None, None, None) };
        if r == ERROR_MORE_DATA {
            continue;
        }
        if r != ERROR_SUCCESS {
            break;
        }
        key[key_len as usize] = 0;
        let mut value = [0u16; 512];
        let mut bytes = std::mem::size_of_val(&value) as u32;
        // SAFETY: `key` 는 위에서 NUL 로 끝냈고, `value`·`bytes` 는 서로 맞는 크기의 지역 버퍼다(RRF_RT_REG_SZ 는 NUL 종료를 보장한다)
        let got = unsafe { RegGetValueW(root, PCWSTR(key.as_ptr()), w!("DistributionName"), RRF_RT_REG_SZ, None, Some(value.as_mut_ptr().cast()), Some(&mut bytes)) };
        if got != ERROR_SUCCESS {
            continue;
        }
        let units = (bytes as usize / 2).min(value.len());
        let text = String::from_utf16_lossy(&value[..units]);
        let name = js_trim_exact(text.trim_end_matches('\0'));
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    // SAFETY: 위에서 연 핸들이다
    let _ = unsafe { RegCloseKey(root) };
    names
}

#[cfg(not(windows))]
pub fn registered_distro_names() -> Vec<String> {
    Vec::new()
}

/// 설치된 WSL 배포판 목록(TS `listWslDistros`). 레지스트리를 읽는 `registered` 와 `wsl.exe` 를 띄우는 `run` 을 주입한다.
///
/// **`registered` 에 사용자 배포판이 하나도 없으면 `run` 을 부르지 않는다** — WSL 을 지운 PC 에도 system32 의 `wsl.exe` 스텁이 남아,
/// 부르기만 하면 "WSL 설치" 창(키를 누르면 설치 시작)이 뜬다. TS 는 `docker-desktop` 만 있어도 부르지만, 여기서는 그것도 사용자 배포판이 아니라고 본다
/// (docker-desktop 만 있으면 어차피 결과가 비는데 스텁을 띄울 위험만 남는다).
/// `run` 은 [`WSL_LIST_ARGV`] 를 받아 stdout 바이트를 돌려준다(실패·시간 초과·0 이 아닌 종료는 `None`). 시험은 가짜 `run` 으로 argv 만 검증한다.
pub async fn list_distros_with<R, F, Fut>(registered: R, run: F) -> Vec<String>
where
    R: FnOnce() -> Vec<String>,
    F: FnOnce(&'static [&'static str]) -> Fut,
    Fut: std::future::Future<Output = Option<Vec<u8>>>,
{
    if !registered().iter().any(|n| !n.starts_with("docker-desktop")) {
        return Vec::new();
    }
    run(&WSL_LIST_ARGV).await.map(|out| parse_wsl_list(&out)).unwrap_or_default()
}

/// 운영: 레지스트리 관문 뒤에 `wsl.exe -l -q`(`WSL_UTF8=1`, 8초 제한). Windows 가 아니면 빈 목록
pub async fn list_distros() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    list_distros_with(registered_distro_names, |argv| async move {
        let spec = Spec {
            program: "wsl.exe".to_string(),
            argv: argv.iter().map(|a| (*a).to_string()).collect(),
            env: WSL_SPAWN_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            git_args: Vec::new(),
            wsl: true,
        };
        // 시간이 지나면 future 를 버린다 — 자식 가드가 프로세스 트리를 죽인다
        let r = tokio::time::timeout(LIST_TIMEOUT, run_process(&spec, &RunOpts::default())).await.ok()?.ok()?;
        (r.code == 0).then_some(r.stdout)
    })
    .await
}

#[derive(Clone)]
pub struct WslExecutor {
    distro: String,
}

impl WslExecutor {
    pub fn new(distro: impl Into<String>) -> Self {
        WslExecutor { distro: distro.into() }
    }

    /// 배포판 이름(캐시 키)
    pub fn distro(&self) -> &str {
        &self.distro
    }

    fn spec<S: AsRef<str>>(&self, repo_path: &str, args: &[S], fetch: Option<&FetchEnv>) -> Spec {
        Spec {
            program: "wsl.exe".to_string(),
            argv: wsl_argv_with(&self.distro, repo_path, args, fetch),
            env: WSL_SPAWN_ENV.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            git_args: to_strings(args),
            wsl: true,
        }
    }

    pub async fn run<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<RunResult, ExecError> {
        let r = run_process(&self.spec(repo_path, args, opts.fetch_env.as_ref()), opts).await?;
        // wsl.exe 자체가 실패하면 메시지를 stdout 으로 내고 stderr 는 비어 있다
        if is_wsl_failure(r.code) && js_trim(&r.stderr).is_empty() {
            let msg = wsl_message(&r.stdout);
            return Ok(RunResult { stdout: Vec::new(), stderr: msg, code: r.code });
        }
        Ok(r)
    }

    pub fn stream<S: AsRef<str>>(&self, repo_path: &str, args: &[S], opts: &RunOpts) -> Result<GitStream, ExecError> {
        stream_process(&self.spec(repo_path, args, None), opts)
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
    fn fetch_환경의_env_순서는_기존_키_제자리_교체_새_키_뒤() {
        let f = FetchEnv { allow_file: false, ssh_command: "ssh x".into() };
        let argv = wsl_argv_with("U", "r", &["fetch"], Some(&f));
        let end = argv.iter().position(|a| a == "git").unwrap();
        let envs = &argv[6..end];
        assert_eq!(
            envs,
            [
                "GIT_OPTIONAL_LOCKS=0",
                "GIT_TERMINAL_PROMPT=0",
                "GIT_LITERAL_PATHSPECS=1",
                "GIT_NO_LAZY_FETCH=1",
                "GIT_NO_REPLACE_OBJECTS=1",
                "GIT_ALLOW_PROTOCOL=https:http:ssh:git",
                "GIT_PAGER=cat",
                "LANG=C.UTF-8",
                "LC_ALL=C.UTF-8",
                "GIT_ASKPASS=",
                "SSH_ASKPASS_REQUIRE=never",
                "GCM_INTERACTIVE=never",
                "GIT_SSH_COMMAND=ssh x",
            ]
        );
    }

    #[test]
    fn wsl_메시지는_utf16le_도_utf8_도_받는다() {
        assert_eq!(wsl_message(&utf16le("\u{FEFF}There is no distribution with the supplied name.\r\n")), "WSL: There is no distribution with the supplied name.");
        assert_eq!(wsl_message(b"Wsl/Service/WSL_E_DISTRO_NOT_FOUND\r\n"), "WSL: Wsl/Service/WSL_E_DISTRO_NOT_FOUND");
        assert!(wsl_message(b"").starts_with("WSL 실행에 실패했습니다"));
    }

    fn utf16_bom(s: &str) -> Vec<u8> {
        let mut v = vec![0xff, 0xfe];
        v.extend(utf16le(s));
        v
    }

    #[test]
    fn 목록은_bom_crlf_가_섞인_utf16le_도_utf8_도_읽고_docker_desktop_과_빈_줄을_버린다() {
        assert_eq!(parse_wsl_list(&utf16_bom("Ubuntu-24.04\r\nDebian\r\n")), ["Ubuntu-24.04", "Debian"]);
        assert_eq!(parse_wsl_list(&utf16le("Ubuntu\r\n")), ["Ubuntu"], "BOM 이 없어도");
        assert_eq!(parse_wsl_list(&utf16_bom("docker-desktop\r\n\r\nUbuntu\r\ndocker-desktop-data\r\n  \r\n")), ["Ubuntu"]);
        assert_eq!(parse_wsl_list(&utf16_bom("My Distro\r\n")), ["My Distro"], "공백이 든 이름은 보존");
        assert_eq!(parse_wsl_list(b"Ubuntu\nDebian\n"), ["Ubuntu", "Debian"], "WSL_UTF8=1 의 UTF-8");
        assert!(parse_wsl_list(b"").is_empty());
    }

    #[test]
    fn lxss_질의_출력에서_배포판_이름만_뽑는다() {
        let out = [
            "",
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\\{0d1e7a1c-0000-0000-0000-000000000001}",
            "    DistributionName    REG_SZ    Ubuntu-22.04",
            "",
            "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\\{0d1e7a1c-0000-0000-0000-000000000002}",
            "    DistributionName    REG_SZ    docker-desktop",
            "",
            "검색 종료: 2 일치하는 항목이 있습니다.",
        ]
        .join("\r\n");
        assert_eq!(parse_lxss_query(&out), ["Ubuntu-22.04", "docker-desktop"]);
        assert!(parse_lxss_query("").is_empty());
        assert!(parse_lxss_query("HKEY_CURRENT_USER\\...\\Lxss\r\n    DefaultDistribution    REG_SZ    {x}").is_empty());
        assert!(parse_lxss_query("DistributionName REG_SZX Ubuntu").is_empty(), "REG_SZ 뒤에는 공백이 있어야 한다");
        assert!(parse_lxss_query("DistributionName REG_SZ   ").is_empty(), "값이 비면 버린다");
    }

    /// 실제 레지스트리는 읽기만 한다(wsl.exe 를 띄우지 않는다)
    #[test]
    fn 레지스트리_읽기는_패닉_없이_이름_목록을_돌려준다() {
        let names = registered_distro_names();
        assert!(names.iter().all(|n| !n.is_empty() && n == n.trim()), "{names:?}");
    }

    #[tokio::test]
    async fn 등록된_배포판이_없으면_wsl_exe_를_부르지_않는다() {
        for registered in [vec![], vec!["docker-desktop".to_string(), "docker-desktop-data".to_string()]] {
            let called = std::sync::atomic::AtomicBool::new(false);
            let got = list_distros_with(
                || registered,
                |_| async {
                    called.store(true, std::sync::atomic::Ordering::SeqCst);
                    Some(b"Ubuntu\n".to_vec())
                },
            )
            .await;
            assert!(got.is_empty());
            assert!(!called.load(std::sync::atomic::Ordering::SeqCst), "스텁 wsl.exe 가 설치 창을 띄운다");
        }
    }

    #[tokio::test]
    async fn 등록된_배포판이_있으면_고정_argv_로_부르고_결과를_해석한다() {
        let seen = std::sync::Mutex::new(Vec::<String>::new());
        let got = list_distros_with(
            || vec!["Ubuntu".to_string()],
            |argv| {
                seen.lock().unwrap().extend(argv.iter().map(|a| (*a).to_string()));
                async { Some(utf16_bom("docker-desktop\r\nUbuntu\r\n")) }
            },
        )
        .await;
        assert_eq!(*seen.lock().unwrap(), ["-l", "-q"]);
        assert_eq!(got, ["Ubuntu"]);
        // 실행 실패·시간 초과(None)는 빈 목록이다
        assert!(list_distros_with(|| vec!["Ubuntu".to_string()], |_| async { None }).await.is_empty());
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
