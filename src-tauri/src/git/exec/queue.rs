//! 동시 실행 수 제한 + 같은 키 요청 합치기(`src/main/executors/ProcessQueue.ts`).
//! 저장소(RepoRuntime)마다 하나씩 두어 git 프로세스가 폭주하지 않게 한다.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::future::{BoxFuture, FutureExt, Shared, WeakShared};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::error::ExecError;

/// 저장소 종류별 동시 git 프로세스 상한(`RepoService.ts` 의 CONCURRENCY 에서 local·wsl 만)
pub const LOCAL_LIMIT: usize = 6;
pub const WSL_LIMIT: usize = 4;

type SharedRun<T> = Shared<BoxFuture<'static, Result<T, ExecError>>>;
type Inflight<T> = Mutex<HashMap<String, (u64, WeakShared<BoxFuture<'static, Result<T, ExecError>>>)>>;

struct Inner<T> {
    sem: Semaphore,
    waiting: AtomicUsize,
    active: AtomicUsize,
    next_id: AtomicU64,
    /// 진행 중인 키 요청. 강한 참조를 두지 않는다 — 기다리는 쪽이 모두 사라지면 future 가 버려져 프로세스가 정리된다
    inflight: Inflight<T>,
}


/// 카운터를 올리고, 버려지면 내린다
struct Gauge<'a>(&'a AtomicUsize);

impl<'a> Gauge<'a> {
    fn up(c: &'a AtomicUsize) -> Self {
        c.fetch_add(1, Ordering::SeqCst);
        Gauge(c)
    }
}

impl Drop for Gauge<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct ProcessQueue<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for ProcessQueue<T> {
    fn clone(&self) -> Self {
        ProcessQueue { inner: self.inner.clone() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueStats {
    pub active: usize,
    pub waiting: usize,
}

impl<T: Clone + Send + Sync + 'static> ProcessQueue<T> {
    pub fn new(limit: usize) -> Self {
        ProcessQueue {
            inner: Arc::new(Inner {
                sem: Semaphore::new(limit),
                waiting: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                next_id: AtomicU64::new(0),
                inflight: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats { active: self.inner.active.load(Ordering::SeqCst), waiting: self.inner.waiting.load(Ordering::SeqCst) }
    }

    /// `task` 를 슬롯이 나는 대로(FIFO) 실행한다.
    ///
    /// `key` 가 같은 요청이 진행 중이면 그 결과를 함께 받는다(`task` 는 실행되지 않는다). **키가 있는 요청에는 취소 신호를
    /// 걸지 않는다** — 한쪽 취소가 다른 쪽을 죽이면 안 되기 때문이다(`cancel` 은 무시된다). 대신 기다리는 쪽이 전부
    /// 사라지면(호출 future 를 모두 버리면) 실행 중인 프로세스도 정리된다.
    /// 키가 없는 요청은 슬롯을 기다리는 동안 `cancel` 로 빠질 수 있고, 이미 실행 중이면 `task` 가 자기 취소 신호를 따른다.
    pub async fn run<F, Fut>(&self, key: Option<&str>, cancel: Option<CancellationToken>, task: F) -> Result<T, ExecError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ExecError>> + Send + 'static,
    {
        let Some(key) = key else {
            return schedule(self.inner.clone(), cancel, task).await;
        };
        let shared = {
            let mut map = self.inner.inflight.lock().unwrap_or_else(PoisonError::into_inner);
            match map.get(key).and_then(|(_, weak)| weak.upgrade()) {
                Some(existing) => existing,
                None => {
                    let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
                    let inner = self.inner.clone();
                    let owned_key = key.to_string();
                    let fut: SharedRun<T> = async move {
                        let r = schedule(inner.clone(), None, task).await;
                        // 끝나면 자기 항목을 지운다(그 사이 같은 키로 새 요청이 들어갔으면 건드리지 않는다)
                        let mut map = inner.inflight.lock().unwrap_or_else(PoisonError::into_inner);
                        if map.get(&owned_key).is_some_and(|(i, _)| *i == id) {
                            map.remove(&owned_key);
                        }
                        r
                    }
                    .boxed()
                    .shared();
                    if let Some(weak) = fut.downgrade() {
                        // 기다리는 쪽이 모두 사라져 버려진 요청은 끝 정리(위 remove)를 못 한다 — 새로 넣을 때 함께 치운다
                        map.retain(|_, (_, w)| w.upgrade().is_some());
                        map.insert(key.to_string(), (id, weak));
                    }
                    fut
                }
            }
        };
        shared.await
    }
}

async fn schedule<T, F, Fut>(inner: Arc<Inner<T>>, cancel: Option<CancellationToken>, task: F) -> Result<T, ExecError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, ExecError>>,
{
    if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return Err(ExecError::Aborted);
    }
    let permit = {
        let _waiting = Gauge::up(&inner.waiting);
        match &cancel {
            Some(t) => tokio::select! {
                biased;
                _ = t.cancelled() => return Err(ExecError::Aborted),
                p = inner.sem.acquire() => p,
            },
            None => inner.sem.acquire().await,
        }
    };
    // 세마포어를 닫지 않으므로 실패하지 않는다
    let _permit = permit.map_err(|e| ExecError::SpawnFailed { message: e.to_string() })?;
    let _active = Gauge::up(&inner.active);
    task().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio::task::yield_now;

    type Q = ProcessQueue<u32>;

    /// 스케줄러가 대기 중인 작업들을 한 바퀴씩 돌게 한다
    async fn settle() {
        for _ in 0..8 {
            yield_now().await;
        }
    }

    fn gated(rx: oneshot::Receiver<u32>) -> impl FnOnce() -> BoxFuture<'static, Result<u32, ExecError>> {
        move || async move { Ok(rx.await.unwrap()) }.boxed()
    }

    #[tokio::test]
    async fn 동시_실행_수를_제한한다() {
        let q = Q::new(2);
        let mut txs = Vec::new();
        let mut hs = Vec::new();
        for _ in 0..3 {
            let (tx, rx) = oneshot::channel();
            txs.push(tx);
            let q = q.clone();
            hs.push(tokio::spawn(async move { q.run(None, None, gated(rx)).await }));
        }
        settle().await;
        assert_eq!(q.stats(), QueueStats { active: 2, waiting: 1 });
        txs.remove(0).send(1).unwrap();
        settle().await;
        assert_eq!(q.stats(), QueueStats { active: 2, waiting: 0 });
        txs.remove(0).send(2).unwrap();
        txs.remove(0).send(3).unwrap();
        let mut got = Vec::new();
        for h in hs {
            got.push(h.await.unwrap().unwrap());
        }
        assert_eq!(got, [1, 2, 3]);
    }

    #[tokio::test]
    async fn 같은_key_요청은_하나로_합친다() {
        let q = Q::new(4);
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = oneshot::channel();
        let c1 = calls.clone();
        let q1 = q.clone();
        let a = tokio::spawn(async move {
            q1.run(Some("k"), None, move || {
                c1.fetch_add(1, Ordering::SeqCst);
                gated(rx)()
            })
            .await
        });
        settle().await;
        let c2 = calls.clone();
        let q2 = q.clone();
        let b = tokio::spawn(async move {
            q2.run(Some("k"), None, move || {
                c2.fetch_add(1, Ordering::SeqCst);
                async { Ok(99) }.boxed()
            })
            .await
        });
        settle().await;
        tx.send(7).unwrap();
        assert_eq!(a.await.unwrap().unwrap(), 7);
        assert_eq!(b.await.unwrap().unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 끝난 키는 지워져 다음 요청은 새로 실행된다
        assert_eq!(q.run(Some("k"), None, || async { Ok(5) }.boxed()).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn 대기_중인_요청을_취소하면_실행하지_않는다() {
        let q = Q::new(1);
        let (tx, rx) = oneshot::channel();
        let q1 = q.clone();
        let first = tokio::spawn(async move { q1.run(None, None, gated(rx)).await });
        settle().await;
        let token = CancellationToken::new();
        let ran = Arc::new(AtomicUsize::new(0));
        let (q2, r2, t2) = (q.clone(), ran.clone(), token.clone());
        let second = tokio::spawn(async move {
            q2.run(None, Some(t2), move || async move {
                r2.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            })
            .await
        });
        settle().await;
        assert_eq!(q.stats().waiting, 1);
        token.cancel();
        assert_eq!(second.await.unwrap(), Err(ExecError::Aborted));
        tx.send(1).unwrap();
        first.await.unwrap().unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!(q.stats(), QueueStats { active: 0, waiting: 0 });
    }

    #[tokio::test]
    async fn 이미_취소된_신호면_바로_aborted() {
        let q = Q::new(1);
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(q.run(None, Some(token), || async { Ok(1) }.boxed()).await, Err(ExecError::Aborted));
    }

    #[tokio::test]
    async fn key_요청에는_취소_신호가_걸리지_않는다() {
        let q = Q::new(1);
        let token = CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        let q1 = q.clone();
        let t1 = token.clone();
        let h = tokio::spawn(async move { q1.run(Some("k"), Some(t1), gated(rx)).await });
        settle().await;
        token.cancel();
        settle().await;
        tx.send(4).unwrap();
        assert_eq!(h.await.unwrap().unwrap(), 4);
    }

    #[tokio::test]
    async fn 기다리는_쪽이_모두_사라지면_슬롯을_돌려준다() {
        let q = Q::new(1);
        let (_tx, rx) = oneshot::channel::<u32>();
        let q1 = q.clone();
        let h = tokio::spawn(async move { q1.run(Some("k"), None, gated(rx)).await });
        settle().await;
        assert_eq!(q.stats().active, 1);
        h.abort();
        let _ = h.await;
        settle().await;
        assert_eq!(q.stats().active, 0);
        // 죽은 항목은 새 요청이 대체한다
        let r = tokio::time::timeout(Duration::from_secs(5), q.run(Some("k"), None, || async { Ok(8) }.boxed())).await;
        assert_eq!(r.unwrap().unwrap(), 8);
    }

    #[tokio::test]
    async fn task_가_실패해도_슬롯을_돌려준다() {
        let q = Q::new(1);
        let r = q.run(None, None, || async { Err(ExecError::Aborted) }.boxed()).await;
        assert_eq!(r, Err(ExecError::Aborted));
        assert_eq!(q.run(None, None, || async { Ok(2) }.boxed()).await.unwrap(), 2);
    }
}
