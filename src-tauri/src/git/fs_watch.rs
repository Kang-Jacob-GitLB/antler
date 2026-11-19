//! 저장소 감시의 실제 이벤트원: [`notify`](https://docs.rs/notify) 어댑터(Phase 6a PR 7b). 상태 기계(`watcher.rs`)의 [`DirWatch`] 를 구현한다.
//! Windows 는 ReadDirectoryChangesW 재귀, Linux 는 inotify 다(Node `fs.watch(dir, {recursive:true})` 의 자리).
//!
//! - **경로**: notify 는 우리가 넘긴 감시 루트에 상대 이름을 붙여 보고한다. 같은 `Path` 로 `strip_prefix` 해서 **감시 디렉터리 기준 상대 경로**로 바꾼다
//!   (`objects/` 판정은 `WatchEvent::should_kick`). 접두를 못 떼면(심볼릭 링크 등) 경로를 모르는 이벤트로 — 항상 확인한다.
//! - **걸러내는 것**: 접근(`EventKind::Access` — notify 8.2 의 inotify 감시 마스크에는 `OPEN`·`CLOSE_WRITE` 가 있어 열기·쓰기 닫기가 접근 이벤트로 온다. 우리 git 이 `.git` 의 파일을 여는 것이 이벤트가 되어
//!   확인이 확인을 부르는 고리를 막는다. 쓰기 자체는 `Modify`·`Create` 로도 오므로 놓치지 않는다. Windows 백엔드는 접근 이벤트를 내지 않는다)과 오류
//!   (TS 는 `w.on('error', () => {})` — 디렉터리가 사라지면 안전망 폴링이 처리한다). 넘침(rescan 표시)은 무엇이 바뀌었는지 모르므로 확인한다.
//! - **막지 않기**: `has_head`(stat)·감시 등록(notify 가 등록 확인을 기다린다)은 blocking 풀에서 돌린다 — 느린 네트워크 드라이브가 tokio 워커를 붙잡지 않는다.
//! - **닫기**: notify 의 `Drop` 은 스레드에 정지만 알리고 돌아온다(join 하지 않는다). 핸들 `Drop` 은 **닫기 요청을 즉시** 하고, 완료(notify 스레드가 이벤트 핸들러를 놓는 때)는
//!   tokio 런타임 밖이거나 종료 중([`NotifyDirs`] 의 `begin_shutdown`)일 때만 [`CLOSE_WAIT`] 까지 기다린다. 런타임 안에서는 기다리지 않는다 — 런타임 밖 스레드·`watch_target` 잠금 안에서 최대 1.5초를 붙잡지 않게.
//!   스레드가 곧 끝난다는 것은 [`NotifyDirs::live`] 가 보여 준다.
//! - **링크·네트워크**: `has_head` 는 링크를 따라가지 않는다(`symlink_metadata` — 디렉터리 자체와 `HEAD` 가 실제 디렉터리·파일일 때만). notify 도 감시 안의 링크를 따라가지 않는다(`follow_symlinks(false)`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::watcher::{DirWatch, EventSink, WatchEvent, WatchHandle};

/// 핸들을 닫을 때 notify 스레드가 끝나기를 기다리는 상한. 보통은 몇 ms 안에 끝난다(정지는 세마포어로 스레드를 바로 깨운다)
pub const CLOSE_WAIT: Duration = Duration::from_millis(1_500);

/// notify 이벤트 하나를 감시 디렉터리 기준의 [`WatchEvent`] 들로 바꾼다. 확인할 필요가 없는 것(접근·오류)은 비어 있다
fn translate(root: &Path, res: notify::Result<Event>) -> Vec<WatchEvent> {
    let Ok(event) = res else { return Vec::new() };
    if event.need_rescan() {
        return vec![WatchEvent { path: None, rescan: true }];
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    if event.paths.is_empty() {
        return vec![WatchEvent::default()];
    }
    event
        .paths
        .iter()
        .map(|p| WatchEvent { path: p.strip_prefix(root).ok().map(|r| r.to_string_lossy().into_owned()), rescan: false })
        .collect()
}

/// 감시 하나의 생사. notify 스레드가 이벤트 핸들러를 놓을 때 [`Drop`] 이 돌아 살아 있는 수를 줄이고 기다리는 쪽을 깨운다
struct Life {
    live: Arc<AtomicUsize>,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl Life {
    fn new(live: &Arc<AtomicUsize>) -> Life {
        live.fetch_add(1, Ordering::SeqCst);
        Life { live: live.clone(), done: Arc::new((Mutex::new(false), Condvar::new())) }
    }
}

impl Drop for Life {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        let (flag, cv) = &*self.done;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        cv.notify_all();
    }
}

/// 열린 감시. 버리면 닫는다(위 "닫기")
struct NotifyHandle {
    watcher: Option<RecommendedWatcher>,
    done: Arc<(Mutex<bool>, Condvar)>,
    /// 앱 종료가 시작됐다(`DirWatch::begin_shutdown`) — 이때만은 런타임 안에서도 기다린다
    shutdown: Arc<AtomicBool>,
}

impl Drop for NotifyHandle {
    fn drop(&mut self) {
        // 닫기 요청은 즉시 — notify 가 스레드에 정지를 알리고 돌아온다
        drop(self.watcher.take());
        // 완료를 기다리는 것은 종료 중이거나 tokio 런타임 밖일 때뿐이다. 런타임 안(`watch_target` 잠금 안·태스크)에서는
        // 최대 1.5초를 붙잡아 다른 요청을 막으므로 기다리지 않는다 — 스레드는 곧 끝나고 `NotifyDirs::live` 가 그 사실을 보여 준다
        if !self.shutdown.load(Ordering::SeqCst) && tokio::runtime::Handle::try_current().is_ok() {
            return;
        }
        let (flag, cv) = &*self.done;
        let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
        // 시간 초과여도 기다림을 접는다(스레드가 멈췄어도 정지를 부른 쪽이 붙잡히지 않는다)
        let _ = cv.wait_timeout_while(guard, CLOSE_WAIT, |done| !*done);
    }
}

/// `root` 를 재귀로 감시한다(blocking 풀에서 부른다)
fn open(root: PathBuf, sink: EventSink, live: &Arc<AtomicUsize>, shutdown: &Arc<AtomicBool>) -> Result<WatchHandle, String> {
    let life = Life::new(live);
    let done = life.done.clone();
    let base = root.clone();
    // `life` 는 이벤트 핸들러가 쥔다 — 핸들러를 쥔 notify 스레드가 끝나면 함께 사라진다(등록에 실패해 핸들러가 버려져도 같다)
    // 링크를 따라가지 않는다(기본값은 따라간다) — 감시 디렉터리 안의 링크가 밖(네트워크 경로 포함)으로 이어져 접속·이벤트 폭주를 일으키지 않게
    let watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let _alive = &life;
            for e in translate(&base, res) {
                sink(e);
            }
        },
        notify::Config::default().with_follow_symlinks(false),
    )
    .map_err(|e| e.to_string())?;
    let mut handle = NotifyHandle { watcher: Some(watcher), done, shutdown: shutdown.clone() };
    if let Some(w) = handle.watcher.as_mut() {
        w.watch(&root, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
    }
    Ok(Box::new(handle))
}

/// 링크를 따라가지 않고(`symlink_metadata`) 디렉터리 자체가 실제 디렉터리이고 `HEAD` 가 실제 파일인가. 링크는 어느 쪽이든 거부한다 —
/// `exists`·`metadata` 는 링크를 따라가므로 UNC 를 가리키는 링크면 stat 이 SMB 접속을 일으킨다
fn is_real_git_dir(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir()) && std::fs::symlink_metadata(dir.join("HEAD")).is_ok_and(|m| m.file_type().is_file())
}

/// notify 로 구현한 [`DirWatch`]. 복제하면 같은 살아 있는 수를 본다(라우터가 하나를 쥐고 시험이 같은 값을 읽는다)
#[derive(Clone, Default)]
pub struct NotifyDirs {
    live: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
}

impl NotifyDirs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 이 인스턴스가 연 감시 중 notify 스레드가 아직 끝나지 않은 수(종료 뒤 0 이어야 한다)
    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }
}

impl DirWatch for NotifyDirs {
    fn has_head(&self, dir: &str) -> BoxFuture<'static, bool> {
        let dir = PathBuf::from(dir);
        async move { tokio::task::spawn_blocking(move || is_real_git_dir(&dir)).await.unwrap_or(false) }.boxed()
    }

    fn watch(&self, dir: &str, sink: EventSink) -> BoxFuture<'static, Result<WatchHandle, String>> {
        let (root, live, shutdown) = (PathBuf::from(dir), self.live.clone(), self.shutdown.clone());
        async move { tokio::task::spawn_blocking(move || open(root, sink, &live, &shutdown)).await.map_err(|e| e.to_string())? }.boxed()
    }

    fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::time::Instant;

    /// 임시 폴더(Drop 에서 지운다). 윈도 CI 의 TEMP 는 8.3 짧은 이름일 수 있지만 상대 경로만 비교하므로 맞출 필요가 없다
    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Tmp {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("antler-fswatch-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            fs::create_dir_all(dir.join("refs").join("heads")).unwrap();
            fs::create_dir_all(dir.join("objects").join("ab")).unwrap();
            fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
            Tmp(dir)
        }
        fn s(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ev(kind: EventKind, paths: &[&Path]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(p.to_path_buf());
        }
        e
    }

    #[test]
    fn 접근과_오류는_버리고_경로는_감시_디렉터리_기준으로_바꾼다() {
        use notify::event::{AccessKind, CreateKind, Flag, ModifyKind};
        let root = Path::new("/r/.git");
        let sub = root.join("refs").join("heads").join("x");
        assert!(translate(root, Ok(ev(EventKind::Access(AccessKind::Any), &[&sub]))).is_empty());
        assert!(translate(root, Err(notify::Error::generic("x"))).is_empty());
        let got = translate(root, Ok(ev(EventKind::Create(CreateKind::File), &[&sub])));
        assert_eq!(got.len(), 1);
        assert!(got[0].path.as_deref().is_some_and(|p| p.replace('\\', "/") == "refs/heads/x") && !got[0].rescan);
        assert!(got[0].should_kick());
        // 객체 파일은 걸러지는 경로로 나온다(필터는 상태 기계의 `should_kick`)
        let obj = translate(root, Ok(ev(EventKind::Modify(ModifyKind::Any), &[&root.join("objects").join("ab").join("cd")])));
        assert!(!obj[0].should_kick());
        // 이름 바꾸기는 경로가 둘이다
        let moved = translate(root, Ok(ev(EventKind::Modify(ModifyKind::Any), &[&root.join("a.lock"), &root.join("a")])));
        assert_eq!(moved.len(), 2);
        // 경로를 모르면(비어 있음·접두가 다름) 확인한다
        assert_eq!(translate(root, Ok(ev(EventKind::Other, &[]))), vec![WatchEvent::default()]);
        let outside = translate(root, Ok(ev(EventKind::Modify(ModifyKind::Any), &[Path::new("/elsewhere/x")])));
        assert_eq!(outside, vec![WatchEvent { path: None, rescan: false }]);
        assert!(outside[0].should_kick());
        // 넘침은 항상 확인한다
        let rescan = translate(root, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
        assert_eq!(rescan, vec![WatchEvent { path: None, rescan: true }]);
    }

    /// 이벤트를 모아 두는 싱크와 읽는 쪽
    fn collector() -> (EventSink, mpsc::Receiver<WatchEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |e| drop(tx.lock().unwrap_or_else(PoisonError::into_inner).send(e))), rx)
    }

    /// 조건에 맞는 이벤트가 올 때까지(상한 20초) 받은 것을 모두 돌려준다 — 고정 sleep 으로 가정하지 않는다
    fn collect_until(rx: &mpsc::Receiver<WatchEvent>, what: &str, mut done: impl FnMut(&WatchEvent) -> bool) -> Vec<WatchEvent> {
        let until = Instant::now() + Duration::from_secs(20);
        let mut got = Vec::new();
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let e = rx.recv_timeout(left).unwrap_or_else(|_| panic!("시간 안에 {what} 이벤트가 오지 않았다: {got:?}"));
            let hit = done(&e);
            got.push(e);
            if hit {
                return got;
            }
        }
    }

    fn rel(e: &WatchEvent) -> String {
        e.path.clone().unwrap_or_default().replace('\\', "/")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 실제_파일_변화가_상대_경로_이벤트로_오고_objects_는_걸러지는_경로다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        assert!(dirs.has_head(&tmp.s()).await);
        let (sink, rx) = collector();
        let handle = dirs.watch(&tmp.s(), sink).await.expect("감시 등록");
        assert_eq!(dirs.live(), 1);
        // 객체 파일을 먼저, ref 를 나중에 만든다 — 순서대로 오므로 ref 이벤트가 오면 객체 이벤트는 이미 받았다
        fs::write(tmp.0.join("objects").join("ab").join("cdef"), "x").unwrap();
        fs::write(tmp.0.join("refs").join("heads").join("feature"), "0000\n").unwrap();
        let got = collect_until(&rx, "refs/heads/feature", |e| rel(e).starts_with("refs/") && e.should_kick());
        assert!(got.iter().any(|e| rel(e).starts_with("objects/")), "객체 변화도 이벤트로는 온다(걸러내는 것은 상태 기계다): {got:?}");
        assert!(got.iter().filter(|e| rel(e).starts_with("objects/")).all(|e| !e.should_kick()), "objects/ 이벤트는 확인을 부르면 안 된다: {got:?}");
        assert!(got.iter().all(|e| !rel(e).contains(&tmp.s().replace('\\', "/"))), "절대 경로가 새면 안 된다: {got:?}");
        drop(handle);
        assert!(live_reaches(&dirs, 0).await, "핸들을 닫으면 notify 스레드가 곧 끝난다");
        // 닫은 뒤의 변화는 오지 않는다
        while rx.try_recv().is_ok() {}
        fs::write(tmp.0.join("refs").join("heads").join("after"), "0000\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "닫은 뒤에 이벤트가 왔다");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn head_가_없거나_없는_디렉터리는_거부하고_등록_실패는_살아_있는_수를_남기지_않는다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        fs::remove_file(tmp.0.join("HEAD")).unwrap();
        assert!(!dirs.has_head(&tmp.s()).await);
        assert!(!dirs.has_head(&tmp.0.join("none").to_string_lossy()).await);
        let (sink, _rx) = collector();
        assert!(dirs.watch(&tmp.0.join("none").to_string_lossy(), sink).await.is_err());
        assert!(live_reaches(&dirs, 0).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 여러_감시를_열고_닫아도_스레드가_남지_않는다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        let (sink, _rx) = collector();
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(dirs.watch(&tmp.s(), sink.clone()).await.expect("감시 등록"));
        }
        assert_eq!(dirs.live(), 8);
        drop(handles);
        assert!(live_reaches(&dirs, 0).await, "런타임 안에서 닫으면 기다리지 않지만 스레드는 곧 끝난다");
    }

    /// 런타임 밖에서 닫으면 notify 스레드가 끝날 때까지 기다린다(돌아온 즉시 0)
    #[test]
    fn 런타임_밖에서_닫으면_스레드_종료까지_기다린다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        let (sink, _rx) = collector();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let handle = rt.block_on(dirs.watch(&tmp.s(), sink)).expect("감시 등록");
        assert_eq!(dirs.live(), 1);
        drop(handle);
        assert_eq!(dirs.live(), 0);
    }

    /// 종료가 시작되면 런타임 안에서도 끝날 때까지 기다린다(`RepoWatcher::dispose` 가 알린다)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 종료가_시작되면_런타임_안에서도_스레드_종료까지_기다린다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        let (sink, _rx) = collector();
        let handle = dirs.watch(&tmp.s(), sink).await.expect("감시 등록");
        dirs.begin_shutdown();
        drop(handle);
        assert_eq!(dirs.live(), 0);
    }

    /// 링크는 따라가지 않는다 — 디렉터리 자체가 링크이거나 `HEAD` 가 링크면 거부한다(심볼릭 링크를 못 만드는 환경은 건너뛴다)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn 링크인_디렉터리와_head_는_거부한다() {
        let tmp = Tmp::new();
        let dirs = NotifyDirs::new();
        assert!(dirs.has_head(&tmp.s()).await);
        // HEAD 가 디렉터리면 파일이 아니다
        let other = Tmp::new();
        fs::remove_file(other.0.join("HEAD")).unwrap();
        fs::create_dir(other.0.join("HEAD")).unwrap();
        assert!(!dirs.has_head(&other.s()).await);
        // HEAD 가 링크
        let linked = Tmp::new();
        fs::remove_file(linked.0.join("HEAD")).unwrap();
        if make_link(&tmp.0.join("HEAD"), &linked.0.join("HEAD"), false) {
            assert!(!dirs.has_head(&linked.s()).await, "HEAD 링크는 따라가지 않는다");
        }
        // 디렉터리 자체가 링크
        let link = std::env::temp_dir().join(format!("antler-fswatch-link-{}", std::process::id()));
        if make_link(&tmp.0, &link, true) {
            assert!(!dirs.has_head(&link.to_string_lossy()).await, "디렉터리 링크는 따라가지 않는다");
            let _ = fs::remove_dir(&link);
        }
    }

    #[cfg(unix)]
    fn make_link(target: &Path, link: &Path, _dir: bool) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// Windows: 파일은 심볼릭 링크(권한이 없으면 실패 → 건너뜀), 디렉터리는 심볼릭 링크 시도
    #[cfg(windows)]
    fn make_link(target: &Path, link: &Path, dir: bool) -> bool {
        if dir {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        } else {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
    }

    /// 살아 있는 notify 스레드 수가 `n` 이 되기를 기다린다(상한 5초)
    async fn live_reaches(dirs: &NotifyDirs, n: usize) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while dirs.live() != n {
            if Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }
}
