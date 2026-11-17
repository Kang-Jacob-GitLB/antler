//! 저장소마다 공유하는 상태: **첫 페이지 문**과 **ref 시그니처 저장소**. TS `RepoService` 의 `armFirstPage`·`afterFirstPage`·`openFirstPage`·`signature`·`invalidateSignature`
//! (`src/main/services/RepoService.ts`)와 같은 의미다. 소비자(refsDetails·activity·wipStatus·RepoWatcher)는 뒤 PR(3b·4·6·7)에서 온다.
//!
//! - **문**: 저장소가 활성화되면 닫히고(`arm`), 첫 로그 페이지가 나가면(`open`) 또는 유예([`FIRST_PAGE_GRACE`])가 지나면 열린다. 첫 화면에 필요 없는 백그라운드 조회는
//!   [`RepoState::after_first_page`] 를 기다린 뒤 git 을 띄운다. 문이 없는 저장소는 기다리지 않는다. 타이머를 따로 두지 않고 문에 마감 시각을 둔다 —
//!   대기자는 마감까지만 기다리고, 마감이 지난 문은 다음에 만지는 순간 열린 것으로 치워진다. 그래서 재활성화한 새 문을 낡은 타이머가 일찍 열 수 없다.
//! - **시그니처**: 기억한 값(`known`)·진행 중 계산(`inflight`)·반영한 최신 순번(`applied`)·순번 발급기(`counter`). 계산은 시작할 때 순번을 받고, 끝났을 때 **더 나중에 시작한 계산(또는 무효화)이
//!   이미 반영되지 않았을 때만**(`seq > applied`) 기록한다. 이력이 바뀐 것을 알면 [`RepoState::invalidate_signature`] 가 기억·진행 중 계산을 버리고 순번을 올려, 그 전에 시작한 계산이 다시 기록하지 못하게 한다.
//!   **기억한 값은 감시(PR 7b)가 갱신하기 전까지 소비하지 않는다** — 감시와 소비자가 다른 프로세스에 있으면 낡는다(계획 §2).
//! - 공유 계산([`RepoState::shared_signature`])은 spawn 한 태스크라 기다리는 쪽이 모두 떠나도 끝까지 간다(TS 의 Promise 와 같다). 제한 시간(20초)이 지나면 계산에 취소를 보낸다.
//!   취소 토큰을 건 호출([`RepoState::compute_signature`])은 합치지 않고 따로 계산한다 — 진행 중이던 계산이 ref 변경 전 값일 수 있어 합치면 변화를 놓친다.
//!
//! 이 모듈은 git 을 부르지 않는다. 계산(future)은 호출자가 넘긴다 — 큐 슬롯·실행기는 호출자의 몫이다.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use tokio::sync::watch;
use tokio::time::{sleep, timeout_at, Instant};
use tokio_util::sync::CancellationToken;

use super::log_service::NoteOp;
use super::signature::Signature;
use crate::git::exec::ExecError;

/// 첫 페이지가 안 나가도 문을 여는 유예. TS `FIRST_PAGE_GRACE_MS`
pub const FIRST_PAGE_GRACE: Duration = Duration::from_millis(1_500);
/// 공유 시그니처 계산의 제한. TS `SIGNATURE_TIMEOUT_MS`
pub const SIGNATURE_TIMEOUT: Duration = Duration::from_secs(20);

/// 시그니처 계산 실패. 합류한 호출자들이 같은 결과를 받도록 복제할 수 있다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigError {
    /// 취소·제한 시간으로 끝났다(TS `AbortError`)
    Aborted,
    /// git 이 실패했다(TS `GitError` — envelope 의 code 가 GIT 이다)
    Git(String),
    Failed(String),
}

impl From<ExecError> for SigError {
    fn from(e: ExecError) -> Self {
        match e {
            ExecError::Aborted => SigError::Aborted,
            ExecError::Git { message, .. } => SigError::Git(message),
            other => SigError::Failed(other.to_string()),
        }
    }
}

impl From<SigError> for ExecError {
    fn from(e: SigError) -> Self {
        match e {
            SigError::Aborted => ExecError::Aborted,
            SigError::Git(message) => ExecError::Git { message, code: -1, stderr: String::new(), args: Vec::new() },
            SigError::Failed(message) => ExecError::Other { message },
        }
    }
}

type SharedSig =Shared<BoxFuture<'static, Result<Signature, SigError>>>;

struct Gate {
    deadline: Instant,
    open: watch::Sender<bool>,
}

struct Inflight {
    /// 같은 저장소에서 나중에 시작한 계산과 구별한다(TS 는 Promise 의 동일성으로 본다)
    id: u64,
    fut: SharedSig,
}

#[derive(Default)]
struct Inner {
    gates: HashMap<String, Gate>,
    known: HashMap<String, Signature>,
    inflight: HashMap<String, Inflight>,
    applied: HashMap<String, u64>,
    counter: u64,
    inflight_ids: u64,
}

impl Inner {
    /// 마감이 지난 문을 연다(TS 의 타이머가 하는 일)
    fn purge(&mut self, now: Instant) {
        let due: Vec<String> = self.gates.iter().filter(|(_, g)| g.deadline <= now).map(|(k, _)| k.clone()).collect();
        for k in due {
            if let Some(g) = self.gates.remove(&k) {
                g.open.send_replace(true);
            }
        }
    }

    fn invalidate(&mut self, repo_id: &str) {
        self.known.remove(repo_id);
        self.inflight.remove(repo_id);
        self.counter += 1;
        self.applied.insert(repo_id.to_string(), self.counter);
    }

    fn next_seq(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }
}

/// 첫 페이지 문과 시그니처 저장소. 복제하면 같은 상태를 본다
#[derive(Clone)]
pub struct RepoState {
    inner: Arc<Mutex<Inner>>,
    grace: Duration,
    timeout: Duration,
}

impl Default for RepoState {
    fn default() -> Self {
        Self::with_timing(FIRST_PAGE_GRACE, SIGNATURE_TIMEOUT)
    }
}

/// 공유 계산이 끝나면(패닉 포함) 아직 자기 자신일 때만 진행 중 표에서 뺀다
struct Cleanup {
    state: RepoState,
    repo_id: String,
    id: u64,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let mut g = self.state.lock();
        if g.inflight.get(&self.repo_id).is_some_and(|i| i.id == self.id) {
            g.inflight.remove(&self.repo_id);
        }
    }
}

impl RepoState {
    /// 유예·공유 계산 제한을 정한다(시험용)
    pub fn with_timing(grace: Duration, timeout: Duration) -> Self {
        RepoState { inner: Arc::default(), grace, timeout }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ── 첫 페이지 문 ──────────────────────────────────────

    /// 저장소가 활성화됐다 — 첫 페이지가 나갈 때(또는 유예 뒤)까지 [`after_first_page`](Self::after_first_page) 가 기다리게 한다.
    /// 다시 활성화된 저장소의 이전 시그니처는 낡았을 수 있어 **먼저** 버린다. 문이 이미 있으면 유예를 다시 걸지 않는다
    pub fn arm(&self, repo_id: &str) {
        let mut g = self.lock();
        g.invalidate(repo_id);
        let now = Instant::now();
        g.purge(now);
        if g.gates.contains_key(repo_id) {
            return;
        }
        let (open, _) = watch::channel(false);
        g.gates.insert(repo_id.to_string(), Gate { deadline: now + self.grace, open });
    }

    /// 첫 화면에 필요 없는 백그라운드 조회는 이것을 기다린 뒤 git 을 띄운다. 문이 없으면(닫은 적 없거나 이미 열렸으면) 즉시 끝난다.
    /// 문은 **부르는 순간** 잡는다(돌려준 future 를 나중에 기다려도 그 사이 재활성화된 새 문이 아니라 이 문을 기다린다 — TS 가 Promise 를 먼저 쥐는 것과 같다)
    pub fn after_first_page(&self, repo_id: &str) -> impl Future<Output = ()> + Send + 'static {
        let wait = {
            let mut g = self.lock();
            g.purge(Instant::now());
            g.gates.get(repo_id).map(|gate| (gate.open.subscribe(), gate.deadline))
        };
        async move {
            if let Some((mut rx, deadline)) = wait {
                // 마감까지만 기다린다. 보낸 쪽이 사라져도(문이 치워졌다) 열린 것이다
                let _ = timeout_at(deadline, rx.wait_for(|opened| *opened)).await;
            }
        }
    }

    /// 첫 페이지가 나갔다 — 문을 연다(없으면 아무 일도 없다)
    pub fn open(&self, repo_id: &str) {
        if let Some(g) = self.lock().gates.remove(repo_id) {
            g.open.send_replace(true);
        }
    }

    /// 문이 닫혀 있는가(시험·진단용)
    pub fn is_armed(&self, repo_id: &str) -> bool {
        let mut g = self.lock();
        g.purge(Instant::now());
        g.gates.contains_key(repo_id)
    }

    // ── 시그니처 ──────────────────────────────────────────

    /// 기억한 시그니처
    pub fn known_signature(&self, repo_id: &str) -> Option<Signature> {
        self.lock().known.get(repo_id).copied()
    }

    /// 기억한 시그니처를 버리고, 그 전에 시작한 계산이 다시 기록하지 못하게 한다(이력이 바뀐 것을 알았을 때)
    pub fn invalidate_signature(&self, repo_id: &str) {
        self.lock().invalidate(repo_id);
    }

    /// Rust 가 알려 주는 일을 TS `noteLog` 와 같은 효과로 옮긴다 — 첫 페이지는 문을 열고, 이력 변경·fetch 변경은 시그니처를 버린다.
    /// (모르는 저장소를 거르는 것은 호출자의 몫이다 — TS `noteLog` 의 `config.repo` 확인)
    pub fn note(&self, op: NoteOp, repo_id: &str) {
        match op {
            NoteOp::FirstPage => self.open(repo_id),
            NoteOp::Stale | NoteOp::FetchChanged => self.invalidate_signature(repo_id),
        }
    }

    /// 취소 가능한 호출용: 합치지 않고 따로 계산한다(TS `computeSignature`). **순번은 이 함수를 부르는 순간** 받는다(큐에서 기다리는 동안 시작 순서가 정해진다).
    /// 계산이 성공했을 때만, 더 나중에 시작한 계산(또는 무효화)이 반영되지 않았다면 기억한다. 실패·취소는 아무것도 기록하지 않는다
    pub fn compute_signature<F>(&self, repo_id: &str, compute: F) -> impl Future<Output = Result<Signature, SigError>> + Send + 'static
    where
        F: Future<Output = Result<Signature, SigError>> + Send + 'static,
    {
        let seq = self.lock().next_seq();
        self.clone().finish(repo_id.to_string(), seq, compute)
    }

    async fn finish<F>(self, repo_id: String, seq: u64, compute: F) -> Result<Signature, SigError>
    where
        F: Future<Output = Result<Signature, SigError>> + Send + 'static,
    {
        let sig = compute.await?;
        let mut g = self.lock();
        if seq > g.applied.get(&repo_id).copied().unwrap_or(0) {
            g.applied.insert(repo_id.clone(), seq);
            g.known.insert(repo_id, sig);
        }
        Ok(sig)
    }

    /// 진행 중인 계산에 합류한다(없으면 `compute` 로 새로 시작한다 — 제한 시간 안에 끝나야 하고, 넘으면 `compute` 가 받은 토큰이 취소된다).
    /// `cancel` 은 **기다림만** 끊는다(계산은 자체 제한 시간까지 간다). 런타임 안에서 불러야 한다(계산을 spawn 한다)
    pub async fn shared_signature<F, Fut>(&self, repo_id: &str, compute: F, cancel: Option<&CancellationToken>) -> Result<Signature, SigError>
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Signature, SigError>> + Send + 'static,
    {
        let shared = self.join_or_start(repo_id, compute);
        let Some(cancel) = cancel else { return shared.await };
        if cancel.is_cancelled() {
            return Err(SigError::Aborted);
        }
        tokio::select! {
            r = shared => r,
            () = cancel.cancelled() => Err(SigError::Aborted),
        }
    }

    fn join_or_start<F, Fut>(&self, repo_id: &str, compute: F) -> SharedSig
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Signature, SigError>> + Send + 'static,
    {
        // 조회·순번·spawn·등록이 한 잠금 안이다 — 태스크의 정리(Cleanup)가 등록보다 먼저 돌아 낡은 항목이 남지 않게
        let mut g = self.lock();
        if let Some(i) = g.inflight.get(repo_id) {
            return i.fut.clone();
        }
        g.inflight_ids += 1;
        let id = g.inflight_ids;
        let seq = g.next_seq();
        let (state, repo, timeout) = (self.clone(), repo_id.to_string(), self.timeout);
        let handle = tokio::spawn(async move {
            let _cleanup = Cleanup { state: state.clone(), repo_id: repo.clone(), id };
            let token = CancellationToken::new();
            let mut work = Box::pin(state.finish(repo, seq, compute(token.clone())));
            tokio::select! {
                r = &mut work => r,
                () = sleep(timeout) => {
                    // 제한 시간: 계산에 취소를 보내고 끝나기를 기다린다(git 이 정리되고 Aborted 로 돌아온다)
                    token.cancel();
                    work.await
                }
            }
        });
        let fut: SharedSig = async move { handle.await.unwrap_or_else(|e| Err(SigError::Failed(e.to_string()))) }.boxed().shared();
        g.inflight.insert(repo_id.to_string(), Inflight { id, fut: fut.clone() });
        fut
    }

    /// 앱 종료: 기다리는 문을 모두 열고 기억을 비운다(TS `dispose`). 순번은 되돌리지 않는다
    pub fn dispose(&self) {
        let mut g = self.lock();
        for (_, gate) in g.gates.drain() {
            gate.open.send_replace(true);
        }
        g.known.clear();
        g.inflight.clear();
        g.applied.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    fn sig(n: u8) -> Signature {
        Signature::from_parts(&[n], b"", b"")
    }

    /// 손으로 풀 수 있는 계산: (future, 결과를 보내는 쪽)
    fn manual() -> (impl Future<Output = Result<Signature, SigError>> + Send + 'static, oneshot::Sender<Result<Signature, SigError>>) {
        let (tx, rx) = oneshot::channel();
        (async move { rx.await.unwrap_or(Err(SigError::Aborted)) }, tx)
    }

    async fn flush() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    // ── 문 ──

    #[tokio::test(start_paused = true)]
    async fn 활성화하지_않은_저장소는_기다리지_않고_활성화하면_첫_페이지가_나간_뒤에_풀린다() {
        let s = RepoState::default();
        s.after_first_page("r").await;
        assert!(!s.is_armed("r"));
        s.arm("r");
        assert!(s.is_armed("r"));
        let t0 = Instant::now();
        let opened = Arc::new(AtomicUsize::new(0));
        let (w, o) = (s.after_first_page("r"), opened.clone());
        let task = tokio::spawn(async move {
            w.await;
            o.fetch_add(1, Ordering::SeqCst);
        });
        flush().await;
        assert_eq!(opened.load(Ordering::SeqCst), 0);
        s.note(NoteOp::FirstPage, "r");
        task.await.unwrap();
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        // 유예가 아니라 알림으로 열렸다(멈춘 시계가 흐르지 않았다)
        assert_eq!(Instant::now(), t0);
        assert!(!s.is_armed("r"));
        // 열린 뒤에는 즉시 통과한다
        s.after_first_page("r").await;
    }

    #[tokio::test(start_paused = true)]
    async fn 첫_페이지가_안_나가도_유예_뒤에_열린다() {
        let s = RepoState::default();
        s.arm("r");
        let w = s.after_first_page("r");
        let t0 = Instant::now();
        w.await;
        assert_eq!(Instant::now() - t0, FIRST_PAGE_GRACE);
        assert!(!s.is_armed("r"));
    }

    #[tokio::test(start_paused = true)]
    async fn 문이_있는_채_다시_arm_해도_유예를_다시_걸지_않고_시그니처만_버린다() {
        let s = RepoState::default();
        s.arm("r");
        s.compute_signature("r", async { Ok(sig(1)) }).await.unwrap();
        assert_eq!(s.known_signature("r"), Some(sig(1)));
        tokio::time::advance(Duration::from_millis(1_000)).await;
        s.arm("r");
        assert_eq!(s.known_signature("r"), None);
        // 마감은 첫 arm 기준 1.5초다(재arm 이 밀지 않는다)
        tokio::time::advance(Duration::from_millis(500)).await;
        assert!(!s.is_armed("r"));
    }

    #[tokio::test(start_paused = true)]
    async fn 열고_다시_arm_한_새_문을_첫_유예가_일찍_열지_못한다() {
        let s = RepoState::default();
        s.arm("r");
        tokio::time::advance(Duration::from_millis(1_000)).await;
        s.open("r");
        s.arm("r"); // 새 문 — 마감은 지금부터 1.5초
        tokio::time::advance(Duration::from_millis(600)).await; // 첫 문의 마감(1.5초)이 지난 시각
        assert!(s.is_armed("r"));
        let t0 = Instant::now();
        s.after_first_page("r").await;
        assert_eq!(Instant::now() - t0, Duration::from_millis(900));
    }

    #[tokio::test(start_paused = true)]
    async fn 저장소마다_문이_따로다() {
        let s = RepoState::default();
        s.arm("a");
        s.arm("b");
        s.note(NoteOp::FirstPage, "a");
        assert!(!s.is_armed("a"));
        assert!(s.is_armed("b"));
        // 문이 없는 저장소의 첫 페이지 알림은 아무 일도 하지 않는다
        s.note(NoteOp::FirstPage, "nope");
    }

    #[tokio::test(start_paused = true)]
    async fn 기다리던_호출은_dispose_로_모두_풀린다() {
        let s = RepoState::default();
        s.arm("a");
        let w = s.after_first_page("a");
        let t0 = Instant::now();
        let task = tokio::spawn(w);
        flush().await;
        assert!(!task.is_finished());
        s.dispose();
        task.await.unwrap();
        assert_eq!(Instant::now(), t0);
        assert!(!s.is_armed("a"));
    }

    // ── 시그니처 ──

    #[tokio::test]
    async fn 성공한_계산은_기억하고_실패한_계산은_기록하지_않는다() {
        let s = RepoState::default();
        assert_eq!(s.compute_signature("r", async { Err(SigError::Failed("x".into())) }).await, Err(SigError::Failed("x".into())));
        assert_eq!(s.known_signature("r"), None);
        assert_eq!(s.compute_signature("r", async { Ok(sig(1)) }).await, Ok(sig(1)));
        assert_eq!(s.known_signature("r"), Some(sig(1)));
        // 저장소마다 따로다
        assert_eq!(s.known_signature("other"), None);
    }

    #[tokio::test]
    async fn 더_나중에_시작한_계산이_먼저_끝나면_먼저_시작한_계산의_낡은_값은_덮지_못한다() {
        let s = RepoState::default();
        let (old, old_tx) = manual();
        let (new, new_tx) = manual();
        let old = tokio::spawn(s.compute_signature("r", old));
        let new = tokio::spawn(s.compute_signature("r", new));
        flush().await;
        new_tx.send(Ok(sig(2))).unwrap();
        assert_eq!(new.await.unwrap(), Ok(sig(2)));
        old_tx.send(Ok(sig(1))).unwrap();
        // 호출자는 자기 값을 받지만 기억은 새 값이다
        assert_eq!(old.await.unwrap(), Ok(sig(1)));
        assert_eq!(s.known_signature("r"), Some(sig(2)));
    }

    #[tokio::test]
    async fn 무효화_전에_시작한_계산은_끝나도_기록하지_못한다() {
        let s = RepoState::default();
        let (c, tx) = manual();
        let task = tokio::spawn(s.compute_signature("r", c));
        flush().await;
        s.invalidate_signature("r");
        tx.send(Ok(sig(1))).unwrap();
        assert_eq!(task.await.unwrap(), Ok(sig(1)));
        assert_eq!(s.known_signature("r"), None);
        // 무효화 뒤에 시작한 계산은 기록한다
        s.compute_signature("r", async { Ok(sig(3)) }).await.unwrap();
        assert_eq!(s.known_signature("r"), Some(sig(3)));
    }

    #[tokio::test]
    async fn 순번은_계산을_부르는_순간_받는다() {
        let s = RepoState::default();
        // 먼저 부른 계산(느림)이 나중에 부른 계산(빠름)보다 늦게 끝나도 나중 값이 남는다 — future 를 폴링하기 전에 순번이 정해진다
        let (slow, slow_tx) = manual();
        let first = s.compute_signature("r", slow);
        let second = s.compute_signature("r", async { Ok(sig(2)) });
        second.await.unwrap();
        slow_tx.send(Ok(sig(1))).unwrap();
        first.await.unwrap();
        assert_eq!(s.known_signature("r"), Some(sig(2)));
    }

    #[tokio::test]
    async fn note_는_이력이_바뀌었거나_fetch_가_ref_를_바꾸면_기억을_버린다() {
        for op in [NoteOp::Stale, NoteOp::FetchChanged] {
            let s = RepoState::default();
            s.compute_signature("r", async { Ok(sig(1)) }).await.unwrap();
            s.compute_signature("o", async { Ok(sig(1)) }).await.unwrap();
            s.note(op, "r");
            assert_eq!(s.known_signature("r"), None, "{op:?}");
            assert_eq!(s.known_signature("o"), Some(sig(1)), "{op:?} — 다른 저장소는 그대로");
        }
        // 첫 페이지 알림은 시그니처를 건드리지 않는다
        let s = RepoState::default();
        s.compute_signature("r", async { Ok(sig(1)) }).await.unwrap();
        s.note(NoteOp::FirstPage, "r");
        assert_eq!(s.known_signature("r"), Some(sig(1)));
    }

    #[tokio::test]
    async fn 공유_계산은_진행_중인_계산에_합류하고_끝나면_다시_시작한다() {
        let s = RepoState::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let (c, tx) = manual();
        let c = Arc::new(Mutex::new(Some(c)));
        let start = |s: &RepoState| {
            let (s, calls, c) = (s.clone(), calls.clone(), c.clone());
            tokio::spawn(async move {
                s.shared_signature(
                    "r",
                    move |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let c = c.lock().unwrap().take();
                        async move {
                            match c {
                                Some(c) => c.await,
                                None => Ok(sig(9)),
                            }
                        }
                    },
                    None,
                )
                .await
            })
        };
        let (a, b) = (start(&s), start(&s));
        flush().await;
        tx.send(Ok(sig(1))).unwrap();
        assert_eq!((a.await.unwrap(), b.await.unwrap()), (Ok(sig(1)), Ok(sig(1))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(s.known_signature("r"), Some(sig(1)));
        // 끝났으니 다음 호출은 새로 계산한다
        assert_eq!(start(&s).await.unwrap(), Ok(sig(9)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn 무효화는_진행_중인_계산에의_합류를_끊고_옛_계산은_기록하지_못한다() {
        let s = RepoState::default();
        let (old, old_tx) = manual();
        let first = {
            let s = s.clone();
            tokio::spawn(async move { s.shared_signature("r", move |_| old, None).await })
        };
        flush().await;
        s.invalidate_signature("r");
        // 합류하지 않고 새로 계산한다
        let fresh = s.shared_signature("r", |_| async { Ok(sig(2)) }, None).await;
        assert_eq!(fresh, Ok(sig(2)));
        old_tx.send(Ok(sig(1))).unwrap();
        assert_eq!(first.await.unwrap(), Ok(sig(1)));
        assert_eq!(s.known_signature("r"), Some(sig(2)));
        // 낡은 계산의 정리가 새 계산의 진행 중 표시를 지우지 않는다: 다시 부르면 (끝났으므로) 새로 계산
        assert_eq!(s.shared_signature("r", |_| async { Ok(sig(4)) }, None).await, Ok(sig(4)));
    }

    #[tokio::test]
    async fn 낡은_계산의_정리는_아직_진행_중인_새_계산의_합류_표시를_지우지_않는다() {
        let s = RepoState::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let (old, old_tx) = manual();
        let first = {
            let s = s.clone();
            tokio::spawn(async move { s.shared_signature("r", move |_| old, None).await })
        };
        flush().await;
        s.invalidate_signature("r");
        // 새 공유 계산 B 를 진행 중으로 둔다
        let (new, new_tx) = manual();
        let second = {
            let (s, calls) = (s.clone(), calls.clone());
            tokio::spawn(async move {
                s.shared_signature(
                    "r",
                    move |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        new
                    },
                    None,
                )
                .await
            })
        };
        flush().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 옛 계산 A 가 끝나 정리된다 — B 는 아직 진행 중
        old_tx.send(Ok(sig(1))).unwrap();
        assert_eq!(first.await.unwrap(), Ok(sig(1)));
        assert!(!second.is_finished());
        // 세 번째 호출은 B 에 합류해야 한다(새로 계산하면 calls 가 는다)
        let third = {
            let (s, calls) = (s.clone(), calls.clone());
            tokio::spawn(async move {
                s.shared_signature(
                    "r",
                    move |_| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        async { Ok(sig(7)) }
                    },
                    None,
                )
                .await
            })
        };
        flush().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "B 에 합류하지 않고 새로 계산했다");
        assert!(!third.is_finished());
        new_tx.send(Ok(sig(2))).unwrap();
        assert_eq!((second.await.unwrap(), third.await.unwrap()), (Ok(sig(2)), Ok(sig(2))));
        assert_eq!(s.known_signature("r"), Some(sig(2)));
    }

    #[tokio::test]
    async fn 기다림을_취소해도_계산은_끝까지_가서_기억한다() {
        let s = RepoState::default();
        let (c, tx) = manual();
        let cancel = CancellationToken::new();
        let waiter = {
            let (s, cancel) = (s.clone(), cancel.clone());
            tokio::spawn(async move { s.shared_signature("r", move |_| c, Some(&cancel)).await })
        };
        flush().await;
        cancel.cancel();
        assert_eq!(waiter.await.unwrap(), Err(SigError::Aborted));
        assert_eq!(s.known_signature("r"), None);
        tx.send(Ok(sig(5))).unwrap();
        flush().await;
        assert_eq!(s.known_signature("r"), Some(sig(5)));
        // 이미 취소된 토큰은 합류만 만들고 즉시 Aborted
        let done = CancellationToken::new();
        done.cancel();
        assert_eq!(s.shared_signature("r", |_| async { Ok(sig(6)) }, Some(&done)).await, Err(SigError::Aborted));
    }

    #[tokio::test(start_paused = true)]
    async fn 공유_계산은_제한_시간이_지나면_계산에_취소를_보낸다() {
        let s = RepoState::default();
        let t0 = Instant::now();
        let r = s
            .shared_signature(
                "r",
                |token| async move {
                    token.cancelled().await;
                    Err(SigError::Aborted)
                },
                None,
            )
            .await;
        assert_eq!(r, Err(SigError::Aborted));
        assert_eq!(Instant::now() - t0, SIGNATURE_TIMEOUT);
        assert_eq!(s.known_signature("r"), None);
        // 실패한 계산은 진행 중 표에서 빠져 다음 호출이 새로 계산한다
        assert_eq!(s.shared_signature("r", |_| async { Ok(sig(1)) }, None).await, Ok(sig(1)));
    }

    #[tokio::test]
    async fn 계산이_패닉해도_진행_중_표시가_남지_않는다() {
        let s = RepoState::default();
        let r = s.shared_signature("r", |_| async { panic!("boom") }, None).await;
        assert!(matches!(r, Err(SigError::Failed(_))));
        assert_eq!(s.shared_signature("r", |_| async { Ok(sig(1)) }, None).await, Ok(sig(1)));
    }

    #[tokio::test]
    async fn 여러_저장소가_동시에_계산해도_서로_섞이지_않는다() {
        let s = RepoState::default();
        let mut tasks = Vec::new();
        for i in 0..16u8 {
            let s = s.clone();
            tasks.push(tokio::spawn(async move {
                let id = format!("r{i}");
                s.shared_signature(&id, move |_| async move { Ok(sig(i)) }, None).await.unwrap();
                (id, i)
            }));
        }
        for t in tasks {
            let (id, i) = t.await.unwrap();
            assert_eq!(s.known_signature(&id), Some(sig(i)));
        }
    }
}
