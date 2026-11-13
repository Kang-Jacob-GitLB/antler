//! 자식 프로세스 실행 공통부(`src/main/executors/processRunner.ts`). Local·WSL 실행기가 공유한다.
//! 이 프로젝트에서 git 을 띄우는 `Command::new` 는 여기 한 곳뿐이다(clippy.toml 의 disallowed-methods).

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use super::common::{RunOpts, RunResult, Spec, DEFAULT_MAX_BUFFER, MAX_STDERR};
use super::error::ExecError;
use super::wsl;

/// Windows: 콘솔 창을 띄우지 않는다
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// WSL 실패 메시지용으로 stdout 앞부분을 이만큼만 기억한다
const WSL_HEAD: usize = 4096;

/// 취소 신호를 기다린다. 신호가 없으면 영원히 기다린다
async fn cancelled(cancel: &Option<CancellationToken>) {
    match cancel {
        Some(t) => t.cancelled().await,
        None => std::future::pending().await,
    }
}

/// `fut` 를 기다리되 취소되면 `None`(취소가 이긴다)
async fn or_cancel<T>(cancel: &Option<CancellationToken>, fut: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        _ = cancelled(cancel) => None,
        v = fut => Some(v),
    }
}

fn is_cancelled(cancel: &Option<CancellationToken>) -> bool {
    cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
}

/// Windows 에서 PATH 의 git 이 Git for Windows 래퍼(cmd\git.exe)면 실제 git.exe 는 손자 프로세스라
/// `Child::kill` 로는 남는다 — `taskkill /pid N /T /F` 로 트리를 종료한다(TS 와 같은 명령).
///
/// **별도 스레드에서** 띄우고 기다린다. taskkill 을 띄우는 CreateProcess 는 부하가 없어도 100ms 를 넘고 부하가 있으면
/// 초 단위라, 런타임 스레드에서 하면 취소한 호출과 그 런타임의 다른 작업이 그동안 멈춘다(실측 — docs/tauri.md 실행기 절).
/// 루트 프로세스 핸들(`process`)을 taskkill 이 끝날 때까지 쥐고 있어 그 사이 PID 가 다른 프로세스에 재사용되지 않는다.
/// taskkill 이 실패하면 그 핸들로 루트만이라도 끝낸다(루트를 먼저 끝내면 `/T` 의 트리 탐색이 깨지므로 순서는 이렇다).
#[cfg(windows)]
fn taskkill_in_background(pid: u32, process: std::os::windows::io::OwnedHandle) {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    std::thread::spawn(move || {
        // taskkill 도 자식 프로세스지만 git 이 아니라 실행기 자신의 종료 수단이라 여기서만 직접 띄운다
        #[allow(clippy::disallowed_methods)]
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/pid", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let ok = cmd.status().is_ok_and(|s| s.success());
        if !ok {
            // 이미 끝난 프로세스면 실패해도 무해하다
            let h = windows::Win32::Foundation::HANDLE(process.as_raw_handle());
            unsafe {
                let _ = windows::Win32::System::Threading::TerminateProcess(h, 1);
            }
        }
        drop(process);
    });
}

/// 프로세스 트리를 종료한다. 이미 끝난(수거된) 자식이면 아무것도 하지 않는다 —
/// 끝난 PID 는 다른 프로세스가 재사용할 수 있어 그 번호로 taskkill 을 부르면 엉뚱한 프로세스(사용자의 앱 포함)를 죽인다.
fn kill_tree(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::BorrowedHandle;
        if let (Some(pid), Some(raw)) = (child.id(), child.raw_handle()) {
            // 아직 수거되지 않은 자식의 핸들이라 유효하다. 복제본을 스레드로 넘겨 PID 를 붙들어 둔다
            if let Ok(owned) = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned() {
                taskkill_in_background(pid, owned);
                return;
            }
        }
        let _ = child.start_kill();
    }
    #[cfg(not(windows))]
    {
        let _ = child.start_kill();
    }
}

/// 자식을 소유하고, 끝나기 전에 버려지면(취소·소비자 이탈·오류) 트리를 죽인다
pub(super) struct ChildGuard {
    child: Child,
    /// 한 번만 죽인다 — 명시적 kill 뒤 Drop 이 다시 taskkill 을 띄우지 않게(taskkill 이 아직 안 끝나 try_wait 는 '실행 중'이다)
    killed: bool,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child, killed: false }
    }

    pub(super) fn kill_tree(&mut self) {
        if std::mem::replace(&mut self.killed, true) {
            return;
        }
        kill_tree(&mut self.child);
    }

    /// 실행 중이면 true(수거되지 않았다)
    pub(super) fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub(super) fn id(&self) -> Option<u32> {
        self.child.id()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_tree();
    }
}

struct Launched {
    guard: ChildGuard,
    stdout: ChildStdout,
    stderr: JoinHandle<Vec<u8>>,
}

fn launch(spec: &Spec) -> Result<Launched, ExecError> {
    // 이 프로젝트에서 git 을 띄우는 유일한 지점(실행기 자신)
    #[allow(clippy::disallowed_methods)]
    let mut cmd = tokio::process::Command::new(&spec.program);
    // 부모 환경을 상속하고 그 위에 덧씌운다(호출자가 정하는 값은 없다 — spec.env 는 실행기 상수에서만 온다)
    cmd.args(&spec.argv)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| ExecError::spawn(&e))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let mut guard = ChildGuard::new(child);
        guard.kill_tree();
        return Err(ExecError::SpawnFailed { message: "자식 프로세스의 파이프를 얻지 못했다".into() });
    };
    Ok(Launched { guard: ChildGuard::new(child), stdout, stderr: drain_stderr(stderr) })
}

/// stderr 는 앞 4MB 만 담고 파이프는 끝까지 비운다(비우지 않으면 git 이 stderr 파이프에서 멈춘다)
fn drain_stderr(mut stderr: ChildStderr) -> JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = MAX_STDERR - kept.len();
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
        kept
    })
}

fn wait_failed(e: &io::Error) -> ExecError {
    ExecError::spawn(e)
}

/// stdout 을 전부 모아 돌려준다. 상한(기본 64MB)을 넘으면 트리를 죽이고 code -1 의 `Git` 오류.
pub(super) async fn run_process(spec: &Spec, opts: &RunOpts) -> Result<RunResult, ExecError> {
    if is_cancelled(&opts.cancel) {
        return Err(ExecError::Aborted);
    }
    let Launched { mut guard, mut stdout, stderr } = launch(spec)?;
    let max = opts.max_buffer.unwrap_or(DEFAULT_MAX_BUFFER);
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let Some(read) = or_cancel(&opts.cancel, stdout.read(&mut chunk)).await else {
            guard.kill_tree();
            return Err(ExecError::Aborted);
        };
        let n = read.map_err(|e| ExecError::spawn(&e))?;
        if n == 0 {
            break;
        }
        if out.len() + n > max {
            guard.kill_tree();
            return Err(ExecError::Git {
                message: format!("output exceeded {max} bytes"),
                code: -1,
                stderr: String::new(),
                args: spec.git_args.clone(),
            });
        }
        out.extend_from_slice(&chunk[..n]);
    }
    let Some(status) = or_cancel(&opts.cancel, guard.child.wait()).await else {
        guard.kill_tree();
        return Err(ExecError::Aborted);
    };
    let status = status.map_err(|e| wait_failed(&e))?;
    let Some(err) = or_cancel(&opts.cancel, stderr).await else {
        guard.kill_tree();
        return Err(ExecError::Aborted);
    };
    let err = err.map_err(|e| ExecError::SpawnFailed { message: e.to_string() })?;
    Ok(RunResult { stdout: out, stderr: String::from_utf8_lossy(&err).into_owned(), code: status.code().unwrap_or(-1) })
}

/// 실행 중인 git 의 stdout. [`AsyncRead`] 로 읽는다 — **읽을 때만** 파이프에서 가져오므로 소비자가 읽지 않으면
/// git 이 파이프가 찬 곳에서 멈춘다(backpressure). 버리면(Drop) 프로세스 트리를 죽인다.
/// EOF 까지 읽은 뒤 [`GitStream::finish`] 로 종료 코드를 확인한다.
pub struct GitStream {
    stdout: ChildStdout,
    guard: ChildGuard,
    stderr: Option<JoinHandle<Vec<u8>>>,
    token: Option<CancellationToken>,
    cancel_wait: Option<Pin<Box<WaitForCancellationFutureOwned>>>,
    git_args: Vec<String>,
    wsl: bool,
    /// WSL 실패 메시지용 stdout 앞부분
    head: Vec<u8>,
    eof: bool,
    aborted: bool,
}

pub(super) fn stream_process(spec: &Spec, opts: &RunOpts) -> Result<GitStream, ExecError> {
    if is_cancelled(&opts.cancel) {
        return Err(ExecError::Aborted);
    }
    let Launched { guard, stdout, stderr } = launch(spec)?;
    Ok(GitStream {
        stdout,
        guard,
        stderr: Some(stderr),
        token: opts.cancel.clone(),
        cancel_wait: opts.cancel.clone().map(|t| Box::pin(t.cancelled_owned())),
        git_args: spec.git_args.clone(),
        wsl: spec.wsl,
        head: Vec::new(),
        eof: false,
        aborted: false,
    })
}

fn aborted_io() -> io::Error {
    io::Error::other("aborted")
}

impl GitStream {
    /// 프로세스 PID(수동 테스트용 — 이미 수거됐으면 None)
    pub fn pid(&self) -> Option<u32> {
        self.guard.id()
    }

    /// 프로세스가 아직 안 끝났으면 true
    pub fn is_running(&mut self) -> bool {
        self.guard.is_running()
    }

    /// EOF 까지 읽은 뒤 호출한다: 종료를 기다려 코드를 검사한다. 취소됐으면 `Aborted`, 0 이 아니면 `Git`.
    /// EOF 전에 부르면(소비자가 중도 포기) 프로세스를 죽이고 `Aborted` 를 돌려준다.
    pub async fn finish(mut self) -> Result<(), ExecError> {
        if self.aborted || !self.eof {
            self.guard.kill_tree();
            return Err(ExecError::Aborted);
        }
        let token = self.token.clone();
        let Some(status) = or_cancel(&token, self.guard.child.wait()).await else {
            self.guard.kill_tree();
            return Err(ExecError::Aborted);
        };
        let status = status.map_err(|e| wait_failed(&e))?;
        let handle = self.stderr.take().expect("finish 는 한 번만 호출된다");
        let Some(err) = or_cancel(&token, handle).await else {
            self.guard.kill_tree();
            return Err(ExecError::Aborted);
        };
        let err = err.map_err(|e| ExecError::SpawnFailed { message: e.to_string() })?;
        if is_cancelled(&token) {
            return Err(ExecError::Aborted);
        }
        let code = status.code().unwrap_or(-1);
        if code == 0 {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&err).into_owned();
        if self.wsl {
            if let Some(e) = wsl::failure_error(code, &stderr, &self.head, &self.git_args) {
                return Err(e);
            }
        }
        let message = match crate::git::parsers::js_trim(&stderr) {
            "" => format!("git exited with {code}"),
            t => t.to_string(),
        };
        Err(ExecError::Git { message, code, stderr, args: self.git_args.clone() })
    }
}

impl AsyncRead for GitStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.aborted {
            return Poll::Ready(Err(aborted_io()));
        }
        // 취소를 먼저 본다(읽다가 취소돼도 깨어나도록 waker 가 여기에 등록된다)
        if let Some(w) = this.cancel_wait.as_mut() {
            if w.as_mut().poll(cx).is_ready() {
                this.aborted = true;
                this.guard.kill_tree();
                return Poll::Ready(Err(aborted_io()));
            }
        }
        let before = buf.filled().len();
        // 받을 자리가 없을 때의 빈 읽기는 EOF 가 아니다
        let had_room = buf.remaining() > 0;
        let r = Pin::new(&mut this.stdout).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = r {
            let got = &buf.filled()[before..];
            if got.is_empty() {
                if had_room {
                    this.eof = true;
                }
            } else if this.wsl && this.head.len() < WSL_HEAD {
                let room = WSL_HEAD - this.head.len();
                this.head.extend_from_slice(&got[..got.len().min(room)]);
            }
        }
        r
    }
}
