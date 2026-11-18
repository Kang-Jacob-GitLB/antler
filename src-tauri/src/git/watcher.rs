//! 저장소 감시 상태 기계(`src/main/services/RepoWatcher.ts` 의 이식). Phase 6a PR 7a 가 상태 기계를, PR 7b 가 실제 이벤트원(`fs_watch` — notify)·`WatchHost` 구현·포커스 배선·소유 판정을 더했다.
//!
//! 활성 저장소 하나만 감시하고, ref 시그니처가 **바뀐 경우에만** [`WatchHost::on_change`] 를 부른다(`.git` 안의 잦은 변화 — index·objects — 는 걸러진다):
//! - 로컬·github: `.git`(워크트리면 공통 디렉터리까지)의 변화 → 디바운스([`DEBOUNCE`]) → 시그니처 비교. 안전망 폴링 [`LOCAL_SAFETY_POLL`].
//! - WSL: 9P 는 이벤트를 믿을 수 없어 [`WSL_POLL`] 폴링만. SSH: [`SSH_POLL`] 폴링만. 포커스를 잃으면 폴링을 [`FOCUS_SLOWDOWN`] 배로 늦춘다.
//!
//! 외부와 닿는 곳은 둘 다 주입한다 — 이벤트원은 [`DirWatch`](`fs_watch::NotifyDirs` 가 notify 로 구현), 시계는 `tokio::time` 이라 시험이 `start_paused` 로 멈춘다(fetch 서비스와 같다).
//! git·소유 저장소 정보·알림은 [`WatchHost`] 다.
//!
//! **TS 와 같은 것**: 상수·세대(저장소를 바꾸면 이전 확인의 결과는 버린다)·기준 시그니처는 첫 페이지 문 뒤·확인은 한 번에 하나(진행 중이면 건너뜀)·확인 제한 20초·
//! 기준을 못 잡았으면(`None`) 첫 확인은 알리지 않고 기억만 한다·`objects/` 이벤트 무시·UNC 와 `HEAD` 없는 디렉터리는 감시하지 않는다(악성 `.git` 파일의 DoS 방어, RepoWatcher.ts:88-90)·
//! 디바운스(`kick`: 오래 기다린 확인은 더 미루지 않고, 확인 사이 최소 [`MIN_CHECK_INTERVAL`]).
//!
//! **TS 와 다른 것**
//! - 이벤트를 큐에 쌓지 않는다. 이벤트 콜백(다른 스레드)은 `AtomicBool` 과 `Notify` 로 합칠 뿐이라 폭주해도 메모리가 일정하고, 감시 태스크가 깨어 한 번 `kick` 한다.
//!   같은 순간에 온 이벤트는 TS 에서도 `kick` 이 같은 결과(타이머를 같은 시각으로 다시 걸기)라 관찰되는 동작은 같다.
//! - 저장소마다 태스크 하나가 상태를 혼자 쥔다(세대 카운터 대신 태스크 취소). 저장소를 바꾸면 태스크를 버려 진행 중인 확인의 git 도 취소 신호를 받는다.
//! - [`RepoWatcher::watch`] 는 동기다(기준 계산은 태스크가 한다). tokio 런타임 안에서 불러야 한다(`router::in_runtime_of`).
//! - 기준을 잡는 동안(`baselining`)의 포커스는 TS 처럼 버린다. 기준을 잡은 뒤 디렉터리를 여는 동안의 포커스는 TS 는 바로 확인하지만 여기서는 감시 루프가 시작된 뒤에 처리한다(디렉터리 조회 한 번만큼 늦다).

use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt};
use tokio::sync::Notify;
use tokio::time::{sleep_until, timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::repo_state::SigError;
use super::signature::Signature;

/// WSL 폴링 주기(포커스 중)
pub const WSL_POLL: Duration = Duration::from_millis(4_000);
/// SSH 폴링 주기(포커스 중)
pub const SSH_POLL: Duration = Duration::from_millis(30_000);
/// 창이 포커스를 잃으면 폴링을 이만큼 늦춘다
pub const FOCUS_SLOWDOWN: u32 = 4;
/// 로컬 이벤트를 모아서 한 번만 확인하는 지연
pub const DEBOUNCE: Duration = Duration::from_millis(400);
/// 이벤트가 몰려도 시그니처 확인(git 실행) 사이에 두는 최소 간격
pub const MIN_CHECK_INTERVAL: Duration = Duration::from_millis(2_000);
/// 시그니처 확인 제한 시간(멈춘 wsl.exe·SSH 가 감시를 붙잡지 않게)
pub const CHECK_TIMEOUT: Duration = Duration::from_millis(20_000);
/// 로컬에서도 이벤트를 놓친 변화(네트워크 드라이브 등)를 잡는 안전망 폴링
pub const LOCAL_SAFETY_POLL: Duration = Duration::from_millis(60_000);

/// 저장소 종류. 폴링 주기와 디렉터리 감시 여부를 정한다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchKind {
    Local,
    /// GitHub 원격 전용 저장소 — 앱 캐시의 bare 저장소라 git-dir 이 곧 저장소 경로다(로컬과 같은 방식으로 감시한다)
    Github,
    Wsl,
    Ssh,
}

impl WatchKind {
    /// 포커스 중의 폴링 주기
    pub fn poll_interval(self) -> Duration {
        match self {
            WatchKind::Local | WatchKind::Github => LOCAL_SAFETY_POLL,
            WatchKind::Wsl => WSL_POLL,
            WatchKind::Ssh => SSH_POLL,
        }
    }

    /// `.git` 디렉터리의 변화를 직접 보는가(WSL·SSH 는 폴링만)
    pub fn watches_dirs(self) -> bool {
        matches!(self, WatchKind::Local | WatchKind::Github)
    }
}

/// 감시 디렉터리 안의 변화 하나. `path` 는 **감시한 디렉터리 기준 상대 경로**다(구분자는 어느 쪽이든 된다). 경로를 모르면 `None`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatchEvent {
    pub path: Option<String>,
    /// 이벤트가 넘쳐 일부를 놓쳤다는 표시(rescan). 무엇이 바뀌었는지 모르므로 항상 확인한다
    pub rescan: bool,
}

impl WatchEvent {
    pub fn at(path: &str) -> Self {
        WatchEvent { path: Some(path.to_string()), rescan: false }
    }

    /// 확인을 부를 이벤트인가. 객체 파일은 fetch·gc 때 대량으로 바뀌고 ref 가 함께 바뀌므로 그쪽 이벤트로 충분하다 —
    /// 구분자를 `/` 로 맞춘 경로가 `objects/` 로 시작하면 버린다. 경로를 모르면 확인한다(TS: 문자열이 아니면 이름 `''`)
    pub fn should_kick(&self) -> bool {
        if self.rescan {
            return true;
        }
        match &self.path {
            Some(p) => !p.replace('\\', "/").starts_with("objects/"),
            None => true,
        }
    }
}

/// 이벤트 콜백. **다른 스레드에서 불린다** — 가볍고 막지 않는다(플래그와 `Notify` 만 만진다)
pub type EventSink = Arc<dyn Fn(WatchEvent) + Send + Sync>;
/// 감시 핸들. 버리면 감시가 멈춘다
pub type WatchHandle = Box<dyn Send>;

/// 디렉터리 이벤트원. `fs_watch::NotifyDirs`(notify)가 구현하고 시험은 가짜로 갈아 끼운다.
/// 두 메서드는 future 다 — 파일 시스템을 만지므로(느린 네트워크 드라이브 stat·감시 등록의 확인 대기) 구현이 blocking 풀에서 돌려 tokio 워커를 막지 않는다
pub trait DirWatch: Send + Sync + 'static {
    /// `dir` 이 실제 git 디렉터리인가 — **링크를 따라가지 않고** 디렉터리 자체와 `HEAD` 가 링크가 아닌 실제 디렉터리·파일일 때만 `true`
    /// (악성 저장소가 UNC 심볼릭 링크를 가리키면 stat 만으로도 SMB 접속이 일어난다)
    fn has_head(&self, dir: &str) -> BoxFuture<'static, bool>;
    /// `dir` 을 재귀로 감시한다. 이벤트마다 `sink` 를 부른다. 재귀 감시를 못 하는 파일 시스템이면 `Err` — 안전망 폴링에 맡긴다
    fn watch(&self, dir: &str, sink: EventSink) -> BoxFuture<'static, Result<WatchHandle, String>>;
    /// 앱 종료가 시작됐다 — 이후 핸들을 닫을 때는 OS 핸들·스레드가 끝날 때까지 **동기로** 기다린다(종료 직전에 스레드를 남기지 않는다). 기본은 아무것도 하지 않는다
    fn begin_shutdown(&self) {}
}

/// 안전망·원격 폴링 주기(포커스 중). 기본은 TS 와 같은 상수이고, 시험만 짧게 준다([`RepoWatcher::with_poll`])
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollTimes {
    pub local: Duration,
    pub wsl: Duration,
    pub ssh: Duration,
}

impl Default for PollTimes {
    fn default() -> Self {
        PollTimes { local: LOCAL_SAFETY_POLL, wsl: WSL_POLL, ssh: SSH_POLL }
    }
}

impl PollTimes {
    fn of(&self, kind: WatchKind) -> Duration {
        match kind {
            WatchKind::Local | WatchKind::Github => self.local,
            WatchKind::Wsl => self.wsl,
            WatchKind::Ssh => self.ssh,
        }
    }
}

/// 감시가 기대는 저장소 서비스와 알림 출구
pub trait WatchHost: Send + Sync + 'static {
    /// 저장소 종류. 모르면(미러에서 사라짐) `None` — 디렉터리 감시도 폴링도 하지 않는다
    fn kind(&self, repo_id: &str) -> Option<WatchKind>;
    /// 첫 페이지 문(`RepoState::after_first_page`). 기준 시그니처는 첫 화면이 나온 뒤에 잡는다
    fn after_first_page(&self, repo_id: &str) -> BoxFuture<'static, ()>;
    /// ref 시그니처. 취소 신호가 없는 호출(기준 계산)은 제한 시간이 있는 공유 계산이어야 한다 — 멈춘 원격이어도 기준 계산이 영영 남지 않게
    fn signature(&self, repo_id: &str, cancel: Option<CancellationToken>) -> BoxFuture<'static, Result<Signature, SigError>>;
    /// `git rev-parse --path-format=absolute --git-dir --git-common-dir` 의 디렉터리들. 실패하면 `None`. `cancel` 이 취소되면 git 을 죽인다(감시가 제한 시간을 걸고 취소한다)
    fn local_git_dirs(&self, repo_id: &str, cancel: CancellationToken) -> BoxFuture<'static, Option<Vec<String>>>;
    /// 시그니처가 바뀌었다
    fn on_change(&self, repo_id: &str);
}

/// `/^[\\/]{2}/` — UNC(네트워크) 경로
pub fn is_unc(dir: &str) -> bool {
    let b = dir.as_bytes();
    b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\')
}

/// 감시하지 않을 경로인가 — UNC 에 더해 NT 네임스페이스(`\??\UNC\host\share`)도 네트워크로 이어진다.
/// 어느 쪽이든 `has_head` 의 stat 이 SMB 접속을 일으키므로 **stat 하기 전에** 거른다
pub fn is_remote_path(dir: &str) -> bool {
    is_unc(dir) || crate::git::fetch::guard::has_nt_namespace(dir)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 활성 저장소 하나를 감시한다. **복제되지 않는 소유자**다 — 공유가 필요하면 `Arc` 로 감싼다. 버리면(`dispose` 를 부르지 않아도) 감시가 멈춘다.
///
/// 정지 계약: [`watch`](Self::watch)(`None`·다른 저장소)·[`unwatch_if`](Self::unwatch_if)·`Drop` 이 돌아올 때 이전 저장소의 **맡겨진** 디렉터리 감시 핸들은 **닫기 요청이 끝나 있다**(핸들을 이 스레드에서 바로 버린다 — TS `stop()` 의 `w.close()`). 실제 notify 핸들(`fs_watch`)은 요청만 즉시 하고 notify 스레드의 종료(완료)는 비동기다 —
/// 런타임 안에서는 기다리지 않아(사이드카 리더 스레드·`watch_target` 잠금을 최대 1.5초 붙잡지 않는다) 그 사이 `NotifyDirs::live` 가 잠깐 남는다. 앱 종료([`dispose`](Self::dispose))만 끝날 때까지 기다린다.
/// 디렉터리를 **여는 중**(blocking 풀에서 등록하는 동안)에 정지하면 그 핸들은 곧 버려진다 — 호출처가 바로 폴더를 지워도 되는 보장으로 쓰지 않는다(github 캐시 삭제 — `config_host` 의 `repos:remove` — 는 재시도하는 `remove_github_cache` 로 지운다. notify 는 `FILE_SHARE_DELETE` 로 열어 닫히는 중에도 지워진다). 또 멀티스레드 런타임에서는
/// 정지 직전에 끝난 확인의 `on_change` 가 드물게 정지 뒤에 한 번 늦게 올 수 있다 — 받는 쪽(renderer)이 activeRepoId 로 거른다
pub struct RepoWatcher {
    inner: Arc<Inner>,
}

struct Inner {
    host: Arc<dyn WatchHost>,
    dirs: Arc<dyn DirWatch>,
    polls: PollTimes,
    /// 창 포커스. 저장소를 바꿔도 유지된다(TS `focused`)
    focused: AtomicBool,
    core: Mutex<Core>,
}

#[derive(Default)]
struct Core {
    current: Option<String>,
    run: Option<Run>,
}

struct Run {
    cancel: CancellationToken,
    ctl: Arc<Ctl>,
}

impl Run {
    /// 태스크에 취소를 보내고 디렉터리 감시 핸들을 **이 스레드에서 바로** 닫는다.
    /// 취소를 먼저 하므로 태스크가 핸들을 맡기는 순간(`Ctl::adopt`)에는 취소를 보거나, 아니면 여기서 닫힌다 — 맡겨진 핸들이 새지 않는다
    /// (등록 중이라 아직 맡겨지지 않은 핸들은 등록이 끝나는 대로 버려진다)
    fn stop(self) {
        self.cancel.cancel();
        let handles = std::mem::take(&mut *lock(&self.ctl.handles));
        drop(handles);
    }
}

/// 감시 태스크와 바깥(포커스)이 나누는 것
struct Ctl {
    /// 열린 디렉터리 감시 핸들. 태스크가 아니라 여기 둬서 정지가 동기로 닫을 수 있다
    handles: Mutex<Vec<WatchHandle>>,
    /// 기준 시그니처를 계산하는 중 — 이때 포커스로 확인을 또 돌리면 같은 for-each-ref 를 한 번 더 실행한다
    baselining: AtomicBool,
    /// 포커스를 얻어 바로 확인하라는 요청
    check_now: Notify,
}

impl Ctl {
    /// 새 핸들을 맡는다. 이미 취소됐으면(정지가 핸들 비우기를 끝냈을 수 있다) 바로 닫는다
    fn adopt(&self, handle: WatchHandle, cancel: &CancellationToken) {
        let mut handles = lock(&self.handles);
        if cancel.is_cancelled() {
            drop(handles);
            drop(handle);
        } else {
            handles.push(handle);
        }
    }
}

/// 이벤트 콜백이 감시 태스크를 깨우는 길. 이벤트를 쌓지 않고 합친다
#[derive(Default)]
struct Events {
    dirty: AtomicBool,
    wake: Notify,
}

/// 진행 중인 확인 하나(TS `checkingGen`). 결과는 `None` = 실패·제한 시간
struct Check {
    fut: BoxFuture<'static, Option<Signature>>,
    /// 안전망 폴링이 시작한 확인 — 끝나면 폴링을 다시 건다
    from_poll: bool,
}

/// 디바운스·최소 간격의 시각 기록(TS `debounce`·`pendingSince`·`lastCheck`)
struct Timing {
    debounce_at: Option<Instant>,
    pending_since: Instant,
    last_check: Option<Instant>,
}

impl Timing {
    /// TS `kick`. 이벤트가 쉬지 않고 와도 확인은 [`MIN_CHECK_INTERVAL`] 에 한 번을 넘지 않는다(디바운스만으로는 계속 밀린다)
    fn kick(&mut self, now: Instant) {
        if self.debounce_at.is_some() {
            // 이미 오래 기다린 확인은 더 미루지 않는다 — 계속 미루면 이벤트가 끊이지 않는 동안 영영 확인하지 못한다
            if now.saturating_duration_since(self.pending_since) > MIN_CHECK_INTERVAL {
                return;
            }
        } else {
            self.pending_since = now;
        }
        let since_gap = self.last_check.map_or(Duration::ZERO, |c| (c + MIN_CHECK_INTERVAL).saturating_duration_since(now));
        self.debounce_at = Some(now + DEBOUNCE.max(since_gap));
    }
}

impl RepoWatcher {
    pub fn new(host: Arc<dyn WatchHost>, dirs: Arc<dyn DirWatch>) -> Self {
        Self::with_poll(host, dirs, PollTimes::default())
    }

    /// 폴링 주기를 정해 만든다(시험이 실제 소켓 위에서 짧은 주기로 폴링을 본다 — 멈춘 시계는 실제 I/O 대기 중에 자동으로 전진해 제한 시간을 헛발화시킨다)
    pub fn with_poll(host: Arc<dyn WatchHost>, dirs: Arc<dyn DirWatch>, polls: PollTimes) -> Self {
        RepoWatcher { inner: Arc::new(Inner { host, dirs, polls, focused: AtomicBool::new(true), core: Mutex::default() }) }
    }

    /// 지금 감시 중인 저장소
    pub fn current(&self) -> Option<String> {
        lock(&self.inner.core).current.clone()
    }

    /// 감시할 저장소를 바꾼다(`None` 이면 멈춘다). 같은 저장소면 아무것도 하지 않는다. 이전 저장소의 확인은 버려지고 그 결과는 알리지 않는다.
    /// 기준 시그니처는 첫 페이지 문 뒤에 태스크가 잡는다. tokio 런타임 안에서 불러야 한다
    pub fn watch(&self, repo_id: Option<&str>) {
        let mut core = lock(&self.inner.core);
        if core.current.as_deref() == repo_id {
            return;
        }
        let old = core.run.take();
        core.current = repo_id.map(str::to_string);
        let started = repo_id.map(|id| {
            let cancel = CancellationToken::new();
            let ctl = Arc::new(Ctl { handles: Mutex::default(), baselining: AtomicBool::new(true), check_now: Notify::new() });
            core.run = Some(Run { cancel: cancel.clone(), ctl: ctl.clone() });
            (id.to_string(), cancel, ctl)
        });
        drop(core);
        // 핸들 닫기는 뮤텍스 밖에서, 하지만 돌아가기 전에 끝낸다
        if let Some(run) = old {
            run.stop();
        }
        if let Some((id, cancel, ctl)) = started {
            let inner = self.inner.clone();
            tokio::spawn(async move {
                // 취소가 먼저다(`biased`) — 취소하면 진행 중인 확인도 버려진다
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {}
                    _ = inner.run(id, ctl, cancel.clone()) => {}
                }
            });
        }
    }

    /// 지금 감시 중인 저장소가 `repo_id` 일 때만 멈춘다(한 잠금 안에서 비교하고 멈춘다 — 저장소 제거가 그 사이 다른 저장소로 바뀐 감시를 끊지 않게).
    /// 멈췄으면 `true`. 돌아올 때 디렉터리 감시 핸들은 닫혀 있다
    pub fn unwatch_if(&self, repo_id: &str) -> bool {
        let mut core = lock(&self.inner.core);
        if core.current.as_deref() != Some(repo_id) {
            return false;
        }
        core.current = None;
        let run = core.run.take();
        drop(core);
        if let Some(run) = run {
            run.stop();
        }
        true
    }

    /// 창 포커스 변화. 다시 포커스를 얻으면 바로 한 번 확인한다(다른 도구에서 커밋하고 돌아온 경우). 감시 중이 아니거나 기준을 잡는 중이면 확인하지 않는다.
    /// 어느 스레드에서 불러도 된다
    pub fn set_focused(&self, focused: bool) {
        self.inner.focused.store(focused, Ordering::SeqCst);
        if !focused {
            return;
        }
        if let Some(run) = &lock(&self.inner.core).run {
            if !run.ctl.baselining.load(Ordering::SeqCst) {
                run.ctl.check_now.notify_one();
            }
        }
    }

    /// 앱 종료. 디렉터리 핸들을 **끝날 때까지 기다려** 닫는다([`DirWatch::begin_shutdown`]) — 평소의 정지는 닫기를 요청만 하고 기다리지 않는다
    pub fn dispose(&self) {
        self.inner.dirs.begin_shutdown();
        self.watch(None);
    }
}

impl Drop for RepoWatcher {
    /// 태스크가 쥔 `Inner` 의 수명에 기대지 않고 소유자가 사라지면 멈춘다
    fn drop(&mut self) {
        self.watch(None);
    }
}

/// 마감이 있으면 그때까지, 없으면 영영 기다린다
async fn deadline(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => pending().await,
    }
}

impl Inner {
    /// 포커스를 잃으면 [`FOCUS_SLOWDOWN`] 배(TS `schedule` — 거는 시점의 포커스로 정한다)
    fn next_poll(&self, kind: Option<WatchKind>) -> Option<Instant> {
        let kind = kind?;
        let slow = if self.focused.load(Ordering::SeqCst) { 1 } else { FOCUS_SLOWDOWN };
        Some(Instant::now() + self.polls.of(kind) * slow)
    }

    fn begin_check(&self, repo_id: &str, cancel: &CancellationToken, from_poll: bool) -> Check {
        let token = cancel.child_token();
        let sig = self.host.signature(repo_id, Some(token.clone()));
        let fut = async move {
            match timeout(CHECK_TIMEOUT, sig).await {
                Ok(r) => r.ok(),
                Err(_) => {
                    // 제한 시간이 지나면 git 에 취소를 보낸다
                    token.cancel();
                    None
                }
            }
        };
        Check { fut: fut.boxed(), from_poll }
    }

    /// `.git` 디렉터리들에 감시를 건다. 악성 저장소의 `.git` 파일이 gitdir/commondir 로 홈 전체나 네트워크 경로를 가리키면
    /// 재귀 감시가 이벤트 폭주(DoS)가 되므로, 실제 git 디렉터리(`HEAD` 가 있는 곳)이고 네트워크 경로가 아닐 때만 건다
    async fn watch_dirs(&self, repo_id: &str, events: &Arc<Events>, ctl: &Ctl, cancel: &CancellationToken) {
        // 멈춘 git 이 감시 시작을 붙잡지 않게 제한 시간을 걸고, 넘기면 git 에 취소를 보낸다(확인과 같다)
        let token = cancel.child_token();
        let Ok(Some(dirs)) = timeout(CHECK_TIMEOUT, self.host.local_git_dirs(repo_id, token.clone())).await else {
            token.cancel();
            return;
        };
        for dir in dirs {
            if is_remote_path(&dir) || !self.dirs.has_head(&dir).await {
                continue;
            }
            let events = events.clone();
            let sink: EventSink = Arc::new(move |e: WatchEvent| {
                if e.should_kick() {
                    events.dirty.store(true, Ordering::Release);
                    events.wake.notify_one();
                }
            });
            // 재귀 감시를 지원하지 않는 파일 시스템 — 안전망 폴링에 맡긴다
            if let Ok(h) = self.dirs.watch(&dir, sink).await {
                ctl.adopt(h, cancel);
            }
        }
    }

    /// 한 저장소의 감시 태스크 본체. 취소되면(저장소가 바뀌거나 멈추면) 어디서든 버려진다
    async fn run(self: Arc<Self>, repo_id: String, ctl: Arc<Ctl>, cancel: CancellationToken) {
        // 기준 시그니처를 먼저 잡아 둔다(감시 시작 전 상태와 비교). 첫 화면이 나온 뒤에 시작한다
        self.host.after_first_page(&repo_id).await;
        let mut last_sig = self.host.signature(&repo_id, None).await.ok();
        ctl.baselining.store(false, Ordering::SeqCst);

        let kind = self.host.kind(&repo_id);
        let events = Arc::new(Events::default());
        // 핸들은 `ctl` 이 쥔다 — 정지가 동기로 닫는다
        if kind.is_some_and(WatchKind::watches_dirs) {
            self.watch_dirs(&repo_id, &events, &ctl, &cancel).await;
        }

        let mut timing = Timing { debounce_at: None, pending_since: Instant::now(), last_check: None };
        let mut poll_at = self.next_poll(kind);
        let mut checking: Option<Check> = None;
        loop {
            tokio::select! {
                () = events.wake.notified() => {
                    if events.dirty.swap(false, Ordering::AcqRel) {
                        timing.kick(Instant::now());
                    }
                }
                () = deadline(timing.debounce_at) => {
                    timing.debounce_at = None;
                    self.start(&repo_id, &cancel, &mut timing, &mut checking, false);
                }
                () = ctl.check_now.notified() => {
                    self.start(&repo_id, &cancel, &mut timing, &mut checking, false);
                }
                () = deadline(poll_at) => {
                    poll_at = None;
                    if checking.is_some() {
                        // 이미 확인 중이면 건너뛰고 다음 폴링을 바로 건다(TS `check` 가 돌아오고 `finally` 가 다시 건다)
                        poll_at = self.next_poll(kind);
                    } else {
                        self.start(&repo_id, &cancel, &mut timing, &mut checking, true);
                    }
                }
                sig = async {
                    match checking.as_mut() {
                        Some(c) => c.fut.as_mut().await,
                        None => pending().await,
                    }
                } => {
                    // 실패(SSH 끊김·시간 초과·잠금 경합)는 다음 주기에 다시 본다
                    if let Some(sig) = sig {
                        if !cancel.is_cancelled() && last_sig.is_some_and(|l| l != sig) {
                            self.host.on_change(&repo_id);
                        }
                        last_sig = Some(sig);
                    }
                    if checking.take().is_some_and(|c| c.from_poll) {
                        poll_at = self.next_poll(kind);
                    }
                }
            }
        }
    }

    /// 확인을 시작한다. 이미 하나가 돌고 있으면 건너뛴다(TS `checkingGen`)
    fn start(&self, repo_id: &str, cancel: &CancellationToken, timing: &mut Timing, checking: &mut Option<Check>, from_poll: bool) {
        if checking.is_some() {
            return;
        }
        timing.last_check = Some(Instant::now());
        *checking = Some(self.begin_check(repo_id, cancel, from_poll));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::time::sleep;

    // ── 가짜 이벤트원·호스트 ─────────────────────────────────────────────

    /// 핸들을 버리면 열린 감시 수가 줄어든다
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct FakeDirs {
        no_head: Mutex<Vec<String>>,
        sinks: Mutex<Vec<(String, EventSink)>>,
        open: Arc<AtomicUsize>,
    }

    impl DirWatch for FakeDirs {
        fn has_head(&self, dir: &str) -> BoxFuture<'static, bool> {
            let has = !lock(&self.no_head).iter().any(|d| d == dir);
            async move { has }.boxed()
        }
        fn watch(&self, dir: &str, sink: EventSink) -> BoxFuture<'static, Result<WatchHandle, String>> {
            lock(&self.sinks).push((dir.to_string(), sink));
            self.open.fetch_add(1, Ordering::SeqCst);
            let handle: WatchHandle = Box::new(Guard(self.open.clone()));
            async move { Ok(handle) }.boxed()
        }
    }

    impl FakeDirs {
        fn emit(&self, path: &str) {
            for (_, s) in lock(&self.sinks).iter() {
                s(WatchEvent::at(path));
            }
        }
        fn watched(&self) -> Vec<String> {
            lock(&self.sinks).iter().map(|(d, _)| d.clone()).collect()
        }
        fn open(&self) -> usize {
            self.open.load(Ordering::SeqCst)
        }
    }

    struct FakeHost {
        t0: Instant,
        kind: Mutex<Option<WatchKind>>,
        sig: Mutex<u8>,
        /// 취소 신호가 있는 호출(확인)의 지연
        check_delay: Mutex<Duration>,
        hang_checks: AtomicBool,
        fail_checks: AtomicBool,
        fail_baseline: AtomicBool,
        gate: Mutex<Option<Arc<Notify>>>,
        git_dirs: Mutex<Option<Vec<String>>>,
        /// `local_git_dirs` 가 끝나지 않는다(멈춘 git)
        hang_dirs: AtomicBool,
        /// `local_git_dirs` 가 받은 취소 신호
        dir_tokens: Mutex<Vec<CancellationToken>>,
        /// (t0 기준 시각, 취소 신호가 있는가)
        calls: Mutex<Vec<(Duration, bool)>>,
        tokens: Mutex<Vec<CancellationToken>>,
        changes: Mutex<Vec<(String, Duration)>>,
    }

    impl FakeHost {
        fn since(&self) -> Duration {
            Instant::now() - self.t0
        }
        fn checks(&self) -> Vec<Duration> {
            lock(&self.calls).iter().filter(|c| c.1).map(|c| c.0).collect()
        }
        fn baselines(&self) -> usize {
            lock(&self.calls).iter().filter(|c| !c.1).count()
        }
        fn changes(&self) -> Vec<String> {
            lock(&self.changes).iter().map(|c| c.0.clone()).collect()
        }
        fn set_sig(&self, n: u8) {
            *lock(&self.sig) = n;
        }
    }

    impl WatchHost for FakeHost {
        fn kind(&self, _: &str) -> Option<WatchKind> {
            *lock(&self.kind)
        }
        fn after_first_page(&self, _: &str) -> BoxFuture<'static, ()> {
            let gate = lock(&self.gate).clone();
            async move {
                if let Some(g) = gate {
                    g.notified().await;
                }
            }
            .boxed()
        }
        fn signature(&self, _: &str, cancel: Option<CancellationToken>) -> BoxFuture<'static, Result<Signature, SigError>> {
            lock(&self.calls).push((self.since(), cancel.is_some()));
            let value = Signature::for_test(*lock(&self.sig));
            let Some(token) = cancel else {
                let fail = self.fail_baseline.load(Ordering::SeqCst);
                return async move { if fail { Err(SigError::Git("baseline".into())) } else { Ok(value) } }.boxed();
            };
            lock(&self.tokens).push(token);
            let delay = *lock(&self.check_delay);
            let (hang, fail) = (self.hang_checks.load(Ordering::SeqCst), self.fail_checks.load(Ordering::SeqCst));
            async move {
                if hang {
                    pending::<()>().await;
                }
                sleep(delay).await;
                if fail {
                    Err(SigError::Git("check".into()))
                } else {
                    Ok(value)
                }
            }
            .boxed()
        }
        fn local_git_dirs(&self, _: &str, cancel: CancellationToken) -> BoxFuture<'static, Option<Vec<String>>> {
            lock(&self.dir_tokens).push(cancel);
            let dirs = lock(&self.git_dirs).clone();
            let hang = self.hang_dirs.load(Ordering::SeqCst);
            async move {
                if hang {
                    pending::<()>().await;
                }
                dirs
            }
            .boxed()
        }
        fn on_change(&self, repo_id: &str) {
            lock(&self.changes).push((repo_id.to_string(), self.since()));
        }
    }

    struct Fx {
        host: Arc<FakeHost>,
        dirs: Arc<FakeDirs>,
        w: RepoWatcher,
        t0: Instant,
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    impl Fx {
        fn new(kind: Option<WatchKind>) -> Fx {
            let t0 = Instant::now();
            let host = Arc::new(FakeHost {
                t0,
                kind: Mutex::new(kind),
                sig: Mutex::new(1),
                check_delay: Mutex::new(Duration::ZERO),
                hang_checks: AtomicBool::new(false),
                fail_checks: AtomicBool::new(false),
                fail_baseline: AtomicBool::new(false),
                gate: Mutex::new(None),
                git_dirs: Mutex::new(Some(vec!["/r/.git".to_string()])),
                hang_dirs: AtomicBool::new(false),
                dir_tokens: Mutex::default(),
                calls: Mutex::default(),
                tokens: Mutex::default(),
                changes: Mutex::default(),
            });
            let dirs = Arc::new(FakeDirs::default());
            let w = RepoWatcher::new(host.clone(), dirs.clone());
            Fx { host, dirs, w, t0 }
        }

        /// 로컬 저장소를 감시하기 시작하고 기준 계산이 끝나게 둔다
        async fn started(kind: WatchKind) -> Fx {
            let fx = Fx::new(Some(kind));
            fx.w.watch(Some("r"));
            settle().await;
            fx
        }

        async fn at(&self, millis: u64) {
            tokio::time::sleep_until(self.t0 + ms(millis)).await;
        }
    }

    /// 시간을 흘리지 않고 밀린 태스크가 돌게 한다
    async fn settle() {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
    }

    // ── 순수 로직 ───────────────────────────────────────────────────────

    #[test]
    fn 상수와_종류별_주기는_ts_와_같다() {
        assert_eq!((WSL_POLL, SSH_POLL, LOCAL_SAFETY_POLL), (ms(4_000), ms(30_000), ms(60_000)));
        assert_eq!((DEBOUNCE, MIN_CHECK_INTERVAL, CHECK_TIMEOUT, FOCUS_SLOWDOWN), (ms(400), ms(2_000), ms(20_000), 4));
        assert_eq!(WatchKind::Local.poll_interval(), LOCAL_SAFETY_POLL);
        assert_eq!(WatchKind::Github.poll_interval(), LOCAL_SAFETY_POLL);
        assert!(WatchKind::Local.watches_dirs() && WatchKind::Github.watches_dirs());
        assert!(!WatchKind::Wsl.watches_dirs() && !WatchKind::Ssh.watches_dirs());
    }

    #[test]
    fn objects_이벤트만_버리고_경로를_모르거나_rescan_이면_확인한다() {
        for (p, kick) in [
            ("objects/ab/cdef", false),
            ("objects\\pack\\x.idx", false),
            ("objects/", false),
            ("objects", true),
            ("refs/heads/main", true),
            ("HEAD", true),
            ("config", true),
            ("index", true),
            ("worktrees/x/objects/y", true),
            ("my-objects/x", true),
            ("Objects/x", true),
        ] {
            assert_eq!(WatchEvent::at(p).should_kick(), kick, "{p}");
        }
        assert!(WatchEvent::default().should_kick());
        assert!(WatchEvent { path: Some("objects/x".into()), rescan: true }.should_kick());
    }

    #[test]
    fn unc_경로_판정은_구분자_둘로_시작하는_것이다() {
        for (d, unc) in [("\\\\server\\share", true), ("//host/x", true), ("\\/x", true), ("/\\x", true), ("C:\\repo", false), ("/home/x", false), ("/", false), ("", false), ("\\", false)] {
            assert_eq!(is_unc(d), unc, "{d:?}");
        }
    }

    // ── 디바운스·최소 간격 ──────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn 이벤트는_디바운스_뒤_한_번_확인하고_시그니처가_바뀌었을_때만_알린다() {
        let fx = Fx::started(WatchKind::Local).await;
        assert_eq!(fx.host.baselines(), 1);
        fx.dirs.emit("refs/heads/main");
        fx.host.set_sig(2);
        fx.at(399).await;
        assert!(fx.host.checks().is_empty(), "디바운스 안에서는 확인하지 않는다");
        fx.at(401).await;
        assert_eq!(fx.host.checks(), vec![ms(400)]);
        assert_eq!(fx.host.changes(), vec!["r"]);

        // ref 가 바뀌지 않는 변화(설정 파일 수정)는 확인은 하지만 알리지 않는다
        fx.dirs.emit("config");
        fx.at(3_000).await;
        assert_eq!(fx.host.checks().len(), 2);
        assert_eq!(fx.host.changes().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn objects_이벤트는_확인을_부르지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        fx.dirs.emit("objects/ab/cdef");
        fx.dirs.emit("objects\\pack\\pack-1.pack");
        fx.at(1_000).await;
        assert!(fx.host.checks().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn 확인_사이에는_최소_간격을_둔다() {
        let fx = Fx::started(WatchKind::Local).await;
        fx.dirs.emit("HEAD");
        fx.at(401).await;
        assert_eq!(fx.host.checks(), vec![ms(400)]);
        // 마지막 확인(400) + 2000 = 2400 이전에는 두 번째 확인이 나가지 않는다
        fx.at(500).await;
        fx.dirs.emit("HEAD");
        fx.at(2_399).await;
        assert_eq!(fx.host.checks().len(), 1);
        fx.at(2_401).await;
        assert_eq!(fx.host.checks(), vec![ms(400), ms(2_400)]);
    }

    #[tokio::test(start_paused = true)]
    async fn 이벤트가_끊이지_않아도_오래_기다린_확인은_더_미루지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        for i in 0..=10 {
            fx.at(i * 300).await;
            fx.dirs.emit("refs/heads/main");
        }
        fx.at(3_500).await;
        // 300ms 마다 디바운스가 밀리다가 첫 이벤트(0)로부터 2000ms 가 지난 뒤의 이벤트(2100)는 밀지 않는다 → 1800 의 마감(2200)에 확인
        assert_eq!(fx.host.checks(), vec![ms(2_200)]);
    }

    #[tokio::test(start_paused = true)]
    async fn 이벤트_십만_개가_몰려도_확인은_한_번이다() {
        let fx = Fx::started(WatchKind::Local).await;
        for _ in 0..100_000 {
            fx.dirs.emit("refs/heads/main");
        }
        fx.at(401).await;
        assert_eq!(fx.host.checks(), vec![ms(400)]);
        fx.at(2_300).await;
        assert_eq!(fx.host.checks().len(), 1, "이미 처리한 이벤트는 다시 확인을 부르지 않는다");
    }

    // ── 폴링·포커스 ─────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn wsl_은_디렉터리를_감시하지_않고_4초마다_폴링하며_포커스를_잃으면_4배로_늦춘다() {
        let fx = Fx::started(WatchKind::Wsl).await;
        assert!(fx.dirs.watched().is_empty());
        fx.at(3_999).await;
        assert!(fx.host.checks().is_empty());
        fx.at(4_001).await;
        assert_eq!(fx.host.checks(), vec![ms(4_000)]);
        // 이미 걸린 타이머(8000)는 포커스 변화와 무관하고, 그 다음부터 늦춰진다(TS: 거는 시점의 포커스)
        fx.w.set_focused(false);
        fx.at(8_001).await;
        assert_eq!(fx.host.checks(), vec![ms(4_000), ms(8_000)]);
        fx.at(23_999).await;
        assert_eq!(fx.host.checks().len(), 2);
        fx.at(24_001).await;
        assert_eq!(fx.host.checks().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn ssh_는_30초_폴링이다() {
        let fx = Fx::started(WatchKind::Ssh).await;
        assert!(fx.dirs.watched().is_empty());
        fx.at(29_999).await;
        assert!(fx.host.checks().is_empty());
        fx.at(30_001).await;
        assert_eq!(fx.host.checks(), vec![ms(30_000)]);
        fx.w.set_focused(false);
        fx.at(60_001).await;
        assert_eq!(fx.host.checks().len(), 2);
        // 포커스를 잃은 뒤 건 타이머는 120초
        fx.at(179_999).await;
        assert_eq!(fx.host.checks().len(), 2);
        fx.at(180_001).await;
        assert_eq!(fx.host.checks().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn 로컬과_github_는_디렉터리를_감시하고_60초_안전망_폴링을_한다() {
        for kind in [WatchKind::Local, WatchKind::Github] {
            let fx = Fx::started(kind).await;
            assert_eq!(fx.dirs.watched(), vec!["/r/.git"]);
            fx.at(59_999).await;
            assert!(fx.host.checks().is_empty());
            fx.at(60_001).await;
            assert_eq!(fx.host.checks(), vec![ms(60_000)]);
            // 이벤트 확인은 폴링 타이머를 다시 걸지 않는다 — 다음 폴링은 120000
            fx.dirs.emit("HEAD");
            fx.at(61_900).await;
            assert_eq!(fx.host.checks().len(), 1, "방금 폴링으로 확인했으니 최소 간격(2초)을 둔다");
            fx.at(62_100).await;
            assert_eq!(fx.host.checks().len(), 2);
            fx.at(119_999).await;
            assert_eq!(fx.host.checks().len(), 2);
            fx.at(120_001).await;
            assert_eq!(fx.host.checks().len(), 3);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn 포커스를_얻으면_바로_확인하고_잃는_것은_확인하지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        fx.w.set_focused(false);
        settle().await;
        assert!(fx.host.checks().is_empty());
        fx.w.set_focused(true);
        settle().await;
        assert_eq!(fx.host.checks(), vec![ms(0)]);
        fx.host.set_sig(2);
        fx.w.set_focused(false);
        fx.w.set_focused(true);
        settle().await;
        assert_eq!(fx.host.checks().len(), 2);
        assert_eq!(fx.host.changes(), vec!["r"], "다른 도구에서 커밋하고 돌아온 경우 알린다");
    }

    #[tokio::test(start_paused = true)]
    async fn 확인이_진행_중이면_포커스·이벤트가_또_확인하지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        // 확인(0~3000)이 디바운스 마감(400)과 최소 간격(2000)을 넘겨 계속된다
        *lock(&fx.host.check_delay) = ms(3_000);
        fx.w.set_focused(true);
        settle().await;
        assert_eq!(fx.host.checks(), vec![ms(0)]);
        fx.w.set_focused(true);
        fx.dirs.emit("HEAD");
        fx.at(401).await;
        assert_eq!(fx.host.checks().len(), 1, "디바운스 마감(400)에 진행 중이라 건너뛴다");
        fx.at(2_900).await;
        assert_eq!(fx.host.checks().len(), 1, "포커스·이벤트 모두 진행 중에는 새 확인을 부르지 않는다");
        fx.at(10_000).await;
        assert_eq!(fx.host.checks().len(), 1, "건너뛴 디바운스는 확인이 끝난 뒤 다시 부르지 않는다");
    }

    #[tokio::test(start_paused = true)]
    async fn 확인_중에_폴링_시각이_오면_건너뛰고_다음_폴링을_다시_건다() {
        let fx = Fx::started(WatchKind::Wsl).await;
        // 포커스 확인(0~5000)이 첫 폴링(4000)을 지나서까지 계속된다
        *lock(&fx.host.check_delay) = ms(5_000);
        fx.w.set_focused(true);
        settle().await;
        fx.at(4_001).await;
        assert_eq!(fx.host.checks(), vec![ms(0)], "진행 중이라 폴링은 확인을 시작하지 않는다");
        fx.at(7_999).await;
        assert_eq!(fx.host.checks().len(), 1);
        // 재무장한 폴링(4000 + 4000)이 다음 확인을 시작한다 — 재무장이 없으면 폴링이 영영 멎는다
        fx.at(8_001).await;
        assert_eq!(fx.host.checks(), vec![ms(0), ms(8_000)]);
    }

    #[tokio::test(start_paused = true)]
    async fn 기준을_잡는_동안의_포커스는_확인하지_않는다() {
        let fx = Fx::new(Some(WatchKind::Local));
        let gate = Arc::new(Notify::new());
        *lock(&fx.host.gate) = Some(gate.clone());
        fx.w.watch(Some("r"));
        settle().await;
        fx.w.set_focused(true);
        settle().await;
        assert_eq!(fx.host.baselines(), 0, "첫 페이지 문이 열리기 전에는 기준도 잡지 않는다");
        gate.notify_one();
        settle().await;
        assert_eq!(fx.host.baselines(), 1);
        assert!(fx.host.checks().is_empty(), "기준 중에 받은 포커스는 버려진다");
    }

    #[tokio::test(start_paused = true)]
    async fn 첫_페이지_문이_열리기_전에는_폴링도_걸지_않는다() {
        let fx = Fx::new(Some(WatchKind::Wsl));
        let gate = Arc::new(Notify::new());
        *lock(&fx.host.gate) = Some(gate.clone());
        fx.w.watch(Some("r"));
        fx.at(60_000).await;
        assert!(lock(&fx.host.calls).is_empty());
        gate.notify_one();
        settle().await;
        assert_eq!(fx.host.baselines(), 1);
        // 폴링은 기준을 잡은 때부터 센다
        fx.at(63_999).await;
        assert!(fx.host.checks().is_empty());
        fx.at(64_001).await;
        assert_eq!(fx.host.checks().len(), 1);
    }

    // ── 시그니처 비교 ───────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn 기준을_못_잡았으면_첫_확인은_알리지_않고_기억만_한다() {
        let fx = Fx::new(Some(WatchKind::Local));
        fx.host.fail_baseline.store(true, Ordering::SeqCst);
        fx.w.watch(Some("r"));
        settle().await;
        fx.host.set_sig(2);
        fx.dirs.emit("HEAD");
        fx.at(401).await;
        assert!(fx.host.changes().is_empty());
        fx.host.set_sig(3);
        fx.dirs.emit("HEAD");
        fx.at(3_000).await;
        assert_eq!(fx.host.changes(), vec!["r"]);
    }

    #[tokio::test(start_paused = true)]
    async fn 확인이_실패하면_알리지_않고_다음_주기에_다시_본다() {
        let fx = Fx::started(WatchKind::Wsl).await;
        fx.host.fail_checks.store(true, Ordering::SeqCst);
        fx.host.set_sig(2);
        fx.at(4_001).await;
        assert!(fx.host.changes().is_empty());
        fx.host.fail_checks.store(false, Ordering::SeqCst);
        fx.at(8_001).await;
        assert_eq!(fx.host.checks().len(), 2);
        assert_eq!(fx.host.changes(), vec!["r"]);
    }

    #[tokio::test(start_paused = true)]
    async fn 확인은_20초가_지나면_취소되고_그_뒤_확인이_다시_나간다() {
        let fx = Fx::started(WatchKind::Local).await;
        fx.host.hang_checks.store(true, Ordering::SeqCst);
        fx.dirs.emit("HEAD");
        fx.at(20_399).await;
        assert_eq!(fx.host.checks(), vec![ms(400)]);
        assert!(!lock(&fx.host.tokens)[0].is_cancelled());
        fx.at(20_401).await;
        assert!(lock(&fx.host.tokens)[0].is_cancelled(), "멈춘 git 에 취소를 보낸다");
        fx.host.hang_checks.store(false, Ordering::SeqCst);
        fx.host.set_sig(2);
        fx.dirs.emit("HEAD");
        fx.at(21_000).await;
        assert_eq!(fx.host.checks().len(), 2);
        assert_eq!(fx.host.changes(), vec!["r"]);
    }

    // ── 세대·종료 ───────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn 저장소를_바꾸면_이전_저장소의_늦게_끝난_확인은_알리지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        *lock(&fx.host.check_delay) = ms(5_000);
        fx.host.set_sig(2);
        fx.dirs.emit("HEAD");
        fx.at(1_000).await;
        assert_eq!(fx.host.checks().len(), 1, "확인이 진행 중");
        fx.w.watch(Some("other"));
        settle().await;
        assert_eq!(fx.w.current().as_deref(), Some("other"));
        assert!(lock(&fx.host.tokens)[0].is_cancelled(), "이전 확인의 git 도 취소된다");
        fx.at(7_000).await;
        assert!(fx.host.changes().is_empty());
        assert_eq!(fx.dirs.open(), 1, "이전 저장소의 감시는 닫히고 새 저장소 것만 남는다");
        assert_eq!(fx.dirs.watched().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn 멈추면_감시와_폴링이_끝나고_더_알리지_않는다() {
        let fx = Fx::started(WatchKind::Local).await;
        assert_eq!(fx.dirs.open(), 1);
        fx.w.watch(None);
        settle().await;
        assert_eq!(fx.w.current(), None);
        assert_eq!(fx.dirs.open(), 0);
        fx.host.set_sig(2);
        fx.dirs.emit("HEAD");
        fx.w.set_focused(true);
        fx.at(200_000).await;
        assert!(fx.host.checks().is_empty());
        assert!(fx.host.changes().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn 정지와_교체는_돌아오기_전에_핸들을_닫는다() {
        let fx = Fx::started(WatchKind::Local).await;
        assert_eq!(fx.dirs.open(), 1);
        fx.w.watch(None);
        assert_eq!(fx.dirs.open(), 0, "settle 없이 곧바로 닫혀 있어야 한다");

        fx.w.watch(Some("r"));
        settle().await;
        assert_eq!(fx.dirs.open(), 1);
        fx.w.watch(Some("other"));
        assert_eq!(fx.dirs.open(), 0, "교체도 이전 핸들을 먼저 닫는다(새 감시는 태스크가 연다)");
        settle().await;
        assert_eq!(fx.dirs.open(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unwatch_if_는_감시_중인_저장소일_때만_멈춘다() {
        let fx = Fx::started(WatchKind::Local).await;
        assert!(!fx.w.unwatch_if("other"));
        assert_eq!((fx.w.current().as_deref(), fx.dirs.open()), (Some("r"), 1));
        assert!(fx.w.unwatch_if("r"));
        assert_eq!((fx.w.current(), fx.dirs.open()), (None, 0), "settle 없이 곧바로 닫혀 있어야 한다");
        assert!(!fx.w.unwatch_if("r"), "이미 멈췄다");
        fx.host.set_sig(2);
        fx.w.set_focused(true);
        fx.at(200_000).await;
        assert!(fx.host.checks().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dispose_없이_소유자를_버려도_태스크와_핸들이_정리된다() {
        let Fx { host, dirs, w, t0 } = Fx::started(WatchKind::Local).await;
        assert_eq!(dirs.open(), 1);
        // 태스크가 쥔 Inner 가 host 를 붙잡고 있다
        assert!(Arc::strong_count(&host) > 1);
        drop(w);
        assert_eq!(dirs.open(), 0, "버리는 즉시 핸들이 닫힌다");
        settle().await;
        assert_eq!(Arc::strong_count(&host), 1, "태스크가 끝나 Inner 가 사라졌다");
        host.set_sig(2);
        dirs.emit("HEAD");
        tokio::time::sleep_until(t0 + ms(200_000)).await;
        assert!(host.checks().is_empty(), "폴링 git 이 더 불리지 않는다");
        assert!(host.changes().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn 같은_저장소를_다시_감시해도_아무것도_하지_않고_dispose_는_멈춘다() {
        let fx = Fx::started(WatchKind::Local).await;
        fx.w.watch(Some("r"));
        settle().await;
        assert_eq!(fx.host.baselines(), 1);
        assert_eq!(fx.dirs.watched().len(), 1);
        fx.w.dispose();
        settle().await;
        assert_eq!((fx.w.current(), fx.dirs.open()), (None, 0));
    }

    // ── 디렉터리 거부 ───────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn unc_경로와_head_가_없는_디렉터리는_감시하지_않는다() {
        let fx = Fx::new(Some(WatchKind::Local));
        *lock(&fx.host.git_dirs) = Some(
            [
                "\\\\server\\share\\.git",
                "//host/x/.git",
                "\\??\\UNC\\server\\share\\.git",
                "//??/UNC/server/share/.git",
                "C:\\repo\\.git",
                "C:\\nohead",
                "/home/x/.git",
            ]
            .map(String::from)
            .to_vec(),
        );
        lock(&fx.dirs.no_head).push("C:\\nohead".to_string());
        fx.w.watch(Some("r"));
        settle().await;
        assert_eq!(fx.dirs.watched(), vec!["C:\\repo\\.git", "/home/x/.git"]);
        // 둘 중 어느 디렉터리의 이벤트든 확인을 부른다(워크트리의 공통 디렉터리)
        lock(&fx.dirs.sinks)[1].1(WatchEvent::at("refs/heads/x"));
        fx.at(401).await;
        assert_eq!(fx.host.checks().len(), 1);
    }

    #[test]
    fn 네트워크_경로_판정은_unc_와_nt_네임스페이스를_거부한다() {
        for (d, remote) in [
            ("\\\\server\\share", true),
            ("//host/x", true),
            ("\\??\\UNC\\server\\share", true),
            ("//??/UNC/server/share", true),
            ("C:\\x\\??\\y", true),
            ("C:\\repo\\.git", false),
            ("/home/x/.git", false),
            ("", false),
        ] {
            assert_eq!(is_remote_path(d), remote, "{d:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn 멈춘_git_디렉터리_조회는_20초에_취소되고_안전망_폴링만_남는다() {
        let fx = Fx::new(Some(WatchKind::Local));
        fx.host.hang_dirs.store(true, Ordering::SeqCst);
        fx.w.watch(Some("r"));
        fx.at(19_000).await;
        assert!(!lock(&fx.host.dir_tokens)[0].is_cancelled(), "제한 시간 전에는 취소하지 않는다");
        fx.at(20_001).await;
        settle().await;
        assert!(lock(&fx.host.dir_tokens)[0].is_cancelled(), "제한 시간이 지나면 git 에 취소를 보낸다");
        assert!(fx.dirs.watched().is_empty());
        // 감시 루프는 계속 돈다 — 제한 시간이 지난 뒤 60초에 안전망 폴링
        fx.at(80_001).await;
        assert_eq!(fx.host.checks().len(), 1);
        // 저장소를 바꾸면(취소) 조회 중인 신호도 취소된다
        let fx2 = Fx::new(Some(WatchKind::Local));
        fx2.host.hang_dirs.store(true, Ordering::SeqCst);
        fx2.w.watch(Some("r"));
        settle().await;
        fx2.w.watch(None);
        settle().await;
        assert!(lock(&fx2.host.dir_tokens)[0].is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn git_디렉터리를_못_구하면_감시_없이_안전망_폴링만_한다() {
        let fx = Fx::new(Some(WatchKind::Local));
        *lock(&fx.host.git_dirs) = None;
        fx.w.watch(Some("r"));
        settle().await;
        assert!(fx.dirs.watched().is_empty());
        fx.at(60_001).await;
        assert_eq!(fx.host.checks().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn 종류를_모르는_저장소는_폴링도_감시도_하지_않는다() {
        let fx = Fx::started_none().await;
        fx.at(3_600_000).await;
        assert!(fx.dirs.watched().is_empty());
        assert!(fx.host.checks().is_empty());
        // 포커스로 부르는 확인은 가능하다(TS 도 종류와 무관하게 확인한다)
        fx.w.set_focused(true);
        settle().await;
        assert_eq!(fx.host.checks().len(), 1);
    }

    impl Fx {
        async fn started_none() -> Fx {
            let fx = Fx::new(None);
            fx.w.watch(Some("r"));
            settle().await;
            fx
        }
    }
}
