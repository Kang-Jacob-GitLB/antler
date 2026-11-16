//! fetch 실행: 저장소 설정 검사(`inspectConfig`)와 `git fetch` 호출(`runFetch`), git 버전 조회. `src/main/git/fetch.ts` 의 이식이다.
//! 판정 로직은 [`super::guard`](PR A)에 있고 여기서는 git 을 부르는 순서와 인자만 정한다. 앱에서는 상태 기계(`service.rs`)가 부른다.
//!
//! 이식 규칙(어기면 보안 모델이 약해진다):
//! - **환경변수는 [`FetchEnv`] 로만 바꾼다.** 일반 env 를 받지 않는다 — 키는 상수이고 자유 문자열은 `GIT_SSH_COMMAND` 의 값뿐이다.
//! - **훅 차단 경로는 이 모듈이 실행기 종류로 만든다**(호출자가 `HooksPath` 를 넘기지 못한다). 로컬은 앱 소유 빈 디렉터리(드라이브 문자 경로),
//!   WSL 은 `/dev/null` 이라 Windows 경로가 WSL 로 갈 수 없다. 경로는 `canonicalize` 하지 않는다(Windows 에서 `\\?\` 접두가 붙어 UNC·NT 모양이 된다).
//! - 모든 `-c` 는 `fetch` 앞에, remote 이름은 `--` 뒤(와 refspec 안)에만 둔다. `--refmap=`(빈 값)을 빼지 않는다.
//! - `filter`·`FollowRedirects::False`·임의 env 는 이 API 에 없다(GitHub 원격 전용 저장소는 사이드카가 처리한다).
//! - git 은 [`GitRunner`] 로만 부른다. 실제 실행은 [`GitExecutor`], 테스트는 옛 git 을 흉내 내는 래퍼를 끼운다(프로세스를 띄우는 길은 늘지 않는다).

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::guard::{
    apply_remote_list, apply_remote_urls, fetch_config_args, finish_remote_urls, hooks_path_for_test, inspect_entries, is_empty_dir, parse_config_list,
    parse_git_version, resolve_ssh_command, show_scope_outcome, unknown_write_fetch_head, ConfigArgsOpts, ConfigEntry, ConfigInspection, FollowRedirects, GitOut,
    GitVersion, ScopeWalk, ShowScopeOutcome, WalkStep, FETCH_ARGS,
};
use super::js_trim;
use crate::git::exec::{ExecError, FetchEnv, GitExecutor, RunOpts, RunResult};

/// 이 모듈이 git 을 부르는 유일한 길
pub trait GitRunner: Sync {
    /// "local" | "wsl"
    fn kind(&self) -> &'static str;
    fn run(&self, repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send;
}

impl GitRunner for GitExecutor {
    fn kind(&self) -> &'static str {
        GitExecutor::kind(self)
    }

    fn run(&self, repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        GitExecutor::run(self, repo_path, args, opts)
    }
}

fn strings<const N: usize>(a: [&str; N]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

/// `Buffer.toString('utf8')` 와 같다(잘못된 바이트는 U+FFFD)
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn config_error(code: i32, message: String, stderr: &str) -> ExecError {
    ExecError::Git { message, code, stderr: stderr.to_string(), args: strings(["config"]) }
}

fn cancel_opts(cancel: Option<&CancellationToken>) -> RunOpts {
    RunOpts { cancel: cancel.cloned(), ..RunOpts::default() }
}

// ── 설정 검사 ─────────────────────────────────────────────────────────────

/// 저장소 설정 항목을 scope 와 함께 읽는다(include 포함). 읽지 못하면 검사할 수 없으므로 Err
pub async fn read_config_entries<R: GitRunner>(r: &R, repo_path: &str, opts: &RunOpts) -> Result<Vec<ConfigEntry>, ExecError> {
    let out = r.run(repo_path, &strings(["config", "--list", "--show-scope", "--includes", "-z"]), opts).await?;
    match show_scope_outcome(out.code, &out.stderr) {
        ShowScopeOutcome::Parsed => Ok(parse_config_list(&text(&out.stdout))),
        ShowScopeOutcome::Fallback => read_config_by_scope(r, repo_path, opts).await,
        ShowScopeOutcome::Error(message) => Err(config_error(out.code, message, &out.stderr)),
    }
}

/// `--show-scope` 가 없는 git(2.26 미만)용: 범위마다 따로 읽는다. 순서·실패 처리는 [`ScopeWalk`] 가 정한다
async fn read_config_by_scope<R: GitRunner>(r: &R, repo_path: &str, opts: &RunOpts) -> Result<Vec<ConfigEntry>, ExecError> {
    let mut walk = ScopeWalk::start();
    loop {
        let scope_flag = format!("--{}", walk.scope());
        let out = r.run(repo_path, &["config".to_string(), scope_flag, "--list".to_string(), "--includes".to_string(), "-z".to_string()], opts).await?;
        match walk.feed(out.code, &text(&out.stdout), &out.stderr) {
            Err(message) => return Err(config_error(out.code, message, &out.stderr)),
            Ok(WalkStep::Read(next)) => walk = next,
            Ok(WalkStep::Done(entries)) => return Ok(entries),
        }
    }
}

/// 설정 파일 목록에 없는 옛 형식 remote(.git/remotes/<n>, .git/branches/<n>)까지 포함해 모든 remote 의 실제 URL 을 검사한다.
/// 하나라도 읽지 못하면 막는다(`remote`·`remote get-url --all -- <n>` 은 읽기만 한다)
async fn inspect_remote_urls<R: GitRunner>(r: &R, repo_path: &str, ins: &mut ConfigInspection, opts: &RunOpts) -> Result<(), ExecError> {
    let list = r.run(repo_path, &strings(["remote"]), opts).await?;
    let stdout = text(&list.stdout);
    let outcome = if list.code == 0 { GitOut::Ok(&stdout) } else { GitOut::Failed(&list.stderr) };
    let Some(names) = apply_remote_list(ins, outcome) else {
        return Ok(());
    };
    for name in names {
        let args = ["remote".to_string(), "get-url".to_string(), "--all".to_string(), "--".to_string(), name.clone()];
        let got = r.run(repo_path, &args, opts).await?;
        let stdout = text(&got.stdout);
        apply_remote_urls(ins, &name, if got.code == 0 { GitOut::Ok(&stdout) } else { GitOut::Failed(&got.stderr) });
    }
    finish_remote_urls(ins);
    Ok(())
}

/// TS `inspectConfig`
pub async fn inspect_config<R: GitRunner>(r: &R, repo_path: &str, cancel: Option<&CancellationToken>) -> Result<ConfigInspection, ExecError> {
    let opts = cancel_opts(cancel);
    let mut ins = inspect_entries(&read_config_entries(r, repo_path, &opts).await?);
    inspect_remote_urls(r, repo_path, &mut ins, &opts).await?;
    Ok(ins)
}

// ── git 버전 ──────────────────────────────────────────────────────────────

const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

/// TS `gitVersion`: 15초 안에 `git --version` 을 읽지 못하면 None(실패도 None)
pub async fn git_version<R: GitRunner>(r: &R) -> Option<GitVersion> {
    let cancel = CancellationToken::new();
    let timer = {
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(VERSION_TIMEOUT).await;
            c.cancel();
        })
    };
    let res = r.run(".", &strings(["--version"]), &cancel_opts(Some(&cancel))).await;
    timer.abort();
    match res {
        Ok(out) if out.code == 0 => parse_git_version(&text(&out.stdout)),
        _ => None,
    }
}

/// 실행기별 git 버전 캐시(TS `FetchService.version` 의 `versions` 맵). 실패(None)도 캐시한다 — TS 가 Promise 를 캐시하는 것과 같다
#[derive(Default)]
pub struct GitVersions {
    cells: Mutex<HashMap<String, Arc<OnceCell<Option<GitVersion>>>>>,
}

impl GitVersions {
    pub async fn get(&self, exec: &GitExecutor) -> Option<GitVersion> {
        let cell = {
            let mut cells = self.cells.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(cells.entry(exec.cache_key()).or_default())
        };
        *cell.get_or_init(|| git_version(exec)).await
    }
}

// ── fetch 실행 ────────────────────────────────────────────────────────────

/// `GIT_SSH_COMMAND`·`GIT_SSH` 만 읽어 만든 환경 맵. 이름 찾기는 `lookup` 이 정한다 —
/// 실제 프로세스에서는 `std::env::var` 라 Windows 에서 대소문자를 구분하지 않는다(Node 의 `process.env` 와 같다).
/// `std::env::vars()` 로 맵을 채우면 `Git_Ssh_Command` 를 놓쳐 [`resolve_ssh_command`] 가 달라진다
pub fn ssh_env(lookup: impl Fn(&str) -> Option<String>) -> HashMap<String, String> {
    ["GIT_SSH_COMMAND", "GIT_SSH"].iter().filter_map(|k| lookup(k).map(|v| ((*k).to_string(), v))).collect()
}

pub fn process_ssh_env() -> HashMap<String, String> {
    ssh_env(|k| std::env::var(k).ok())
}

/// 이 fetch 의 환경. ssh 명령은 항상 명시한다(저장소 안의 core.sshCommand 가 적용되지 않게). 플랫폼은 실제 플랫폼이다
pub fn fetch_env_for(ins: &ConfigInspection, kind: &str, env: &HashMap<String, String>, allow_file: bool) -> FetchEnv {
    fetch_env_for_test(ins, kind, env, cfg!(windows), allow_file)
}

/// [`fetch_env_for`] 에 플랫폼을 주입한다. **테스트 전용** — 운영 코드는 부르지 않는다(`tests/fetch_service.rs` 가 소스로 강제한다)
#[doc(hidden)]
pub fn fetch_env_for_test(ins: &ConfigInspection, kind: &str, env: &HashMap<String, String>, platform_win32: bool, allow_file: bool) -> FetchEnv {
    FetchEnv { allow_file, ssh_command: resolve_ssh_command(ins.user_ssh_command.as_deref(), kind, env, platform_win32) }
}

/// fetch 에만 쓰는 `-c` 인자. 훅 차단 경로는 실행기 종류와 `no_hooks_dir` 로 여기서 정한다([`hooks_path_for`]). 플랫폼은 실제 플랫폼이다
pub fn fetch_config_for(kind: &str, no_hooks_dir: Option<&str>, credential_helpers: bool) -> Result<Vec<String>, String> {
    fetch_config_for_test(kind, no_hooks_dir, cfg!(windows), credential_helpers)
}

/// [`fetch_config_for`] 에 플랫폼을 주입한다. **테스트 전용** — 운영 코드는 부르지 않는다
#[doc(hidden)]
pub fn fetch_config_for_test(kind: &str, no_hooks_dir: Option<&str>, platform_win32: bool, credential_helpers: bool) -> Result<Vec<String>, String> {
    let hooks = hooks_path_for_test(kind == "local", no_hooks_dir, platform_win32, is_empty_dir)?;
    Ok(fetch_config_args(&hooks, ConfigArgsOpts { credential_helpers, follow_redirects: FollowRedirects::Initial }))
}

/// remote 하나의 fetch 인자(`git -C <repo> <전역 인자>` 뒤). 순서: 모든 `-c` → `fetch` 와 고정 옵션 → (`--no-write-fetch-head`) → `--refmap=` → `--` → 이름 → refspec.
/// refspec 의 목적지는 `refs/remotes/<이름>/` 로 고정한다 — 설정의 refspec 은 쓰지 않는다
pub fn fetch_args(cfg: &[String], name: &str, write_fetch_head: bool) -> Vec<String> {
    let mut v: Vec<String> = cfg.to_vec();
    v.extend(FETCH_ARGS.iter().map(|s| (*s).to_string()));
    if !write_fetch_head {
        v.push("--no-write-fetch-head".to_string());
    }
    v.extend(["--refmap=".to_string(), "--".to_string(), name.to_string(), format!("+refs/heads/*:refs/remotes/{name}/*")]);
    v
}

pub struct FetchRunOpts<'a> {
    pub cancel: Option<CancellationToken>,
    /// 로컬 실행기의 훅 차단용 빈 디렉터리(앱 소유). WSL 은 쓰지 않는다
    pub no_hooks_dir: Option<&'a str>,
    pub allow_file: bool,
    /// 자격 증명 도우미를 쓴다(사용자가 직접 누른 fetch, 또는 같은 remote 가 수동 fetch 로 승인된 경우)
    pub credential_helpers: bool,
}

/// TS `runFetch`: remote 마다 목적지가 `refs/remotes/<이름>/` 로 고정된 refspec 을 명령줄에 직접 준다(`fetch --all` 이 아니다).
/// 설정의 refspec 이 목적지를 정하면 refs/heads·refs/stash·refs/replace·HEAD 를 덮어쓸 수 있어서다. `--refmap=`(빈 값)이 필수다 —
/// 없으면 git 이 설정된 refspec 으로 기회적 원격 추적 갱신을 해 명령줄 refspec 을 줘도 설정이 고른 목적지에 쓴다.
/// 옛 git 이 `--no-write-fetch-head` 를 모르면 그 옵션 없이 한 번 더 한다. 한 remote 가 실패해도 나머지는 계속하고 첫 오류를 돌려준다.
pub async fn run_fetch<R: GitRunner>(r: &R, repo_path: &str, ins: &ConfigInspection, o: &FetchRunOpts<'_>) -> Result<(), ExecError> {
    run_fetch_for_test(r, repo_path, ins, o, &process_ssh_env(), cfg!(windows)).await
}

/// [`run_fetch`] 에 ssh 환경과 플랫폼을 주입한다. **테스트 전용** — 플랫폼을 호출자가 정하면 훅 경로 판정이 달라진다.
/// 운영 코드는 [`run_fetch`] 만 부른다(`tests/fetch_service.rs` 가 service.rs 소스에 이 이름이 없음을 강제한다)
#[doc(hidden)]
pub async fn run_fetch_for_test<R: GitRunner>(
    r: &R,
    repo_path: &str,
    ins: &ConfigInspection,
    o: &FetchRunOpts<'_>,
    env: &HashMap<String, String>,
    platform_win32: bool,
) -> Result<(), ExecError> {
    let cfg = fetch_config_for_test(r.kind(), o.no_hooks_dir, platform_win32, o.credential_helpers).map_err(|message| ExecError::Other { message })?;
    let run_opts = RunOpts { cancel: o.cancel.clone(), fetch_env: Some(fetch_env_for_test(ins, r.kind(), env, platform_win32, o.allow_file)), ..RunOpts::default() };
    let mut first_error: Option<ExecError> = None;
    for name in &ins.remote_names {
        let mut out = r.run(repo_path, &fetch_args(&cfg, name, false), &run_opts).await?;
        if out.code != 0 && unknown_write_fetch_head(&out.stderr) {
            out = r.run(repo_path, &fetch_args(&cfg, name, true), &run_opts).await?;
        }
        if out.code != 0 && first_error.is_none() {
            let stderr = js_trim(&out.stderr);
            let message = if stderr.is_empty() { format!("git fetch 실패({})", out.code) } else { stderr.to_string() };
            first_error = Some(ExecError::Git { message, code: out.code, stderr: out.stderr.clone(), args: FETCH_ARGS.iter().map(|s| (*s).to_string()).collect() });
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_args_는_c_가_모두_fetch_앞이고_이름은_구분자_뒤다() {
        let cfg = fetch_config_for_test("wsl", None, true, false).unwrap();
        let v = fetch_args(&cfg, "-x", false);
        let fetch_at = v.iter().position(|a| a == "fetch").unwrap();
        assert!(v.iter().enumerate().filter(|(_, a)| *a == "-c").all(|(i, _)| i < fetch_at));
        assert!(v.contains(&"--refmap=".to_string()));
        let dd = v.iter().position(|a| a == "--").unwrap();
        assert_eq!(&v[dd + 1..], ["-x", "+refs/heads/*:refs/remotes/-x/*"]);
        assert_eq!(v.iter().filter(|a| a.as_str() == "-x").count(), 1);
    }

    #[test]
    fn ssh_env_는_조회_함수가_정한_이름만_읽는다() {
        let env = ssh_env(|k| (k == "GIT_SSH").then(|| "/bin/ssh".to_string()));
        assert_eq!(env.get("GIT_SSH").map(String::as_str), Some("/bin/ssh"));
        assert!(!env.contains_key("GIT_SSH_COMMAND"));
    }
}
