//! 저장소를 추가할 때의 환경 해석과, 저장하기 전의 SSH 연결 테스트(`src/main/services/RepoService.ts` 의 `resolveEnv`·`remoteRelative`·`testEnv`).
//!
//! 경로·호스트·배포판의 **형식 검사**는 `mirror::{check_path,check_ssh_host,check_distro}` 가 한다(TS `checkEnvInput`). 여기서는 검사를 통과한 입력을
//! 저장할 모양으로 바꾼다. 공백 제거는 JS `trim` 이다(`git::fetch::js_trim` — `parsers::js_trim` 은 U+0085 를 지워 다르다).

use std::sync::Arc;
use std::time::Duration;

use crate::git::exec::ssh::SshPool;
use crate::git::exec::wsl_paths::wsl_from_unc;
use crate::git::exec::RunOpts;
use crate::git::fetch::js_trim;

/// 연결 테스트의 제한 시간(TS `ENV_TEST_TIMEOUT_MS`)
pub const ENV_TEST_TIMEOUT: Duration = Duration::from_secs(25);

/// 추가 입력(TS `AddRepoInput` 에서 그룹을 뺀 것)
#[derive(Debug, Clone, Copy)]
pub struct EnvInput<'a> {
    pub kind: &'a str,
    pub path: &'a str,
    pub distro: Option<&'a str>,
    pub ssh_host: Option<&'a str>,
}

/// 해석한 환경. `target` 은 WSL 의 배포판·SSH 의 호스트이고 로컬은 `None`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEnv {
    pub kind: &'static str,
    pub target: Option<String>,
    pub path: String,
}

/// 입력의 환경과 경로를 정한다(TS `resolveEnv`). 로컬·WSL 로 고른 경로가 `\\wsl.localhost\…` 같은 WSL 공유 경로면 WSL 저장소로 바꾼다
/// (WSL 안에서 git 을 돌려야 9P 파일 공유를 거치지 않아 빠르고 줄바꿈·권한이 정확하다).
/// GitHub 원격 전용 저장소는 이 경로로 만들 수 없다(임의 경로를 github 소스에 올리지 못하게). 모르는 종류도 거부한다(TS 는 앞단 검사가 거른다)
pub fn resolve_env(input: &EnvInput<'_>) -> Result<ResolvedEnv, String> {
    let kind = match input.kind {
        "local" => "local",
        "wsl" => "wsl",
        "ssh" => "ssh",
        "github" => return Err("GitHub 원격 저장소는 여기서 추가할 수 없습니다".to_string()),
        _ => return Err("잘못된 저장소 종류입니다".to_string()),
    };
    let path = js_trim(input.path);
    if kind == "local" || kind == "wsl" {
        if let Some((distro, linux)) = wsl_from_unc(path) {
            return Ok(ResolvedEnv { kind: "wsl", target: Some(distro), path: linux });
        }
    }
    let target = match kind {
        "wsl" => input.distro.map(str::to_string),
        "ssh" => input.ssh_host.map(|h| js_trim(h).to_string()),
        _ => None,
    };
    Ok(ResolvedEnv { kind, target, path: path.to_string() })
}

/// WSL·SSH 에서 `~/` 로 시작하는 경로는 셸 없이 실행하므로 펼쳐지지 않는다(TS `remoteRelative`).
/// 두 실행기 모두 홈 디렉터리에서 git 을 실행하므로(WSL `--cd ~`, SSH exec 기본 위치) 홈 기준 상대 경로로 바꾼다. 로컬은 그대로다
pub fn remote_relative(kind: &str, path: &str) -> String {
    if kind == "local" {
        return path.to_string();
    }
    if path == "~" {
        return ".".to_string();
    }
    match path.strip_prefix("~/") {
        Some("") => ".".to_string(),
        Some(rest) => rest.to_string(),
        None => path.to_string(),
    }
}

/// 호스트 키 거부 오류에 붙일 안내를 만든다. Antler 는 `known_hosts` 에 쓰지 않는다(TOFU 금지) — 사용자가 터미널에서 `ssh <별칭>` 을 한 번 실행해
/// 지문을 확인·수락해야 한다. 오류 문구 자체(지문 포함)는 실행기가 만든 그대로 두고 그 뒤에 명령만 덧붙인다. 호스트 키와 무관한 오류는 그대로 돌려준다
pub fn with_host_key_hint(host: &str, message: &str) -> String {
    if message.contains("알 수 없는 호스트입니다") || message.contains("호스트 키가 known_hosts 의 기록과 다릅니다") {
        format!("{message} 확인 명령: {}", ssh_cli_command(host))
    } else {
        message.to_string()
    }
}

/// `[user@]host[:port]` 표기를 ssh CLI 명령으로(`:port` 는 `-p`). 별칭이면 그대로 `ssh 별칭` 이다
fn ssh_cli_command(host: &str) -> String {
    if let Some((name, port)) = host.rsplit_once(':') {
        if !name.is_empty() && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && !name.contains(':') {
            return format!("ssh -p {port} {name}");
        }
    }
    format!("ssh {host}")
}

/// SSH 연결 테스트(TS `testEnv` 의 ssh 갈래): 그 호스트에서 git 버전을 돌려준다(인증·호스트 키 문제를 저장 전에 드러낸다).
/// 풀에서 연결을 **잡아** 쓰므로(`SshPool::hold`) 같은 새 호스트를 다루는 다른 요청이나 미러 변경(`retain`)이 연결을 끊지 않고,
/// 끝나면 저장되지 않은 호스트의 연결은 닫힌다. 호스트 키가 모르는 키·바뀐 키면 거부하고 지문과 `ssh <별칭>` 안내를 오류에 담는다
pub async fn test_ssh_env(pool: &Arc<SshPool>, host: &str) -> Result<String, String> {
    let guard = pool.hold(host);
    let opts = RunOpts::default();
    let run = guard.executor().run(".", &["--version"], &opts);
    match tokio::time::timeout(ENV_TEST_TIMEOUT, run).await {
        Err(_) => Err(format!("{}초 안에 응답이 없습니다(네트워크·방화벽·WSL 상태를 확인하세요)", ENV_TEST_TIMEOUT.as_secs())),
        Ok(Err(e)) => Err(with_host_key_hint(host, &e.to_string())),
        Ok(Ok(r)) if r.code != 0 => {
            let stderr = js_trim(&r.stderr);
            Err(if stderr.is_empty() { format!("git 실행 실패({})", r.code) } else { stderr.to_string() })
        }
        Ok(Ok(r)) => Ok(js_trim(&String::from_utf8_lossy(&r.stdout)).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(kind: &'a str, path: &'a str, distro: Option<&'a str>, host: Option<&'a str>) -> EnvInput<'a> {
        EnvInput { kind, path, distro, ssh_host: host }
    }

    #[test]
    fn 경로는_js_trim_한_값이고_wsl_공유_경로는_wsl_저장소가_된다() {
        let r = resolve_env(&input("local", "  C:\\work\\repo \n", None, None)).unwrap();
        assert_eq!(r, ResolvedEnv { kind: "local", target: None, path: "C:\\work\\repo".into() });
        // U+0085 는 JS trim 이 지우지 않는다
        assert_eq!(resolve_env(&input("local", "\u{85}a", None, None)).unwrap().path, "\u{85}a");
        for kind in ["local", "wsl"] {
            let r = resolve_env(&input(kind, r"\\wsl.localhost\Ubuntu\home\me\repo", Some("Other"), None)).unwrap();
            assert_eq!(r, ResolvedEnv { kind: "wsl", target: Some("Ubuntu".into()), path: "/home/me/repo".into() });
        }
        assert_eq!(resolve_env(&input("wsl", "/home/me", Some("Debian"), None)).unwrap().target.as_deref(), Some("Debian"));
        assert_eq!(resolve_env(&input("wsl", "/home/me", None, None)).unwrap().target, None);
    }

    #[test]
    fn ssh_호스트는_trim_하고_ssh_는_wsl_공유_경로를_바꾸지_않는다() {
        let r = resolve_env(&input("ssh", " ~/proj ", None, Some("  me@host  "))).unwrap();
        assert_eq!(r, ResolvedEnv { kind: "ssh", target: Some("me@host".into()), path: "~/proj".into() });
        let unc = resolve_env(&input("ssh", r"\\wsl.localhost\Ubuntu\x", None, Some("h"))).unwrap();
        assert_eq!((unc.kind, unc.path.as_str()), ("ssh", r"\\wsl.localhost\Ubuntu\x"));
    }

    #[test]
    fn github_와_모르는_종류는_거부한다() {
        assert_eq!(resolve_env(&input("github", "x", None, None)).unwrap_err(), "GitHub 원격 저장소는 여기서 추가할 수 없습니다");
        assert!(resolve_env(&input("ftp", "x", None, None)).is_err());
    }

    #[test]
    fn 홈_상대_경로는_셸_없이_쓰도록_바꾼다() {
        assert_eq!(remote_relative("local", "~/x"), "~/x");
        assert_eq!(remote_relative("ssh", "~"), ".");
        assert_eq!(remote_relative("wsl", "~/"), ".");
        assert_eq!(remote_relative("ssh", "~/a/b"), "a/b");
        assert_eq!(remote_relative("ssh", "/abs"), "/abs");
        assert_eq!(remote_relative("ssh", "~user/x"), "~user/x");
    }

    #[test]
    fn 호스트_키_거부에는_ssh_명령_안내를_붙이고_다른_오류는_그대로_둔다() {
        let unknown = "알 수 없는 호스트입니다: example.com (키 지문 SHA256:abc). 터미널에서 ssh CLI 로 …";
        let out = with_host_key_hint("work", unknown);
        assert!(out.starts_with(unknown) && out.ends_with("확인 명령: ssh work"), "{out}");
        assert!(with_host_key_hint("me@h:2222", unknown).ends_with("ssh -p 2222 me@h"));
        assert!(with_host_key_hint("h", "호스트 키가 known_hosts 의 기록과 다릅니다: h (받은 키 지문 SHA256:x). …").contains("확인 명령: ssh h"));
        assert_eq!(with_host_key_hint("h", "인증에 실패했습니다"), "인증에 실패했습니다");
    }
}
