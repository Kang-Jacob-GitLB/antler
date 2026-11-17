//! `git:log` 채널의 커서와 서비스. `src/main/git/logCursor.ts`(LogCursor)와 `src/main/services/RepoService.ts` 의 `log()` 경로
//! (커서 맵·앵커·STALE 판정·유휴 정리)의 이식이다.
//!
//! git log 는 시작할 때마다 모든 ref 를 읽는다. 페이지마다 새 프로세스를 띄우면 스크롤을 못 따라가므로, 다음 페이지가 직전 페이지 바로
//! 뒤일 때는 열어 둔 `git log` 에서 이어 읽는다. 소비하지 않는 동안에는 파이프 backpressure 로 git 이 멈춘다.
//! 멈춘 git 은 pack 파일 핸들을 쥐어 사용자의 `git gc` 를 막으므로 5초 유휴 뒤 닫는다(2.5초마다 훑는다).
//!
//! 구조: 커서마다 **읽기 태스크 하나**가 `GitStream` 을 소유하고, 요청은 mpsc 로 받아 보낸 순서대로 처리한다.
//! 요청을 보내는 일(= 위치 예약)이 동기라서 "예약 순서 = 읽기 순서"가 보장된다(TS 의 `busy` 사슬과 같은 효과).
//!
//! 보안(앱에 연결하기 전 필수 — docs/tauri.md 실행기 절): 커서를 닫을 때는 **반드시 `GitStream` 을 버린다**. 스트림의 취소 토큰은 다음
//! `poll_read` 때만 확인되므로 토큰만 취소하고 스트림을 남기면 git 이 파이프에서 멈춘 채 남는다. [`LogCursor::dispose`] 는 토큰을
//! 취소하고 읽기 태스크를 `abort` 해 태스크가 쥔 스트림이 버려지게(= 프로세스 트리 종료) 한다. 오류로 끝난 읽기 태스크도 스트림을 함께 버린다.
//!
//! SSH 대상(Phase 5): 원격 스트림은 파이프 backpressure 가 없다 — 채널을 펌프가 즉시 비우고 선읽기 상한(16MB)을 넘으면 원격 `git log` 를 죽인다.
//! 읽기가 [`is_read_ahead_exceeded`] 로 끝나면 **오류가 아니라** 그 커서를 못 쓰게(`overflowed`) 하고 지금까지 읽은 만큼만 낸다(`has_more` 는 참).
//! 앵커(마지막으로 낸 커밋의 위치·해시)는 남으므로 다음 페이지는 기존 STALE 경로(`--skip` + 앵커 확인)로 새 커서를 연다.
//! 단 **커밋을 하나도 받지 못하고** 끊기면(진척 0) 빈 페이지(`has_more` 참)를 내지 않고 오류로 끝낸다 — 빈 페이지는 renderer 가 백오프 없이 다시 요청하게 만든다.
//! 이때 커서만 실패로 닫고 기존 앵커는 그대로 둔다.
//! 인자 주입 방어는 `scope.rs`(solo·hide 형식)와 `parse_query`(숫자만)에 있다.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::git::exec::ssh::is_read_ahead_exceeded;
use crate::git::exec::{ExecError, GitExecutor, GitStream, RunOpts, DEFAULT_MAX_BUFFER};
use crate::git::parsers::log::{parse_log_record, Commit};
use crate::git::parsers::nul_records::{NulRecordSplitter, MAX_STREAM_RECORD};
use crate::git::scope::{log_args, parse_scope, scope_args, scope_key, Scope};
use crate::mirror::{RepoMirror, Resolved, Target};

/// 이 시간 동안 안 쓰인 커서는 닫는다
pub const CURSOR_IDLE: Duration = Duration::from_secs(5);
/// 유휴 커서를 훑는 주기
pub const CURSOR_SWEEP: Duration = Duration::from_millis(2500);
/// 한 페이지 상한(`RepoService.log` 의 `Math.min(…, 5000)`)
pub const MAX_COUNT: usize = 5000;
/// stdout 에서 한 번에 읽는 크기(Node 스트림 청크와 비슷하다)
const READ_CHUNK: usize = 64 * 1024;
/// 기억하는 앵커 수 상한. 범위(solo·hide) 조합마다 앵커가 하나씩 쌓이므로 가장 오래된 것부터 버린다
const MAX_ANCHORS: usize = 32;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub commits: Vec<Commit>,
    pub has_more: bool,
}

/// 커서 하나가 낼 수 있는 오류
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    Exec(ExecError),
    /// 이어 읽기 앞의 확인용 한 커밋이 기억한 앵커와 다르다
    Mismatch,
}

/// `git:log` 오류. envelope code 는 ABORTED / STALE / GIT / ERROR
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    Exec(ExecError),
    /// 이어 읽으려던 위치의 커밋이 바뀌었다 — renderer 가 처음부터 다시 읽는다
    Stale,
}

impl LogError {
    pub fn envelope_code(&self) -> &'static str {
        match self {
            LogError::Exec(e) => e.envelope_code(),
            LogError::Stale => "STALE",
        }
    }
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogError::Exec(e) => e.fmt(f),
            LogError::Stale => f.write_str("commit history changed while paging"),
        }
    }
}

impl From<ExecError> for LogError {
    fn from(e: ExecError) -> Self {
        LogError::Exec(e)
    }
}

/// 커밋이 없는 새 저장소에서 HEAD 를 해석하지 못해 나는 오류인지(`isEmptyRepoError`, 대소문자 무시)
pub fn is_empty_repo_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    ["does not have any commits", "bad default revision", "bad revision 'head'", "ambiguous argument 'head'"].iter().any(|p| s.contains(p))
}

// ── 질의 검증 ────────────────────────────────────────────

/// 검증을 마친 `git:log` 질의
#[derive(Debug, Clone)]
pub struct LogRequest {
    pub scope: Option<Scope>,
    pub count: usize,
    pub skip: u64,
}

/// `LogQuery` 검증. TS `RepoService.log` 와 같은 순서·규칙이다: 객체가 아니거나 `count`·`skip` 이 유한한 숫자가 아니면 `invalid query`,
/// 그다음 scope(`invalid scope`·`invalid solo`·`invalid hide`). 통과하면 count 는 1..=5000, skip 은 0 이상의 정수로 맞춘다.
/// git 인자는 여기서 만들지 않는다 — 숫자만 [`log_args`] 의 `--skip=` 에 들어간다.
pub fn parse_query(v: Option<&Value>) -> Result<LogRequest, String> {
    let invalid = || "invalid query".to_string();
    let Some(Value::Object(obj)) = v else { return Err(invalid()) };
    let number = |key: &str| match obj.get(key) {
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()),
        _ => None,
    };
    let (Some(count), Some(skip)) = (number("count"), number("skip")) else { return Err(invalid()) };
    let scope = parse_scope(obj.get("scope"))?;
    // 인자를 만들어 보아 hide 의 해시 같은 오류를 커서를 만지기 전에 잡는다
    scope_args(scope.as_ref())?;
    // JS `Math.floor` 후 clamp. f64 → 정수 변환은 범위를 넘으면 포화한다
    let count = (count.floor().max(1.0) as usize).min(MAX_COUNT);
    let skip = skip.floor().max(0.0) as u64;
    Ok(LogRequest { scope, count, skip })
}

// ── 커서 ────────────────────────────────────────────

#[derive(Debug)]
struct Meta {
    /// 지금까지 페이지로 내보낸 커밋 수
    position: u64,
    /// 대기 중인 read 까지 반영한 다음 시작 위치. 재사용 판정은 이 값으로 한다
    next_position: u64,
    /// 마지막으로 내보낸 커밋 해시. 커서를 다시 만들 때 이어지는지 검증하는 데 쓴다
    last_hash: Option<String>,
    last_used: Instant,
    /// 읽는 중이거나 대기 중인 요청 수
    pending: usize,
    failed: bool,
    closed: bool,
    /// 끝까지 읽어 더 내보낼 커밋이 없다
    exhausted: bool,
    /// 원격(SSH) 스트림이 선읽기 상한을 넘어 끊겼다. 이어 읽을 수는 없지만 실패는 아니다 — 앵커는 남겨 다음 페이지가 새 커서로 이어 읽는다
    overflowed: bool,
    /// git 프로세스 PID(테스트용). 읽기 태스크가 띄운 뒤에 채워진다
    pid: Option<u32>,
}

struct Req {
    /// 먼저 한 커밋을 읽어 이 해시와 같은지 본다(앵커 이어 읽기)
    verify: Option<String>,
    count: usize,
    reply: oneshot::Sender<Result<LogPage, CursorError>>,
}

/// 요청을 기다리는 쪽(`read` 가 돌려준 future)이 끝나면 대기 수를 줄인다. 응답을 받기 전에 버려지면 위치 예약을 믿을 수 없으므로 커서를 못 쓰게 한다
struct PendingGuard {
    meta: Arc<Mutex<Meta>>,
    settled: bool,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut m = lock(&self.meta);
        m.pending = m.pending.saturating_sub(1);
        if !self.settled {
            m.failed = true;
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 하나의 `git log` 프로세스를 열어 두고 페이지 단위로 이어 읽는 커서
pub struct LogCursor {
    meta: Arc<Mutex<Meta>>,
    tx: mpsc::UnboundedSender<Req>,
    task: JoinHandle<()>,
    token: CancellationToken,
}

impl LogCursor {
    /// `args` 는 `start_at` 위치부터 읽도록 만든 `git log` 인자다. tokio 런타임 안에서 불러야 한다.
    /// git 은 여기서 띄우지 않는다 — 서비스는 상태 락을 쥔 채 이 함수를 부르는데 CreateProcess 는 느리다(부하가 있으면 초 단위).
    /// 읽기 태스크가 첫 요청을 처리하기 전에(락 밖에서) 띄운다
    pub fn new(exec: &GitExecutor, repo_path: &str, args: &[String], start_at: u64) -> LogCursor {
        let token = CancellationToken::new();
        let meta = Arc::new(Mutex::new(Meta {
            position: start_at,
            next_position: start_at,
            last_hash: None,
            last_used: Instant::now(),
            pending: 0,
            failed: false,
            closed: false,
            exhausted: false,
            overflowed: false,
            pid: None,
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        let reader = Reader {
            launch: Some(Launch { exec: exec.clone(), repo_path: repo_path.to_string(), args: args.to_vec() }),
            stream: None,
            splitter: NulRecordSplitter::with_limit(MAX_STREAM_RECORD),
            buffered: VecDeque::new(),
            done: false,
            overflowed: false,
            token: token.clone(),
            chunk: vec![0u8; READ_CHUNK],
        };
        let task = tokio::spawn(run_reader(reader, rx, meta.clone()));
        LogCursor { meta, tx, task, token }
    }

    /// git 프로세스 PID(테스트용). 아직 띄우기 전이거나 띄우지 못했으면 None
    pub fn pid(&self) -> Option<u32> {
        lock(&self.meta).pid
    }

    /// 닫혔거나 실패했거나 선읽기 상한으로 끊긴 커서는 재사용하면 안 된다
    pub fn usable(&self) -> bool {
        let m = lock(&self.meta);
        !m.failed && !m.closed && !m.overflowed
    }

    /// 마지막 위치·해시를 앵커로 남겨도 되는가(상한으로 끊긴 커서는 재사용은 못 해도 위치는 믿을 수 있다)
    pub fn anchorable(&self) -> bool {
        let m = lock(&self.meta);
        !m.failed && !m.closed
    }

    /// 끝까지 읽어 더 내보낼 커밋이 없다. 그 뒤에 생긴 커밋(첫 fetch·첫 커밋)은 이 커서로는 볼 수 없다
    pub fn ended(&self) -> bool {
        let m = lock(&self.meta);
        m.exhausted && m.pending == 0
    }

    /// 읽는 중이거나 대기 중인 요청이 있는지
    pub fn reading(&self) -> bool {
        lock(&self.meta).pending > 0
    }

    pub fn position(&self) -> u64 {
        lock(&self.meta).position
    }

    pub fn next_position(&self) -> u64 {
        lock(&self.meta).next_position
    }

    pub fn last_hash(&self) -> Option<String> {
        lock(&self.meta).last_hash.clone()
    }

    fn last_used(&self) -> Instant {
        lock(&self.meta).last_used
    }

    /// count 개를 읽는다. 호출 시점에 시작 위치를 예약하고 요청을 큐에 넣는다(순서가 곧 읽기 순서). 돌려준 future 를 기다려 결과를 받는다
    pub fn read(&self, count: usize) -> impl Future<Output = Result<LogPage, CursorError>> + Send + 'static {
        self.enqueue(None, count)
    }

    /// 먼저 한 커밋을 읽어 `expected` 와 같은지 확인하고, 같으면 이어서 count 개를 읽는다(앵커에서 다시 시작한 커서용).
    /// 다르면 `Mismatch`. 확인과 읽기를 한 요청으로 묶어 그 사이에 다른 읽기가 끼지 못하게 한다
    pub fn verify_then_read(&self, expected: String, count: usize) -> impl Future<Output = Result<LogPage, CursorError>> + Send + 'static {
        self.enqueue(Some(expected), count)
    }

    fn enqueue(&self, verify: Option<String>, count: usize) -> impl Future<Output = Result<LogPage, CursorError>> + Send + 'static {
        {
            let mut m = lock(&self.meta);
            // skip 이 u64 끝까지 올 수 있다(`1e300` → 포화) — 넘쳐도 패닉하지 않고 포화한다
            m.next_position = m.next_position.saturating_add(count as u64).saturating_add(u64::from(verify.is_some()));
            m.pending += 1;
        }
        let (reply, rx) = oneshot::channel();
        // 읽기 태스크가 이미 끝났으면 reply 가 버려져 아래에서 Aborted 가 된다
        let _ = self.tx.send(Req { verify, count, reply });
        let guard = PendingGuard { meta: self.meta.clone(), settled: false };
        async move {
            // 통째로 옮겨 담는다(필드만 캡처하면 Drop 이 future 가 아니라 여기서 돈다)
            let mut guard = guard;
            let r = rx.await;
            guard.settled = true;
            r.unwrap_or(Err(CursorError::Exec(ExecError::Aborted)))
        }
    }

    /// 커서를 닫는다. 토큰을 취소하고 읽기 태스크를 중단해 **`GitStream` 을 버린다**(프로세스 트리 종료). 여러 번 불러도 된다
    pub fn dispose(&self) {
        lock(&self.meta).closed = true;
        self.token.cancel();
        self.task.abort();
    }
}

impl Drop for LogCursor {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// 읽기 태스크가 첫 요청 전에 git 을 띄우는 데 필요한 것
struct Launch {
    exec: GitExecutor,
    repo_path: String,
    args: Vec<String>,
}

struct Reader {
    /// 아직 띄우지 않았으면 Some — 첫 요청을 처리하기 전에 [`Reader::ensure_started`] 가 띄운다
    launch: Option<Launch>,
    stream: Option<GitStream>,
    splitter: NulRecordSplitter,
    buffered: VecDeque<Commit>,
    done: bool,
    /// 원격 스트림이 선읽기 상한으로 끊겼다(스트림은 이미 버렸다). 쌓인 만큼은 낸다
    overflowed: bool,
    token: CancellationToken,
    /// 읽기 버퍼(요청마다 새로 잡지 않는다)
    chunk: Vec<u8>,
}

impl Reader {
    /// 아직 안 띄웠으면 git 을 띄우고(실행기가 blocking 풀·원격 시작을 맡는다) PID 를 meta 에 적는다.
    /// 띄우는 도중 커서가 닫히면(abort) 결과가 버려지며 스트림의 Drop 이 프로세스 트리·원격 명령을 죽인다
    async fn ensure_started(&mut self, meta: &Mutex<Meta>) -> Result<(), ExecError> {
        let Some(l) = self.launch.take() else { return Ok(()) };
        let started = l.exec.stream(&l.repo_path, &l.args, &RunOpts::with_cancel(self.token.clone())).await?;
        lock(meta).pid = started.pid();
        self.stream = Some(started);
        Ok(())
    }

    fn push_records(&mut self, records: Vec<String>) {
        for r in records {
            if let Some(c) = parse_log_record(&r) {
                self.buffered.push_back(c);
            }
        }
    }

    /// 버퍼에 count 개보다 많이(다음 페이지 존재 여부를 알기 위해 하나 더) 쌓일 때까지, 또는 끝까지 읽는다
    ///
    /// 한 번 채우는 동안 읽는 바이트는 [`DEFAULT_MAX_BUFFER`](실행기 `run` 의 출력 상한과 같은 64MB)를 넘지 않는다 —
    /// 레코드 상한만으로는 큰 메시지의 커밋 1000개가 페이지 하나에 수 GB 를 쌓을 수 있다(압축돼 저장소는 작아도 된다).
    async fn fill(&mut self, count: usize) -> Result<(), ExecError> {
        let mut read_bytes = 0usize;
        while self.buffered.len() <= count && !self.done && !self.overflowed {
            let Some(stream) = self.stream.as_mut() else { return Err(ExecError::Aborted) };
            let n = match stream.read(&mut self.chunk).await {
                Ok(n) => n,
                // SSH: 소비가 늦어 선읽기 상한을 넘었다. 원격은 이미 죽었다 — 실패가 아니라 이 커서를 접고 쌓인 만큼만 낸다
                Err(e) if is_read_ahead_exceeded(&e) => {
                    self.stream = None;
                    self.overflowed = true;
                    break;
                }
                // 토큰으로 깨어난 읽기 오류는 취소다
                Err(_) if self.token.is_cancelled() => return Err(ExecError::Aborted),
                Err(e) => return Err(ExecError::Other { message: e.to_string() }),
            };
            if n == 0 {
                let stream = self.stream.take().expect("위에서 확인했다");
                match stream.finish().await {
                    Ok(()) => {}
                    // 커밋이 하나도 없는 새 저장소는 빈 목록으로 취급한다
                    Err(ExecError::Git { stderr, .. }) if is_empty_repo_error(&stderr) => {}
                    Err(e) => return Err(e),
                }
                self.done = true;
                let rest = self.splitter.end();
                self.push_records(rest);
            } else {
                read_bytes = read_bytes.saturating_add(n);
                if read_bytes > DEFAULT_MAX_BUFFER {
                    return Err(ExecError::Other { message: "git log page too large".into() });
                }
                // 한 레코드가 상한을 넘으면 오류로 끝낸다(커서는 실패 처리되고 스트림이 버려진다)
                let records = self.splitter.push_checked(&self.chunk[..n]).map_err(|_| ExecError::Other { message: "git log record too long".into() })?;
                self.push_records(records);
            }
        }
        Ok(())
    }

    async fn take(&mut self, count: usize) -> Result<(Vec<Commit>, bool), ExecError> {
        self.fill(count).await?;
        let n = count.min(self.buffered.len());
        let commits: Vec<Commit> = self.buffered.drain(..n).collect();
        let has_more = !self.buffered.is_empty() || !self.done;
        Ok((commits, has_more))
    }
}

/// 한 번 읽은 결과를 메타에 반영한다(`readNow` 의 끝부분)
fn commit_progress(meta: &Mutex<Meta>, reader: &Reader, reserved: usize, commits: &[Commit]) {
    let mut m = lock(meta);
    m.position = m.position.saturating_add(commits.len() as u64);
    // 끝에 닿아 덜 받았으면 예약했던 위치를 되돌린다
    m.next_position = m.next_position.saturating_sub(reserved.saturating_sub(commits.len()) as u64);
    if let Some(last) = commits.last() {
        m.last_hash = Some(last.hash.clone());
    }
    m.exhausted = reader.done && reader.buffered.is_empty();
    m.overflowed = reader.overflowed;
    m.last_used = Instant::now();
}

async fn serve(reader: &mut Reader, meta: &Mutex<Meta>, req: &Req) -> Result<LogPage, CursorError> {
    lock(meta).last_used = Instant::now();
    reader.ensure_started(meta).await.map_err(CursorError::Exec)?;
    if let Some(expected) = &req.verify {
        let (head, _) = reader.take(1).await.map_err(CursorError::Exec)?;
        commit_progress(meta, reader, 1, &head);
        // 확인용 커밋을 받기 전에 상한으로 끊겼다 — 확인하지 못했으니 아무것도 내지 않는다(앵커는 그대로라 다음 요청이 다시 확인한다)
        if head.is_empty() && reader.overflowed {
            return Err(no_progress());
        }
        if head.first().map(|c| c.hash.as_str()) != Some(expected.as_str()) {
            return Err(CursorError::Mismatch);
        }
    }
    let (commits, has_more) = reader.take(req.count).await.map_err(CursorError::Exec)?;
    commit_progress(meta, reader, req.count, &commits);
    if commits.is_empty() && reader.overflowed {
        return Err(no_progress());
    }
    Ok(LogPage { commits, has_more })
}

/// 선읽기 상한으로 커밋을 하나도 받지 못했다(빈 페이지로 내면 renderer 가 백오프 없이 되풀이한다)
fn no_progress() -> CursorError {
    CursorError::Exec(ExecError::Other { message: "SSH 선읽기 상한으로 첫 커밋을 받지 못했습니다".into() })
}

/// 읽기 태스크. 요청을 도착 순서대로 하나씩 처리한다. 오류가 나면 커서를 실패로 표시하고 끝낸다 — 끝나며 `Reader`(= 스트림)가 버려진다
async fn run_reader(mut reader: Reader, mut rx: mpsc::UnboundedReceiver<Req>, meta: Arc<Mutex<Meta>>) {
    while let Some(req) = rx.recv().await {
        let result = serve(&mut reader, &meta, &req).await;
        let fatal = matches!(result, Err(CursorError::Exec(_)));
        if fatal {
            lock(&meta).failed = true;
        }
        let _ = req.reply.send(result);
        if fatal {
            break;
        }
    }
}

// ── 서비스 ────────────────────────────────────────────

/// 사이드카에 알리는 일(`{t:'note',op,repoId}`). 사이드카의 `RepoService.noteLog` 가 받는다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteOp {
    /// 첫 로그 페이지가 나갔다(성공·실패·취소 모두) — 백그라운드 조회를 푼다
    FirstPage,
    /// 이어 읽으려던 위치의 커밋이 바뀌었다 — 기억한 시그니처를 버린다
    Stale,
    /// Rust 의 fetch 가 ref 를 바꿨다 — 기억한 시그니처를 버린다(`repo:changed` 보다 먼저 보낸다: 그 이벤트로 다시 계산하는 쪽이 낡은 값을 쓰지 않게)
    FetchChanged,
}

impl NoteOp {
    /// 프로토콜의 `op` 문자열(`src/sidecar/protocol.ts` 의 `NOTE_OPS`)
    pub fn as_str(self) -> &'static str {
        match self {
            NoteOp::FirstPage => "logFirstPage",
            NoteOp::Stale => "logStale",
            NoteOp::FetchChanged => "fetchChanged",
        }
    }
}

pub type NoteSink = Arc<dyn Fn(NoteOp, &str) + Send + Sync>;

struct Entry {
    repo_id: String,
    target: Target,
    cursor: Arc<LogCursor>,
}

struct Anchor {
    repo_id: String,
    target: Target,
    position: u64,
    hash: String,
}

/// 키마다 하나씩 기억하는 앵커. 개수 상한을 넘으면 가장 오래전에 넣은 것부터 버린다(`order` 가 삽입 순서)
#[derive(Default)]
struct Anchors {
    map: HashMap<String, Anchor>,
    order: VecDeque<String>,
}

impl Anchors {
    fn len(&self) -> usize {
        self.map.len()
    }

    fn get(&self, key: &str) -> Option<&Anchor> {
        self.map.get(key)
    }

    fn insert(&mut self, key: String, anchor: Anchor) {
        self.order.retain(|k| k != &key);
        self.order.push_back(key.clone());
        self.map.insert(key, anchor);
        while self.order.len() > MAX_ANCHORS {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
    }

    fn remove(&mut self, key: &str) {
        if self.map.remove(key).is_some() {
            self.order.retain(|k| k != key);
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(&Anchor) -> bool) {
        self.map.retain(|_, a| keep(a));
        let map = &self.map;
        self.order.retain(|k| map.contains_key(k));
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

#[derive(Default)]
struct State {
    cursors: HashMap<String, Entry>,
    anchors: Anchors,
}

impl State {
    /// 커서를 닫는다. 이어 읽을 수 있도록 마지막 위치·해시를 앵커로 남긴다
    fn retire(&mut self, key: &str) {
        let Some(e) = self.cursors.remove(key) else { return };
        if e.cursor.anchorable() {
            if let Some(hash) = e.cursor.last_hash() {
                self.anchors.insert(key.to_string(), Anchor { repo_id: e.repo_id, target: e.target, position: e.cursor.position(), hash });
            }
        }
        e.cursor.dispose();
    }

    /// 앵커를 남기지 않고 닫는다(요청 취소·저장소 제거)
    fn drop_cursor(&mut self, key: &str) {
        if let Some(e) = self.cursors.remove(key) {
            e.cursor.dispose();
        }
    }
}

struct Inner {
    state: Mutex<State>,
    mirror: RepoMirror,
    sink: Mutex<Option<NoteSink>>,
    sweeper: AtomicBool,
    idle: Duration,
    sweep: Duration,
}

impl Inner {
    fn notify(&self, op: NoteOp, repo_id: &str) {
        let sink = lock(&self.sink).clone();
        if let Some(s) = sink {
            s(op, repo_id);
        }
    }

    /// 유휴 시간이 지난 커서를 닫는다(읽는 중인 커서는 건드리지 않는다). 남은 커서 수를 돌려준다
    fn sweep_locked(&self, st: &mut State, now: Instant) -> usize {
        let stale: Vec<String> = st
            .cursors
            .iter()
            .filter(|(_, e)| !e.cursor.reading() && now.saturating_duration_since(e.cursor.last_used()) > self.idle)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            st.retire(&k);
        }
        st.cursors.len()
    }
}

/// 저장소 활성화·커서 맵을 가진 `git:log` 서비스. 커서는 활성 저장소 하나에만 유지한다
pub struct LogService {
    inner: Arc<Inner>,
}

impl LogService {
    pub fn new(mirror: RepoMirror) -> Self {
        Self::with_timing(mirror, CURSOR_IDLE, CURSOR_SWEEP)
    }

    /// 유휴 시간·훑는 주기를 정한다(테스트용)
    pub fn with_timing(mirror: RepoMirror, idle: Duration, sweep: Duration) -> Self {
        LogService { inner: Arc::new(Inner { state: Mutex::default(), mirror, sink: Mutex::new(None), sweeper: AtomicBool::new(false), idle, sweep }) }
    }

    pub fn set_sink(&self, sink: NoteSink) {
        *lock(&self.inner.sink) = Some(sink);
    }

    /// 열려 있는 커서 수
    pub fn cursor_count(&self) -> usize {
        lock(&self.inner.state).cursors.len()
    }

    /// 기억 중인 앵커 수(테스트용)
    pub fn anchor_count(&self) -> usize {
        lock(&self.inner.state).anchors.len()
    }

    /// 열려 있는 커서의 git PID(테스트용)
    pub fn cursor_pids(&self) -> Vec<u32> {
        lock(&self.inner.state).cursors.values().filter_map(|e| e.cursor.pid()).collect()
    }

    /// 유휴 커서를 훑는다(`now` 기준). 남은 커서 수를 돌려준다
    pub fn sweep_idle(&self, now: Instant) -> usize {
        let mut st = lock(&self.inner.state);
        self.inner.sweep_locked(&mut st, now)
    }

    /// 훑는 태스크를 하나만 띄운다. 커서가 없어지면 스스로 끝나고 다음 커서가 생길 때 다시 뜬다. 상태 락을 쥔 채 부른다
    fn ensure_sweeper(&self) {
        if self.inner.sweeper.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let period = self.inner.sweep;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let Some(inner) = weak.upgrade() else { break };
                // 비었는지 보는 것과 깃발을 내리는 것을 같은 락 안에서 한다(그 사이에 만들어진 커서의 훑기가 사라지지 않게)
                let remaining = {
                    let mut st = lock(&inner.state);
                    let n = inner.sweep_locked(&mut st, Instant::now());
                    if n == 0 {
                        inner.sweeper.store(false, Ordering::SeqCst);
                    }
                    n
                };
                if remaining == 0 {
                    break;
                }
            }
        });
    }

    /// 활성 저장소가 아닌 저장소의 커서를 모두 닫는다(`repos:watch` 를 사이드카로 넘기기 전). `None` 이면 전부 닫는다
    pub fn close_others(&self, active: Option<&str>) {
        let mut st = lock(&self.inner.state);
        let keys: Vec<String> = st.cursors.iter().filter(|(_, e)| Some(e.repo_id.as_str()) != active).map(|(k, _)| k.clone()).collect();
        for k in keys {
            st.retire(&k);
        }
    }

    /// 미러가 바뀐 뒤 불린다: 미러에서 사라졌거나 대상(경로·환경)이 바뀐 저장소의 커서와 앵커를 지운다(앵커 없이 닫는다)
    pub fn sync_mirror(&self) {
        let mirror = &self.inner.mirror;
        let same = |repo_id: &str, target: &Target| matches!(mirror.resolve(repo_id), Resolved::Rust(t) if &t == target);
        let mut st = lock(&self.inner.state);
        let gone: Vec<String> = st.cursors.iter().filter(|(_, e)| !same(&e.repo_id, &e.target)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            st.drop_cursor(&k);
        }
        st.anchors.retain(|a| same(&a.repo_id, &a.target));
    }

    /// 모든 커서를 닫고 앵커를 지운다(앱 종료)
    pub fn close_all(&self) {
        let mut st = lock(&self.inner.state);
        for (_, e) in st.cursors.drain() {
            e.cursor.dispose();
        }
        st.anchors.clear();
    }

    /// 커밋 한 페이지. 직전 페이지에 이어지는 요청은 열려 있는 커서에서 이어 읽는다.
    /// 커서·앵커를 만지는 앞부분은 await 없이 한 번의 락 안에서 끝난다(TS 가 첫 await 전에 하는 일과 같다).
    pub async fn log(&self, repo_id: &str, exec: &GitExecutor, target: &Target, req: &LogRequest, token: &CancellationToken) -> Result<LogPage, LogError> {
        let first = req.skip == 0;
        let key = format!("{repo_id}\0{}", scope_key(req.scope.as_ref()));
        let started = self.start(repo_id, &key, exec, target, req, token);
        let (cursor, fut) = match started {
            Ok(s) => s,
            Err(e) => {
                if first {
                    self.inner.notify(NoteOp::FirstPage, repo_id);
                }
                return Err(e);
            }
        };
        let result = tokio::select! {
            biased;
            () = token.cancelled() => {
                // 읽는 도중 취소되면 커서 위치를 신뢰할 수 없으므로 버린다
                let mut st = lock(&self.inner.state);
                if st.cursors.get(&key).is_some_and(|e| Arc::ptr_eq(&e.cursor, &cursor)) {
                    st.drop_cursor(&key);
                } else {
                    cursor.dispose();
                }
                Err(LogError::Exec(ExecError::Aborted))
            }
            r = fut => match r {
                Ok(page) => Ok(page),
                Err(CursorError::Exec(e)) => Err(LogError::Exec(e)),
                Err(CursorError::Mismatch) => {
                    // 그 사이 ref 가 움직였다 — 이어 붙이면 중복·누락이 생기므로 renderer 가 처음부터 다시 읽게 한다
                    let mut st = lock(&self.inner.state);
                    if st.cursors.get(&key).is_some_and(|e| Arc::ptr_eq(&e.cursor, &cursor)) {
                        st.retire(&key);
                    } else {
                        cursor.dispose();
                    }
                    st.anchors.remove(&key);
                    drop(st);
                    // 기억해 둔 시그니처도 낡았다 — 감시가 알아채기 전에 활동 조회가 낡은 캐시를 쓰지 않게 한다
                    self.inner.notify(NoteOp::Stale, repo_id);
                    Err(LogError::Stale)
                }
            },
        };
        // 첫 페이지가 나가면 백그라운드 조회를 풀어 준다(실패·취소해도 풀어 준다 — 오류 화면에서 다른 조회가 막히면 안 된다)
        if first {
            self.inner.notify(NoteOp::FirstPage, repo_id);
        }
        result
    }

    /// `log` 의 동기 앞부분: 다른 저장소·범위의 커서를 닫고, 재사용할 커서를 고르거나 새로 만들고, 읽기 요청을 큐에 넣는다.
    /// 요청을 락 안에서 넣어야 그 사이에 끼어든 다른 요청이 예약 전의 위치를 보고 같은 커서를 쓰지 못한다
    fn start(&self, repo_id: &str, key: &str, exec: &GitExecutor, target: &Target, req: &LogRequest, token: &CancellationToken) -> Result<(Arc<LogCursor>, ReadFuture), LogError> {
        if token.is_cancelled() {
            return Err(LogError::Exec(ExecError::Aborted));
        }
        let skip = req.skip;
        let mut st = lock(&self.inner.state);
        let others: Vec<String> = st.cursors.keys().filter(|k| k.as_str() != key).cloned().collect();
        for k in others {
            st.retire(&k);
        }
        // 활성 저장소의 앵커만 남긴다(다른 저장소로 옮겨 간 뒤의 앵커는 쓸모없다. 개수 상한은 `Anchors` 가 따로 건다)
        st.anchors.retain(|a| a.repo_id == repo_id);
        if skip == 0 {
            st.anchors.remove(key);
        }
        // 예약된 다음 위치가 맞고 살아 있는 커서만 이어 쓴다. 끝까지 읽은 커서는 다시 열어야 그 뒤의 커밋이 보인다
        // (빈 저장소의 첫 페이지 뒤 위치가 0 으로 돌아가 같은 커서를 계속 이어 쓰면, 첫 fetch 가 끝나도 목록이 빈 채로 남는다)
        let reusable = st.cursors.get(key).filter(|e| e.cursor.usable() && !e.cursor.ended() && e.cursor.next_position() == skip).map(|e| e.cursor.clone());
        let (cursor, verify) = match reusable {
            Some(c) => (c, None),
            None => {
                st.retire(key);
                // 닫힌 커서의 마지막 커밋 위치에서 다시 시작하면, 그 커밋이 그대로인지 한 개 겹쳐 읽어 확인한다
                let anchor = st.anchors.get(key).filter(|a| a.position == skip && skip > 0).map(|a| a.hash.clone());
                let start = if anchor.is_some() { skip - 1 } else { skip };
                let args = log_args(req.scope.as_ref(), start).map_err(|message| LogError::Exec(ExecError::Other { message }))?;
                let cursor = Arc::new(LogCursor::new(exec, target.path(), &args, start));
                st.cursors.insert(key.to_string(), Entry { repo_id: repo_id.to_string(), target: target.clone(), cursor: cursor.clone() });
                self.ensure_sweeper();
                (cursor, anchor)
            }
        };
        let fut: ReadFuture = match verify {
            Some(hash) => Box::pin(cursor.verify_then_read(hash, req.count)),
            None => Box::pin(cursor.read(req.count)),
        };
        Ok((cursor, fut))
    }
}

type ReadFuture = Pin<Box<dyn Future<Output = Result<LogPage, CursorError>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(v: Value) -> Result<LogRequest, String> {
        parse_query(Some(&v))
    }

    fn err(v: Value) -> String {
        q(v).unwrap_err()
    }

    #[test]
    fn 질의의_count_skip_은_숫자만() {
        let r = q(json!({ "skip": 0, "count": 100 })).unwrap();
        assert_eq!((r.count, r.skip), (100, 0));
        assert!(r.scope.is_none());
        // 숫자가 아니면 TS 도 invalid query 다
        for bad in [
            json!(null),
            json!(5),
            json!("x"),
            json!([]),
            json!({}),
            json!({ "count": 10 }),
            json!({ "skip": 0 }),
            json!({ "skip": 0, "count": "100" }),
            json!({ "skip": "0", "count": 100 }),
            json!({ "skip": 0, "count": null }),
            json!({ "skip": 0, "count": true }),
            json!({ "skip": 0, "count": [10] }),
            json!({ "skip": 0, "count": "-c" }),
            json!({ "skip": "0 --output=x", "count": 1 }),
        ] {
            assert_eq!(err(bad.clone()), "invalid query", "{bad}");
        }
        assert_eq!(parse_query(None).unwrap_err(), "invalid query");
    }

    #[test]
    fn count_는_1에서_5000_skip_은_0_이상_정수로_맞춘다() {
        let n = |count: f64, skip: f64| {
            let r = q(json!({ "count": count, "skip": skip })).unwrap();
            (r.count, r.skip)
        };
        assert_eq!(n(0.0, 0.0), (1, 0));
        assert_eq!(n(-4.0, -3.0), (1, 0));
        assert_eq!(n(2.7, 1.9), (2, 1));
        assert_eq!(n(99999.0, 250.0), (5000, 250));
        assert_eq!(n(5000.0, 0.0), (5000, 0));
        assert_eq!(n(0.5, 0.5), (1, 0));
    }

    #[test]
    fn 질의_오류_순서는_query_scope_순이다() {
        assert_eq!(err(json!({ "skip": 0, "count": "1", "scope": { "solo": ["-c"] } })), "invalid query");
        assert_eq!(err(json!({ "skip": 0, "count": 1, "scope": { "solo": ["-c"] } })), "invalid solo");
        assert_eq!(err(json!({ "skip": 0, "count": 1, "scope": null })), "invalid scope");
        // hide 의 해시는 커서를 만지기 전에 거부된다
        assert_eq!(err(json!({ "skip": 0, "count": 1, "scope": { "hide": ["a".repeat(40)] } })), "invalid hide");
    }

    #[test]
    fn 빈_저장소_오류는_대소문자를_무시한다() {
        for s in [
            "fatal: your current branch 'main' does not have any commits yet",
            "fatal: bad default revision 'HEAD'",
            "fatal: ambiguous argument 'HEAD': unknown revision or path not in the working tree.",
            "FATAL: Bad Revision 'HEAD'",
        ] {
            assert!(is_empty_repo_error(s), "{s}");
        }
        for s in ["", "fatal: not a git repository", "fatal: bad revision 'main'", "fatal: ambiguous argument 'main'"] {
            assert!(!is_empty_repo_error(s), "{s}");
        }
    }

    fn anchor(repo: &str, position: u64) -> Anchor {
        Anchor { repo_id: repo.into(), target: Target::Local { path: "/r".into() }, position, hash: "h".into() }
    }

    #[test]
    fn 앵커는_상한을_넘으면_오래된_것부터_버린다() {
        let mut a = Anchors::default();
        for i in 0..MAX_ANCHORS + 5 {
            a.insert(format!("k{i}"), anchor("r", i as u64));
        }
        assert_eq!(a.len(), MAX_ANCHORS);
        assert!(a.get("k0").is_none() && a.get("k4").is_none());
        assert!(a.get("k5").is_some() && a.get(&format!("k{}", MAX_ANCHORS + 4)).is_some());
        // 같은 키를 다시 넣으면 가장 최근으로 올라가 개수가 늘지 않는다
        a.insert("k5".into(), anchor("r", 99));
        a.insert("new".into(), anchor("r", 1));
        assert_eq!(a.len(), MAX_ANCHORS);
        assert!(a.get("k5").is_some() && a.get("k6").is_none());
        assert_eq!(a.get("k5").map(|x| x.position), Some(99));
    }

    #[test]
    fn 앵커_제거와_걸러내기는_삽입_순서를_함께_정리한다() {
        let mut a = Anchors::default();
        a.insert("a".into(), anchor("x", 1));
        a.insert("b".into(), anchor("y", 2));
        a.insert("c".into(), anchor("x", 3));
        a.remove("a");
        a.retain(|n| n.repo_id == "x");
        assert_eq!((a.len(), a.order.len()), (1, 1));
        // 정리한 키가 순서에 남아 있으면 상한 계산이 틀어진다
        for i in 0..MAX_ANCHORS {
            a.insert(format!("n{i}"), anchor("x", 0));
        }
        assert_eq!(a.len(), MAX_ANCHORS);
        assert!(a.get("c").is_none());
        a.clear();
        assert_eq!((a.len(), a.order.len()), (0, 0));
    }

    #[test]
    fn 오류_코드와_문구() {
        assert_eq!(LogError::Stale.envelope_code(), "STALE");
        assert_eq!(LogError::Stale.to_string(), "commit history changed while paging");
        assert_eq!(LogError::Exec(ExecError::Aborted).envelope_code(), "ABORTED");
        assert_eq!(LogError::Exec(ExecError::Other { message: "m".into() }).envelope_code(), "ERROR");
        assert_eq!(NoteOp::FirstPage.as_str(), "logFirstPage");
        assert_eq!(NoteOp::Stale.as_str(), "logStale");
        assert_eq!(NoteOp::FetchChanged.as_str(), "fetchChanged");
    }
}
