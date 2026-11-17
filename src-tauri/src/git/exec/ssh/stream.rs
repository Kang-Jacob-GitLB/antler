//! SSH `stream` 의 소비자 쪽 — 채널을 소유한 펌프 태스크(`connection.rs` 의 `pump`)가 보내는 이벤트를 [`AsyncRead`] 로 읽는다.
//!
//! **왜 펌프인가**: russh 는 채널 윈도우를 받은 바이트로 조정하고 채널 큐(메시지 100개)가 차면 세션 루프 전체가 멈춘다. 소비자가 느리거나 읽지 않는 스트림 하나가
//! 같은 연결의 다른 채널을 모두 세울 수 있다(docs/tauri.md 스파이크). 그래서 채널은 소비자가 아니라 **펌프 태스크가 즉시 비우고**, 소비자는 무제한 큐에서 가져간다.
//! 큐에 쌓인 바이트가 [`STREAM_AHEAD_LIMIT`](16MB)를 넘으면 펌프가 원격을 KILL·close 하고 남은 출력을 버린다. 소비자는 이미 쌓인 데이터를 다 읽은 뒤
//! [`ReadAheadExceeded`] 오류를 받는다(Local·WSL 에는 없는 동작 — 거기서는 파이프 backpressure 로 git 이 멈춘다).
//!
//! 스트림을 버리면(Drop) 펌프에 중지를 알려 원격에 KILL → close 를 보내고 슬롯을 닫힘 확인 뒤에 돌려준다.

use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, DropGuard, WaitForCancellationFutureOwned};

use crate::git::exec::error::ExecError;
use crate::git::parsers::js_trim;

/// 소비자가 읽기 전에 쌓아 둘 수 있는 stdout 상한(16MB). 넘으면 원격을 죽이고 스트림이 [`ReadAheadExceeded`] 로 끝난다
pub const STREAM_AHEAD_LIMIT: usize = 16 * 1024 * 1024;

/// 큐에 들어가는 메시지 하나가 상한에 더하는 고정 오버헤드. 빈 데이터·1바이트 프레임을 무제한 보내 `Vec` 와 큐 노드만 쌓는 서버를 막는다
/// (펌프가 더하고 소비자가 같은 값을 뺀다)
pub(super) const EVENT_COST: usize = 64;

const CONNECTION_LOST: &str = "SSH 연결이 끊겨 원격 git 이 끝나기 전에 종료되었습니다";

/// 선읽기 상한을 넘어 원격을 죽였다는 읽기 오류의 원인. [`is_read_ahead_exceeded`] 로 알아본다
#[derive(Debug)]
pub struct ReadAheadExceeded;

impl fmt::Display for ReadAheadExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SSH 출력을 소비하는 속도가 느려 선읽기 상한을 넘어 원격 git 을 중단했습니다")
    }
}

impl std::error::Error for ReadAheadExceeded {}

/// 스트림 읽기 오류가 선읽기 상한 초과인가(취소·연결 오류와 구별한다)
pub fn is_read_ahead_exceeded(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<ReadAheadExceeded>())
}

/// 원격 명령이 끝났다는 알림
pub(super) struct End {
    /// 종료 코드. 받지 못하고 채널이 닫혔으면 `None`(연결이 끊긴 것)
    pub code: Option<i32>,
    /// stderr 앞 4MB
    pub stderr: Vec<u8>,
    /// 서버가 exec 요청을 거부했다
    pub rejected: bool,
}

pub(super) enum Event {
    Data(Vec<u8>),
    End(End),
    /// 선읽기 상한을 넘어 원격을 죽였다(이 앞의 데이터는 모두 전달됐다)
    Overflow,
}

/// 원격 git 의 stdout. [`GitStream`](crate::git::exec::GitStream) 의 SSH 변형이다
pub struct SshStream {
    rx: mpsc::UnboundedReceiver<Event>,
    /// 큐에 쌓였지만 소비자가 아직 꺼내지 않은 바이트(펌프가 더하고 여기서 뺀다)
    queued: Arc<AtomicUsize>,
    cur: Vec<u8>,
    pos: usize,
    end: Option<End>,
    overflowed: bool,
    aborted: bool,
    token: Option<CancellationToken>,
    cancel_wait: Option<Pin<Box<WaitForCancellationFutureOwned>>>,
    git_args: Vec<String>,
    /// 버려지면 펌프에 중지를 알린다
    _stop: DropGuard,
}

impl SshStream {
    pub(super) fn new(
        rx: mpsc::UnboundedReceiver<Event>,
        queued: Arc<AtomicUsize>,
        token: Option<CancellationToken>,
        git_args: Vec<String>,
        stop: DropGuard,
    ) -> Self {
        SshStream {
            rx,
            queued,
            cur: Vec::new(),
            pos: 0,
            end: None,
            overflowed: false,
            aborted: false,
            cancel_wait: token.clone().map(|t| Box::pin(t.cancelled_owned())),
            token,
            git_args,
            _stop: stop,
        }
    }

    /// 큐에 쌓였지만 아직 꺼내지 않은 과금 바이트(시험 전용 — 소비하면 0 으로 돌아오는지 본다)
    #[cfg(feature = "test-hooks")]
    pub fn queued_for_test(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// 끝(EOF)을 아직 받지 못했으면 true
    pub fn is_running(&mut self) -> bool {
        self.end.is_none()
    }

    /// EOF 까지 읽은 뒤 호출한다: 종료 코드를 검사한다. 취소됐으면 `Aborted`, 0 이 아니면 `Git`, 코드 없이 끊겼으면 연결 오류.
    /// EOF 전에 부르면(소비자가 중도 포기) 원격을 죽이고(Drop) `Aborted` 를 돌려준다.
    /// 다른 변형(`ProcessStream::finish`)과 모양을 맞춘 async 다(기다릴 것은 없다)
    pub async fn finish(self) -> Result<(), ExecError> {
        let Some(end) = &self.end else { return Err(ExecError::Aborted) };
        if self.aborted || self.overflowed || self.token.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(ExecError::Aborted);
        }
        if end.rejected {
            return Err(ExecError::Other { message: "원격 명령을 시작하지 못했습니다: 서버가 exec 요청을 거부했습니다".into() });
        }
        match end.code {
            None => Err(ExecError::Other { message: CONNECTION_LOST.into() }),
            Some(0) => Ok(()),
            Some(code) => {
                let stderr = String::from_utf8_lossy(&end.stderr).into_owned();
                let message = match js_trim(&stderr) {
                    "" => format!("git exited with {code}"),
                    t => t.to_string(),
                };
                Err(ExecError::Git { message, code, stderr, args: self.git_args.clone() })
            }
        }
    }
}

fn aborted_io() -> io::Error {
    io::Error::other("aborted")
}

impl AsyncRead for SshStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.aborted {
            return Poll::Ready(Err(aborted_io()));
        }
        // 취소를 먼저 본다(읽다가 취소돼도 깨어나도록 waker 가 여기에 등록된다)
        if let Some(w) = this.cancel_wait.as_mut() {
            if w.as_mut().poll(cx).is_ready() {
                this.aborted = true;
                return Poll::Ready(Err(aborted_io()));
            }
        }
        if this.overflowed {
            return Poll::Ready(Err(io::Error::other(ReadAheadExceeded)));
        }
        loop {
            // 받을 자리가 없을 때의 빈 읽기는 EOF 가 아니다
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            if this.pos < this.cur.len() {
                let n = buf.remaining().min(this.cur.len() - this.pos);
                buf.put_slice(&this.cur[this.pos..this.pos + n]);
                this.pos += n;
                return Poll::Ready(Ok(()));
            }
            // 끝을 받았으면 EOF(데이터는 항상 끝보다 앞에 온다)
            if this.end.is_some() {
                return Poll::Ready(Ok(()));
            }
            match this.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Event::Data(d))) => {
                    this.queued.fetch_sub(d.len() + EVENT_COST, Ordering::SeqCst);
                    this.cur = d;
                    this.pos = 0;
                }
                Poll::Ready(Some(Event::End(e))) => this.end = Some(e),
                Poll::Ready(Some(Event::Overflow)) => {
                    this.overflowed = true;
                    return Poll::Ready(Err(io::Error::other(ReadAheadExceeded)));
                }
                // 펌프가 끝을 알리지 못하고 사라졌다(런타임 종료 등) — 연결이 끊긴 것으로 본다
                Poll::Ready(None) => this.end = Some(End { code: None, stderr: Vec::new(), rejected: false }),
            }
        }
    }
}
