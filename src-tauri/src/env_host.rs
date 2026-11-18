//! 저장소 추가 창의 환경 채널 5개(Phase 6b PR 8)의 Rust 쪽 처리: `env:wslDistros`·`env:sshHosts`·`env:test`·`env:listDir`·`repos:scan`.
//! 라우터([`crate::router::Router`])가 `ENV_CHANNELS`(router.rs)를 이 모듈로 넘기면 `src/main/ipc/handlers.ts`(`checkEnvInput`·핸들러)와 `RepoService`(`scanRepos`·`testEnv`·
//! `listRemoteDir`)와 같은 입력 검증·결과·오류 봉투(`{ok:false,error,code:"ERROR"}`)를 돌려준다. 설정(ConfigStore)은 읽지도 쓰지도 않는다 — 입력과 `~/.ssh/config` 만 본다.
//!
//! **PR 4a·4b 의 함수만 쓴다**(계약): WSL 은 `resolve_env_checked_with`(형식 + 설치 목록 대조 — 운영은 레지스트리 관문 뒤의 `list_distros`, `wsl.exe` 를 직접 부르지 않는다),
//! 경로·호스트 검사는 `mirror::{check_path,check_ssh_host,check_listdir_path}` 의 결과값만, SSH 는 `SshPool::hold`(저장 전 연결을 다른 미러 변경이 끊지 않게)와 `test_ssh_env`·SFTP `list_dirs`,
//! 로컬·WSL 폴더 찾기는 `scan_local_repos`(호출 전에 `check_path`, 블로킹이라 `spawn_blocking`).
//!
//! **호스트 키는 `known_hosts` 로만 검증한다(TOFU 없음, 결정 D3)**: 모르는 호스트·바뀐 키는 실행기가 거부하고 오류에 지문이 든다. 안전한 호스트 문자열일 때만 `ssh <별칭>` 안내가 붙는다
//! (`with_host_key_hint`). 이 모듈은 `known_hosts` 를 쓰지 않는다. 설정에 없는 SSH 호스트는 연결하기 전에 네이티브 확인 창으로 묻는다(승인은 프로세스 메모리, 취소가 기본).
//!
//! 대화상자·WSL 목록·실행기·`~/.ssh/config` 는 **포트로 주입**한다([`EnvHost`]) — 시험은 가짜를 달아 실제 창·`wsl.exe`·사용자 `~/.ssh` 를 건드리지 않는다.
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::git::exec::ssh::config::{list_hosts, load_ssh_config, local_user_name, process_env, Ctx, RealFs};
use crate::git::exec::ssh::{SftpError, SshExecutor, SshPool};
use crate::git::exec::wsl_paths::{unc_from_wsl, wsl_from_unc};
use crate::git::exec::{ExecError, GitExecutor, RunOpts};
use crate::git::fetch::js_trim;
use crate::mirror::{check_listdir_path, check_path, check_ssh_host};
use crate::repos::env::{check_wsl_distro_name, is_format_char, remote_relative, resolve_env_checked_with, test_git_env, test_ssh_env, with_host_key_hint, EnvInput, ResolvedEnv, ENV_TEST_TIMEOUT};
use crate::repos::scan::{scan_from_listing, scan_local_repos, RemoteDirListing, RepoScanResult};

const BAD_INPUT: &str = "잘못된 입력입니다";
const BAD_PATH: &str = "잘못된 경로입니다";
const BAD_GROUP: &str = "잘못된 그룹 이름입니다";
const CANCELED: &str = "연결을 취소했습니다";
const NOT_INSTALLED: &str = "설치된 WSL 배포판이 아닙니다";
const FAILED: &str = "처리하지 못했습니다";
/// 그룹 이름 상한(UTF-16 단위, `handlers.ts`)
const MAX_GROUP_LEN: usize = 200;

/// `~/.ssh/config` 의 구체 별칭 하나(TS `SshHostInfo`). `user` 는 설정에 없으면 로컬 사용자 이름이라 항상 있다(TS `resolveHost` 도 그렇다)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostInfo {
    pub alias: String,
    pub host_name: String,
    pub user: String,
    pub port: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
}

/// 설치된 WSL 배포판 목록(운영: `wsl::list_distros` — 레지스트리 관문 뒤에서만 `wsl.exe` 를 부른다)
pub type DistrosFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> + Send + Sync>;
/// `~/.ssh/config` 의 호스트 목록(파일을 읽는 블로킹 호출)
pub type SshHostsFn = Arc<dyn Fn() -> Vec<SshHostInfo> + Send + Sync>;
/// 설정에 없는 SSH 호스트의 확인 창. 인자는 메시지 상자 인자(`dialogs::message_box` 와 같은 모양)이고 값은 "연결"을 눌렀는가(실패·닫기·Esc·취소는 false). 막는 호출이다
pub type ConfirmSsh = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
/// local·wsl 환경의 git 실행기(`kind` 는 `local`·`wsl`, `distro` 는 wsl 일 때의 배포판)
pub type ExecFor = Arc<dyn Fn(&str, Option<&str>) -> GitExecutor + Send + Sync>;

/// 환경 채널의 포트. 라우터가 하나만 가진다. 복제하면 같은 승인 기억을 본다
#[derive(Clone)]
pub struct EnvHost {
    pub distros: DistrosFn,
    pub ssh_hosts: SshHostsFn,
    pub confirm_ssh: ConfirmSsh,
    pub exec_for: ExecFor,
    /// 이번 실행 동안 사용자가 연결을 승인한 설정 밖 SSH 호스트(TS `approvedSshHosts`). 앱을 다시 켜면 다시 묻는다
    approved: Arc<Mutex<HashSet<String>>>,
}

impl EnvHost {
    pub fn new(distros: DistrosFn, ssh_hosts: SshHostsFn, confirm_ssh: ConfirmSsh, exec_for: ExecFor) -> Self {
        EnvHost { distros, ssh_hosts, confirm_ssh, exec_for, approved: Arc::default() }
    }
}

/// 홈의 `~/.ssh/config` 에서 구체 별칭을 읽는다(TS `listSshHosts`). 파일이 없으면 빈 목록이다
pub fn ssh_hosts_for_home(home: &str) -> Vec<SshHostInfo> {
    let config_path = Path::new(home).join(".ssh").join("config").to_string_lossy().into_owned();
    let config = load_ssh_config(&config_path, home, &RealFs);
    let user = local_user_name();
    let ctx = Ctx { local_user: &user, env: &process_env };
    list_hosts(&config, &ctx)
        .into_iter()
        .map(|h| SshHostInfo { alias: h.alias, host_name: h.host_name, user: h.user, port: h.port, proxy_jump: h.proxy_jump })
        .collect()
}

/// 확인 창의 메시지 상자 인자(사이드카 `electronShim` 이 `handlers.ts` 의 옵션으로 만드는 것과 같다: kind `info`·본문은 message 와 detail 을 빈 줄로 이은 것·`[연결][취소]`·취소가 기본).
/// 화면에 찍는 호스트 이름은 서식 문자(bidi 제어·제로폭)를 `\u{…}` 로 바꿔 보여 준다 — 연결하는 값은 바뀌지 않는다
pub fn ssh_confirm_arg(host: &str) -> Value {
    let shown: String = host.chars().map(|c| if is_format_char(c) { format!("\\u{{{:04X}}}", c as u32) } else { c.to_string() }).collect();
    json!({
        "kind": "info",
        "title": "SSH 연결 확인",
        "message": format!("~/.ssh/config 에 없는 호스트입니다.\n{shown} 에 연결할까요?\n\n키 또는 ssh-agent 로 인증하며, 호스트 키가 known_hosts 에 등록돼 있어야 합니다."),
        "buttons": ["연결", "취소"],
        "cancelId": 1,
    })
}

/// 이 호출을 사이드카가 아니라 Rust 가 SSH 풀로 처리해야 하는가(ssh 종류 — `env:listDir` 는 항상 ssh). 풀이 없으면(`ANTLER_RUST_SSH=0`) 라우터가 이 호출을 사이드카로 보낸다(계획 §13 C4).
/// local·wsl 과 `env:wslDistros`·`env:sshHosts` 는 풀과 무관하다
pub fn needs_ssh(channel: &str, args: &[Option<Value>]) -> bool {
    match channel {
        "env:listDir" => true,
        "env:test" | "repos:scan" => args.first().and_then(Option::as_ref).and_then(|a| a.get("kind")).and_then(Value::as_str) == Some("ssh"),
        _ => false,
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// 막는 호출을 런타임 스레드를 붙잡지 않게 따로 돌린다
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|_| FAILED.to_string())
}

impl EnvHost {
    /// 설정에 없는 SSH 호스트면 연결 확인 창을 띄운다(TS `checkEnvInput` 의 ssh 갈래). `host` 는 `check_ssh_host` 가 돌려준 값이다.
    /// 승인 집합·`~/.ssh/config` 별칭에 있으면 묻지 않는다. 승인(응답 "연결")이어야만 집합에 넣는다
    async fn approve_ssh_host(&self, host: &str) -> Result<(), String> {
        if self.approved.lock().unwrap_or_else(PoisonError::into_inner).contains(host) {
            return Ok(());
        }
        let list = self.ssh_hosts.clone();
        if blocking(move || list()).await?.iter().any(|h| h.alias == host) {
            return Ok(());
        }
        let (confirm, arg) = (self.confirm_ssh.clone(), ssh_confirm_arg(host));
        if !blocking(move || confirm(&arg)).await.unwrap_or(false) {
            return Err(CANCELED.to_string());
        }
        self.approved.lock().unwrap_or_else(PoisonError::into_inner).insert(host.to_string());
        Ok(())
    }

    /// 환경·경로 입력 검증(TS `checkEnvInput`) 뒤 해석한 환경을 돌려준다. 순서는 TS 와 같다: kind → (경로가 있으면) 경로·그룹 → WSL 설치 확인 / SSH 호스트 검사·확인 창.
    /// **저장·실행으로 이어지는 해석은 `resolve_env_checked_with` 하나다**(WSL 은 형식 + 설치 목록 대조)
    pub(crate) async fn check_env_input(&self, input: Option<&Value>, with_path: bool) -> Result<ResolvedEnv, String> {
        let obj = input.and_then(Value::as_object).ok_or(BAD_INPUT)?;
        let kind = match obj.get("kind").and_then(Value::as_str) {
            Some(k @ ("local" | "wsl" | "ssh")) => k,
            _ => return Err(BAD_INPUT.to_string()),
        };
        let mut path = "";
        if with_path {
            let p = obj.get("path").and_then(Value::as_str).ok_or(BAD_PATH)?;
            check_path(kind, p)?;
            if let Some(g) = obj.get("group") {
                if !g.as_str().is_some_and(|g| utf16_len(g) <= MAX_GROUP_LEN) {
                    return Err(BAD_GROUP.to_string());
                }
            }
            path = p;
        }
        let distro = obj.get("distro").and_then(Value::as_str);
        let ssh_host = if kind == "ssh" {
            let host = check_ssh_host(obj.get("sshHost").and_then(Value::as_str))?;
            self.approve_ssh_host(&host).await?;
            Some(host)
        } else {
            None
        };
        let env_input = EnvInput { kind, path, distro, ssh_host: ssh_host.as_deref() };
        let list = self.distros.clone();
        resolve_env_checked_with(&env_input, move || list()).await.map_err(|e| with_distro_name(e, path, distro))
    }
}

/// 설치 확인 실패에 TS 처럼 이름을 붙인다(`설치된 WSL 배포판이 아닙니다: <이름|(없음)>`). 형식 검사를 통과하지 못한 이름(서식·제어 문자 등)은 그대로 되돌리지 않는다
fn with_distro_name(error: String, path: &str, distro: Option<&str>) -> String {
    if error != NOT_INSTALLED {
        return error;
    }
    match wsl_from_unc(js_trim(path)).map(|(d, _)| d).or_else(|| distro.map(str::to_string)) {
        None => format!("{NOT_INSTALLED}: (없음)"),
        Some(name) if check_wsl_distro_name(&name).is_ok() => format!("{NOT_INSTALLED}: {name}"),
        Some(_) => error,
    }
}

/// SSH 환경의 풀 실행기를 잡는다(저장 전 호스트라 `hold` — 다른 미러 변경이 끊지 않고, 끝나면 미러에 없는 호스트는 닫힌다)
fn ssh_pool(pool: Option<&Arc<SshPool>>) -> Result<&Arc<SshPool>, String> {
    pool.ok_or_else(|| "SSH 를 사용할 수 없습니다".to_string())
}

/// SFTP 폴더 목록(TS `listRemoteDir`): 25초 제한. 호스트 키 거부에는 `ssh <별칭>` 안내를 붙인다
async fn list_remote_dir(pool: &Arc<SshPool>, host: &str, path: &str) -> Result<RemoteDirListing, SftpOrText> {
    let guard = pool.hold(host);
    list_with(guard.executor(), host, path).await
}

async fn list_with(exec: &SshExecutor, host: &str, path: &str) -> Result<RemoteDirListing, SftpOrText> {
    match tokio::time::timeout(ENV_TEST_TIMEOUT, exec.list_dirs(path, None)).await {
        Err(_) => Err(SftpOrText::Text(format!("{}초 안에 응답이 없습니다(네트워크·방화벽 상태를 확인하세요)", ENV_TEST_TIMEOUT.as_secs()))),
        Ok(Err(e)) => Err(SftpOrText::Sftp(e, host.to_string())),
        Ok(Ok(listing)) => Ok(listing),
    }
}

/// 폴더 목록의 실패. `repos:scan` 은 `Unavailable` 을 따로 다룬다
enum SftpOrText {
    Text(String),
    Sftp(SftpError, String),
}

impl SftpOrText {
    fn message(&self) -> String {
        match self {
            SftpOrText::Text(t) => t.clone(),
            SftpOrText::Sftp(e, host) => with_host_key_hint(host, &e.to_string()),
        }
    }
}

/// `repos:scan`(TS `scanRepos`): 고른 폴더가 저장소(작업 트리 안)면 그렇다고 알리고, 아니면 바로 아래 하위 저장소를 찾는다. 읽기 전용이다
async fn scan_repos(host: &EnvHost, pool: Option<&Arc<SshPool>>, input: Option<&Value>) -> Result<Value, String> {
    let env = host.check_env_input(input, true).await?;
    let path = js_trim(input.and_then(|i| i.get("path")).and_then(Value::as_str).unwrap_or("")).to_string();
    let rel = remote_relative(env.kind, &env.path);
    // 저장소 안의 하위 폴더(서브모듈이 든 폴더 등)를 골랐을 때 하위 저장소 목록 대신 그 저장소를 추가하게 한다 — git 이 저장소가 아니라고 하면(종료 코드 ≠ 0) 아래로 간다. 그 밖의 오류는 올린다
    let probe = |r: Result<crate::git::exec::RunResult, ExecError>| match r {
        Ok(r) => Ok(r.code == 0),
        Err(ExecError::Git { .. }) => Ok(false),
        Err(e) => Err(e),
    };
    let to_json = |r: RepoScanResult| serde_json::to_value(r).unwrap_or(Value::Null);
    if env.kind == "ssh" {
        let ssh_host = env.target.clone().unwrap_or_default();
        let guard = ssh_pool(pool)?.hold(&ssh_host);
        let inside = probe(guard.executor().run(&rel, &["rev-parse", "--show-toplevel"], &RunOpts::default()).await).map_err(|e| with_host_key_hint(&ssh_host, &e.to_string()))?;
        if inside {
            return Ok(to_json(RepoScanResult { path, is_repo: true, repos: Vec::new(), truncated: false }));
        }
        return match list_with(guard.executor(), &ssh_host, &env.path).await {
            Ok(listing) => {
                Ok(to_json(scan_from_listing(&listing)))
            }
            // SFTP 를 막은 서버는 찾기 없이 그 경로를 그대로 추가하게 둔다
            Err(SftpOrText::Sftp(SftpError::Unavailable(_), _)) => Ok(to_json(RepoScanResult { path, is_repo: false, repos: Vec::new(), truncated: false })),
            Err(e) => Err(e.message()),
        };
    }
    // 고른 폴더(로컬)가 링크로 원격에 이어지면 `git -C`·`read_dir` 가 따라가는 순간 SMB 인증(NTLM)이 샌다 — 실행기를 만들기도 전에 구성 요소 단위로 확인한다.
    // WSL 공유 경로(`\\wsl.localhost\…`)는 리눅스 쪽 링크라 Windows 가 따라가지 못하므로 local 종류만 본다
    if cfg!(windows) && env.kind == "local" {
        let root = path.clone();
        blocking(move || crate::fsguard::check_local_root(&root, crate::fsguard::is_remote_link_target)).await??;
    }
    let exec = (host.exec_for)(env.kind, env.target.as_deref());
    if probe(exec.run(&rel, &["rev-parse", "--show-toplevel"], &RunOpts::default()).await).map_err(|e| e.to_string())? {
        return Ok(to_json(RepoScanResult { path, is_repo: true, repos: Vec::new(), truncated: false }));
    }
    // WSL 은 공유 경로(`\\wsl.localhost\…`)로 읽는다(결과도 공유 경로 — 추가할 때 WSL 경로로 바뀐다)
    let root = if env.kind == "wsl" && wsl_from_unc(&path).is_none() { unc_from_wsl(env.target.as_deref().unwrap_or(""), &env.path) } else { path };
    let scanned = blocking(move || scan_local_repos(&root)).await??;
    Ok(to_json(scanned))
}

fn ok_data(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err(message: &str) -> Value {
    json!({ "ok": false, "error": message, "code": "ERROR" })
}

/// 환경 채널 하나를 처리해 IpcEnvelope 를 돌려준다. `args` 는 표식을 되돌린 인자 배열(`None` = undefined)이다. `pool` 은 SSH 풀(ssh 종류는 라우터가 풀이 있을 때만 이쪽으로 보낸다)
pub async fn env_envelope(host: EnvHost, pool: Option<Arc<SshPool>>, channel: &str, args: Vec<Option<Value>>) -> Value {
    let a0 = args.first().and_then(Option::as_ref);
    let result: Result<Value, String> = match channel {
        "env:wslDistros" => Ok(json!((host.distros)().await)),
        "env:sshHosts" => {
            let list = host.ssh_hosts.clone();
            blocking(move || list()).await.map(|hosts| serde_json::to_value(hosts).unwrap_or(Value::Null))
        }
        "env:test" => env_test(&host, pool.as_ref(), a0).await.map(Value::String),
        "env:listDir" => env_list_dir(&host, pool.as_ref(), a0).await,
        "repos:scan" => scan_repos(&host, pool.as_ref(), a0).await,
        other => Err(format!("알 수 없는 채널: {other}")),
    };
    match result {
        Ok(data) => ok_data(data),
        Err(e) => err(&e),
    }
}

/// 연결 테스트(TS `env:test` + `testEnv`): 그 환경에서 git 버전을 돌려준다
async fn env_test(host: &EnvHost, pool: Option<&Arc<SshPool>>, input: Option<&Value>) -> Result<String, String> {
    let env = host.check_env_input(input, false).await?;
    if env.kind == "ssh" {
        return test_ssh_env(ssh_pool(pool)?, env.target.as_deref().unwrap_or("")).await;
    }
    test_git_env(&(host.exec_for)(env.kind, env.target.as_deref())).await
}

/// SSH 원격 폴더 목록(TS `env:listDir`): 경로 검사가 호스트 검사보다 먼저다. 경로가 문자열이 아니면 빈 문자열(홈)이다
async fn env_list_dir(host: &EnvHost, pool: Option<&Arc<SshPool>>, input: Option<&Value>) -> Result<Value, String> {
    let obj = input.and_then(Value::as_object);
    let path = obj.and_then(|o| o.get("path")).and_then(Value::as_str).unwrap_or("");
    check_listdir_path(path)?;
    // 호스트 검증은 연결 테스트와 같다(설정에 없는 호스트면 사용자 확인)
    let mut ssh = Map::new();
    ssh.insert("kind".to_string(), json!("ssh"));
    if let Some(h) = obj.and_then(|o| o.get("sshHost")) {
        ssh.insert("sshHost".to_string(), h.clone());
    }
    let env = host.check_env_input(Some(&Value::Object(ssh)), false).await?;
    let ssh_host = env.target.unwrap_or_default();
    let listing = list_remote_dir(ssh_pool(pool)?, &ssh_host, path).await.map_err(|e| e.message())?;
    Ok(serde_json::to_value(listing).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_가_필요한_호출은_ssh_종류와_env_list_dir_뿐이다() {
        let a = |v: Value| vec![Some(v)];
        assert!(needs_ssh("env:listDir", &[]));
        assert!(needs_ssh("env:listDir", &a(json!({ "sshHost": "x" }))));
        assert!(needs_ssh("env:test", &a(json!({ "kind": "ssh" }))));
        assert!(needs_ssh("repos:scan", &a(json!({ "kind": "ssh", "path": "~" }))));
        for v in [json!({ "kind": "local" }), json!({ "kind": "wsl" }), json!({}), json!(null), json!("ssh"), json!({ "kind": 1 })] {
            assert!(!needs_ssh("env:test", &a(v.clone())) && !needs_ssh("repos:scan", &a(v)));
        }
        assert!(!needs_ssh("env:test", &[]));
        assert!(!needs_ssh("env:wslDistros", &[]) && !needs_ssh("env:sshHosts", &[]));
    }

    #[test]
    fn 확인_창_인자는_사이드카_모양이고_서식_문자는_드러난다() {
        let a = ssh_confirm_arg("host\u{202E}x");
        assert_eq!(a["kind"], "info");
        assert_eq!(a["title"], "SSH 연결 확인");
        assert_eq!(a["buttons"], json!(["연결", "취소"]));
        assert_eq!(a["cancelId"], 1);
        let m = a["message"].as_str().unwrap();
        assert!(m.starts_with("~/.ssh/config 에 없는 호스트입니다.\nhost\\u{202E}x 에 연결할까요?\n\n키 또는 ssh-agent 로"), "{m}");
        assert!(!m.contains('\u{202E}'));
    }

    #[test]
    fn 설치_확인_실패에는_이름이_붙고_형식_오류는_그대로다() {
        assert_eq!(with_distro_name(NOT_INSTALLED.into(), "", Some("Ubuntu")), "설치된 WSL 배포판이 아닙니다: Ubuntu");
        assert_eq!(with_distro_name(NOT_INSTALLED.into(), "", None), "설치된 WSL 배포판이 아닙니다: (없음)");
        assert_eq!(with_distro_name(NOT_INSTALLED.into(), r"\\wsl.localhost\Debian\home", Some("Ubuntu")), "설치된 WSL 배포판이 아닙니다: Debian");
        // 서식 문자가 든 이름은 오류 문구에 되돌려 싣지 않는다
        assert_eq!(with_distro_name(NOT_INSTALLED.into(), "", Some("a\u{202E}b")), NOT_INSTALLED);
        assert_eq!(with_distro_name("다른 오류".into(), "", Some("Ubuntu")), "다른 오류");
    }

    #[test]
    fn 호스트_정보는_ts_모양으로_직렬화된다() {
        let h = SshHostInfo { alias: "a".into(), host_name: "h".into(), user: "u".into(), port: 22, proxy_jump: None };
        assert_eq!(serde_json::to_string(&h).unwrap(), r#"{"alias":"a","hostName":"h","user":"u","port":22}"#);
        let j = SshHostInfo { proxy_jump: Some("b".into()), ..h };
        assert_eq!(serde_json::to_string(&j).unwrap(), r#"{"alias":"a","hostName":"h","user":"u","port":22,"proxyJump":"b"}"#);
    }
}
