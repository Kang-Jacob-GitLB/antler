//! GitHub CLI(gh)에서 로그인 토큰을 가져온다(`src/main/github/ghCli.ts` 의 `createGhCli`) — [`ExternalTokenProvider`] 의 실제 구현. 아직 앱에서 호출하지 않는다.
//!
//! git 실행기(`git::exec`)와 따로 띄운다. 거기는 부모 환경을 상속하고 git 전용 방어를 얹지만 gh 는 **환경을 비우고** 허용 목록([`gh_env`])만 넘겨야 하기 때문이다.
//! - 실행 파일은 **PATH 를 직접 해석해 절대 경로로** 띄운다([`find_gh`]). 이름만 넘기면 Windows 의 프로세스 생성(`CreateProcess` 계열)이 현재 작업 디렉터리나 저장소 폴더의 `gh.exe` 를 집을 수 있는 함정이 있어 미리 막는다.
//!   PATH 의 빈 항목·상대 경로 항목·현재 디렉터리와 같은 항목은 건너뛰고, `gh.exe` 만 찾는다(셸 없이는 `.cmd`·`.bat` 을 띄울 수 없다).
//! - 프로세스마다 Job Object(`KILL_ON_JOB_CLOSE`)를 만들어 일시 정지로 띄우고 Job 에 넣은 뒤 재개한다. 끝나든 제한 시간으로 버려지든 Job 이 닫히면 손자까지 죽는다 —
//!   git 실행기와 달리 루트가 스스로 끝나도 남은 자손을 두지 않는다. Job 을 만들거나 넣지 못하면 **실행하지 않고** 실패한다(taskkill 폴백 없음).
//! - stdout 은 64KB 까지만 읽고(읽는 중에 적용 — 넘으면 바로 죽인다), 띄우기·읽기·종료 대기 전체에 10초 제한을 건다. stderr 는 `null` 로 버려 보관하지 않는다.
//! - 토큰은 [`SecretString`] 으로만 돌려주고, 읽는 버퍼는 `Zeroizing` 이다(tokio 파이프 내부 버퍼는 덮을 수 없다). 오류 문구는 고정 문장이라 stdout·stderr·경로가 실리지 않는다.
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt};
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::OnceCell;
use zeroize::Zeroizing;

use crate::git::parsers::js_trim;
use crate::net::redact::SecretString;

use super::gh_env::gh_env;
use super::ports::{ExternalTokenError, ExternalTokenProvider};

/// 실행 시간 제한(TS `TIMEOUT_MS`)
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// stdout 상한(TS `MAX_BUFFER`)
pub const MAX_STDOUT: usize = 64 * 1024;

pub const MSG_MISSING: &str = "GitHub CLI(gh)가 설치되어 있지 않습니다";
pub const MSG_TIMEOUT: &str = "GitHub CLI 가 10초 안에 응답하지 않았습니다";
pub const MSG_NOT_LOGGED_IN: &str = "GitHub CLI 에 로그인되어 있지 않습니다. 터미널에서 `gh auth login` 을 먼저 실행하세요";
pub const MSG_TOO_LARGE: &str = "GitHub CLI 가 토큰으로 볼 수 없는 긴 출력을 냈습니다";

#[cfg(windows)]
const GH_FILE: &str = "gh.exe";
#[cfg(not(windows))]
const GH_FILE: &str = "gh";

/// Windows: 콘솔 창을 띄우지 않는다
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows: 주 스레드를 멈춘 채 만든다 — Job 에 넣은 뒤에 재개해야 gh 가 띄우는 손자도 Job 안에 있다
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

/// 경로 비교용 정규형(대소문자 무시 — Windows 파일 시스템). 해석하지 못하면 원래 경로 그대로
fn comparable(p: &Path) -> String {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon.to_string_lossy().to_lowercase()
}

/// `path`(PATH 값)에서 실행할 `gh.exe` 의 절대 경로를 찾는다. 앞 항목이 우선이다.
/// 빈 항목·절대 경로가 아닌 항목(`.`·`bin`·`C:foo`·`\foo` 는 모두 현재 디렉터리 기준으로 풀린다)과 `cwd` 와 같은 항목은 건너뛴다.
/// 동기 파일 시스템 호출이라 비동기 문맥에서는 `spawn_blocking` 안에서 부른다. 파일이 있는 항목만 `cwd` 와 비교한다(canonicalize 가 비싸다).
pub fn find_gh(path: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let cwd = cwd.map(comparable);
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(GH_FILE);
        if !candidate.is_file() {
            continue;
        }
        if cwd.as_deref().is_some_and(|c| comparable(&dir) == c) {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// 실행 한 번의 실패 종류
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// 실행 파일이 없다
    Missing,
    Timeout,
    TooLarge,
    /// 띄우지 못했거나 종료 코드가 0 이 아니다(로그인 안 됨으로 본다 — stderr 는 보지 않는다)
    Failed,
}

/// 띄운 gh 와 그 트리를 담은 Job. 버려지면(끝났든 제한 시간으로 취소됐든) 트리가 죽는다
struct Running {
    child: Child,
    #[cfg(windows)]
    job: crate::git::exec::job::Job,
}

impl Drop for Running {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.start_kill();
    }
}

/// GitHub CLI 토큰 출처. `available` 은 한 번만 실행한다(동시 호출도 한 번)
pub struct GhCli {
    avail: OnceCell<bool>,
    timeout: Duration,
    max_stdout: usize,
    /// `None` 이면 호출할 때마다 이 프로세스의 환경을 읽는다(TS 의 `env()`). 값은 [`gh_env`] 로 한 번 더 걸러진다
    source_env: Option<Vec<(String, String)>>,
}

impl Default for GhCli {
    fn default() -> Self {
        Self::new()
    }
}

impl GhCli {
    pub fn new() -> Self {
        GhCli { avail: OnceCell::new(), timeout: TIMEOUT, max_stdout: MAX_STDOUT, source_env: None }
    }

    /// 시험용: 환경 원본을 주입한다(PATH 에 가짜 gh 의 폴더를 넣는다). [`gh_env`] 필터는 그대로 거친다
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn with_env_for_test(mut self, env: Vec<(String, String)>) -> Self {
        self.source_env = Some(env);
        self
    }

    /// 시험용: 제한 시간과 stdout 상한을 줄인다
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn with_limits_for_test(mut self, timeout: Duration, max_stdout: usize) -> Self {
        self.timeout = timeout;
        self.max_stdout = max_stdout;
        self
    }

    /// gh 를 인자와 함께 실행해 stdout 을 돌려준다. 종료 코드가 0 이 아니면 실패.
    /// 환경 수집과 gh 탐색(동기 파일 시스템 호출)도 제한 시간 안에서 `spawn_blocking` 으로 돈다
    async fn run(&self, args: &[&str]) -> Result<Zeroizing<Vec<u8>>, Failure> {
        let source_env = self.source_env.clone();
        let max_stdout = self.max_stdout;
        let work = async move {
            let (env, program) = tokio::task::spawn_blocking(move || resolve(source_env)).await.map_err(|_| Failure::Failed)?;
            let program = program.ok_or(Failure::Missing)?;
            exec(&program, args, &env, max_stdout).await
        };
        match tokio::time::timeout(self.timeout, work).await {
            Ok(r) => r,
            // future 가 버려지며 Running 이 트리를 죽인다
            Err(_) => Err(Failure::Timeout),
        }
    }

    async fn token_impl(&self) -> Result<SecretString, ExternalTokenError> {
        let err = |message: &str| ExternalTokenError { message: message.to_string() };
        let out = match self.run(&["auth", "token", "--hostname", "github.com"]).await {
            Ok(out) => out,
            Err(Failure::Missing) => return Err(err(MSG_MISSING)),
            Err(Failure::Timeout) => return Err(err(MSG_TIMEOUT)),
            Err(Failure::TooLarge) => return Err(err(MSG_TOO_LARGE)),
            Err(Failure::Failed) => return Err(err(MSG_NOT_LOGGED_IN)),
        };
        // UTF-8 이 아니면 토큰일 수 없다. 사본을 만들지 않고 빌려서 다듬은 뒤 SecretString 으로 옮긴다
        let text = std::str::from_utf8(&out).map_err(|_| err(MSG_NOT_LOGGED_IN))?;
        let token = js_trim(text);
        if token.is_empty() {
            return Err(err(MSG_NOT_LOGGED_IN));
        }
        Ok(SecretString::new(token))
    }
}

impl ExternalTokenProvider for GhCli {
    fn available(&self) -> BoxFuture<'_, bool> {
        async move { *self.avail.get_or_init(|| async { self.run(&["--version"]).await.is_ok() }).await }.boxed()
    }

    fn token(&self) -> BoxFuture<'_, Result<SecretString, ExternalTokenError>> {
        self.token_impl().boxed()
    }
}

/// gh 에 넘길 환경(TS 의 `env()`; 원본이 없으면 이 프로세스의 환경 — [`gh_env`] 로 걸러진다)과 실행할 gh 의 절대 경로. 동기 호출이다
fn resolve(source_env: Option<Vec<(String, String)>>) -> (Vec<(String, String)>, Option<PathBuf>) {
    let env = match source_env {
        Some(env) => gh_env(&env),
        None => {
            let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
            gh_env(&vars)
        }
    };
    let path = env.iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.as_str()).unwrap_or_default();
    let cwd = std::env::current_dir().ok();
    let program = find_gh(path, cwd.as_deref());
    (env, program)
}

/// gh 를 띄워 stdout 을 상한까지 읽고 종료를 기다린다
async fn exec(program: &Path, args: &[&str], env: &[(String, String)], max_stdout: usize) -> Result<Zeroizing<Vec<u8>>, Failure> {
    // gh 를 띄우는 유일한 지점. 환경을 비우고 허용 목록만 넘긴다(git 실행기와 규칙이 달라 process.rs 를 쓰지 않는다)
    #[allow(clippy::disallowed_methods)]
    let mut cmd = tokio::process::Command::new(program);
    // cwd 는 찾은 gh 의 폴더로 고정한다 — Antler 의 cwd(저장소 폴더일 수 있다)를 물려주지 않는다
    if let Some(dir) = program.parent() {
        cmd.current_dir(dir);
    }
    cmd.args(args).env_clear().envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    // Job 은 spawn 전에 만든다. 못 만들면 gh 를 띄우지 않는다
    #[cfg(windows)]
    let job = crate::git::exec::job::Job::new().map_err(|_| Failure::Failed)?;
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut child = cmd.spawn().map_err(|e| if e.kind() == io::ErrorKind::NotFound { Failure::Missing } else { Failure::Failed })?;
    let stdout = child.stdout.take();
    #[cfg(windows)]
    let mut running = Running { child, job };
    #[cfg(not(windows))]
    let mut running = Running { child };
    // Running 이 버려지면 Job 이 닫히고 트리가 죽는다 — 할당·재개에 실패해도 정지된 gh 를 남기지 않는다
    #[cfg(windows)]
    {
        running.job.assign(&running.child).map_err(|_| Failure::Failed)?;
        crate::git::exec::job::resume_process(&running.child).map_err(|_| Failure::Failed)?;
    }
    let Some(mut stdout) = stdout else { return Err(Failure::Failed) };

    let mut out: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(max_stdout));
    let mut chunk: Zeroizing<Vec<u8>> = Zeroizing::new(vec![0u8; 8 * 1024]);
    loop {
        let n = stdout.read(&mut chunk).await.map_err(|_| Failure::Failed)?;
        if n == 0 {
            break;
        }
        // 넘는 순간 바로 끝낸다(읽기 전에 거른다 — 상한 밖 바이트를 버퍼에 두지 않는다)
        if out.len() + n > max_stdout {
            return Err(Failure::TooLarge);
        }
        out.extend_from_slice(&chunk[..n]);
    }
    let status = running.child.wait().await.map_err(|_| Failure::Failed)?;
    if !status.success() {
        return Err(Failure::Failed);
    }
    Ok(out)
}
