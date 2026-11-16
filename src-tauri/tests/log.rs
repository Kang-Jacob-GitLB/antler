//! `git:log` 의 커서·서비스 통합 테스트(실제 git). `tests/logCursor.test.ts`·`tests/repoService.test.ts` 의 log 케이스를 옮기고,
//! Rust 에서 새로 생긴 약속(커서를 버리면 git 프로세스가 끝난다·유휴 정리·`repos:watch`·미러 변경·사이드카 알림)을 본다.
//! 채널 패리티(TS 골든)는 `tests/channels.rs` 가 맡는다.
//!
//! 격리: 다른 통합 테스트와 같다 — 시작할 때 `GIT_CONFIG_GLOBAL`(커밋된 `tests/gitconfig`)·`GIT_CONFIG_NOSYSTEM` 을 넣는다.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use antler_lib::git::exec::{GitExecutor, LocalExecutor, RunOpts};
use antler_lib::git::log_service::{parse_query, CursorError, LogCursor, LogError, LogRequest, LogService, NoteOp};
use antler_lib::git::parsers::log::{parse_log, Commit};
use antler_lib::git::scope::log_args;
use antler_lib::mirror::{MirrorRepo, RepoMirror, Target};
use antler_lib::router::{Routed, Router};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

/// 테스트 준비용 날 프로세스. **제품 코드는 이 길을 쓰지 않는다**(clippy.toml 의 disallowed-methods)
#[allow(clippy::disallowed_methods)]
fn raw(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

fn raw_git(dir: &Path) -> std::process::Command {
    let mut c = raw("git");
    c.current_dir(dir);
    c
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-log-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn join(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        for _ in 0..20 {
            if fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

/// 새 저장소에 fast-import 스트림을 먹인다
fn feed(dir: &Path, stream: &[u8]) {
    let mut child = raw_git(dir).args(["fast-import", "--quiet"]).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stream).unwrap();
    assert!(child.wait().unwrap().success(), "git fast-import");
}

fn init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    assert!(raw_git(dir).args(["init", "-q", "-b", "main"]).status().unwrap().success(), "git init");
}

/// main 위에 커밋 n 개가 일직선인 저장소. 출력이 파이프 버퍼보다 커서 소비하지 않으면 git 이 멈춘다(n ≥ 2000)
fn linear_repo(dir: &Path, n: usize) {
    init(dir);
    let mut s = String::new();
    for i in 1..=n {
        let msg = format!("commit {i} 안녕하세요 lorem ipsum dolor sit amet consectetur adipiscing elit");
        s += &format!("commit refs/heads/main\nmark :{i}\ncommitter T <t@t> {} +0000\ndata {}\n{msg}\n", 1_700_000_000 + i * 60, msg.len());
        if i > 1 {
            s += &format!("from :{}\n", i - 1);
        }
        s += "\n";
    }
    feed(dir, s.as_bytes());
}

/// tests/fixtures/repos/<name>.fi 를 import 한 저장소
fn fixture_repo(dir: &Path, name: &str) {
    init(dir);
    feed(dir, &fs::read(repo_root().join("tests/fixtures/repos").join(format!("{name}.fi"))).unwrap());
}

fn local() -> GitExecutor {
    GitExecutor::Local(LocalExecutor::default())
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

async fn full_log(dir: &Path) -> Vec<Commit> {
    let args = log_args(None, 0).unwrap();
    parse_log(&local().run(&path_str(dir), &args, &RunOpts::default()).await.unwrap().stdout)
}

fn cursor(dir: &Path, start: u64) -> LogCursor {
    LogCursor::new(&local(), &path_str(dir), &log_args(None, start).unwrap(), start)
}

fn hashes(commits: &[Commit]) -> Vec<&str> {
    commits.iter().map(|c| c.hash.as_str()).collect()
}

const COMMITS: usize = 1200;

/// 테스트마다 새로 만든다(정적으로 공유하면 프로세스가 끝날 때 Drop 이 돌지 않아 임시 폴더가 남는다)
fn linear_in(tag: &str, n: usize) -> (TempDir, PathBuf) {
    let tmp = TempDir::new(tag);
    let dir = tmp.join("repo 저장소");
    linear_repo(&dir, n);
    (tmp, dir)
}

/// 1200 커밋 저장소
fn shared_repo() -> (TempDir, PathBuf) {
    linear_in("shared", COMMITS)
}

// ── 커서(logCursor.test.ts) ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 페이지를_이어_붙이면_전체_log_와_같다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let all = full_log(dir).await;
    assert_eq!(all.len(), COMMITS);
    let c = cursor(dir, 0);
    let mut got: Vec<Commit> = Vec::new();
    loop {
        let page = c.read(250).await.unwrap();
        got.extend(page.commits);
        if !page.has_more {
            break;
        }
    }
    assert_eq!(hashes(&got), hashes(&all));
    assert_eq!(c.position(), COMMITS as u64);
    assert!(c.ended());
    c.dispose();
    assert!(!c.usable());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skip_위치에서_시작한_커서는_같은_구간을_돌려준다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let all = full_log(dir).await;
    let c = cursor(dir, 700);
    let page = c.read(100).await.unwrap();
    assert_eq!(hashes(&page.commits), hashes(&all[700..800]));
    assert!(page.has_more);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 동시에_read_해도_각_페이지가_자기_구간이다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let all = full_log(dir).await;
    let c = cursor(dir, 0);
    // 호출 시점에 예약되므로 future 를 만든 순서가 곧 읽는 순서다 — 합쳐서 기다려도 각 페이지는 자기 구간과 같아야 한다
    let futs: Vec<_> = (0..20).map(|_| c.read(10)).collect();
    assert_eq!(c.next_position(), 200);
    let pages = futures_util::future::join_all(futs).await;
    for (i, p) in pages.into_iter().enumerate() {
        assert_eq!(hashes(&p.unwrap().commits), hashes(&all[i * 10..i * 10 + 10]), "페이지 {i}");
    }
    assert_eq!(c.position(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 커밋_없는_저장소는_빈_페이지() {
    let tmp = TempDir::new("empty");
    let dir = tmp.join("empty");
    init(&dir);
    let c = cursor(&dir, 0);
    let page = c.read(100).await.unwrap();
    assert!(page.commits.is_empty());
    assert!(!page.has_more);
    assert!(c.usable() && c.ended());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispose_후_read_는_실패한다() {
    let (_tmp, dir) = shared_repo();
    let c = cursor(&dir, 0);
    c.dispose();
    assert!(c.read(10).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn head_가_아직_커밋_없는_orphan_브랜치여도_다른_브랜치_커밋을_보여_준다() {
    let tmp = TempDir::new("orphan");
    let dir = tmp.join("orphan");
    init(&dir);
    let g = |args: &[&str]| assert!(raw_git(&dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).status().unwrap().success(), "{args:?}");
    g(&["commit", "-q", "--allow-empty", "-m", "on main"]);
    g(&["switch", "-q", "--orphan", "fresh"]);
    let page = cursor(&dir, 0).read(10).await.unwrap();
    assert_eq!(page.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["on main"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_이_실패한_커서는_사용할_수_없다() {
    let c = cursor(Path::new("/nonexistent-antler-log-cursor"), 0);
    assert!(matches!(c.read(10).await, Err(CursorError::Exec(_))));
    assert!(!c.usable());
    // 실패한 뒤의 요청은 끊긴 것으로 끝난다(멈추지 않는다)
    assert!(c.read(10).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 끝에서_덜_받으면_예약_위치를_되돌린다() {
    let (_tmp, dir) = shared_repo();
    let c = cursor(&dir, (COMMITS - 5) as u64);
    let page = c.read(100).await.unwrap();
    assert_eq!(page.commits.len(), 5);
    assert!(!page.has_more);
    assert_eq!(c.next_position(), COMMITS as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 확인_읽기는_앵커와_다르면_불일치로_끝난다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let all = full_log(dir).await;
    let ok = cursor(dir, 9);
    assert_eq!(hashes(&ok.verify_then_read(all[9].hash.clone(), 5).await.unwrap().commits), hashes(&all[10..15]));
    assert_eq!(ok.next_position(), 15);
    let bad = cursor(dir, 9);
    assert_eq!(bad.verify_then_read(all[8].hash.clone(), 5).await.unwrap_err(), CursorError::Mismatch);
}

// ── 프로세스가 정말 끝나는가(Windows) ────────────────────────────

/// pid → 부모 pid 스냅샷
#[cfg(windows)]
fn process_snapshot() -> Vec<(u32, u32)> {
    let out = raw("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId),$($_.ParentProcessId)\" }"])
        .output()
        .expect("powershell 실행");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (a, b) = l.trim().split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect()
}

/// root 와 그 자손
#[cfg(windows)]
fn tree_of(root: u32, snap: &[(u32, u32)]) -> Vec<u32> {
    let mut tree = vec![root];
    let mut i = 0;
    while i < tree.len() {
        let parent = tree[i];
        for &(pid, ppid) in snap {
            if ppid == parent && !tree.contains(&pid) {
                tree.push(pid);
            }
        }
        i += 1;
    }
    tree
}

/// 트리가 모두 사라질 때까지 기다린다
#[cfg(windows)]
async fn assert_tree_dies(tree: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snap = process_snapshot();
        let alive: Vec<u32> = tree.iter().copied().filter(|p| snap.iter().any(|(pid, _)| pid == p)).collect();
        if alive.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "git 트리가 남았다: {alive:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// 한 번 읽어 git 이 파이프에서 멈춘 커서의 프로세스 트리
#[cfg(windows)]
fn live_tree(pid: u32) -> Vec<u32> {
    let snap = process_snapshot();
    assert!(snap.iter().any(|(p, _)| *p == pid), "멈춘 git 이 떠 있어야 한다(출력이 파이프보다 커야 한다)");
    tree_of(pid, &snap)
}

/// 4000 커밋 저장소 — 출력이 파이프 버퍼보다 훨씬 커서 소비하지 않으면 git 이 멈춘다
fn big_repo() -> (TempDir, PathBuf) {
    linear_in("big", 4000)
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 커서를_닫으면_멈춰_있던_git_트리가_끝난다() {
    let (_tmp, dir) = big_repo();
    let dir = &dir;
    let c = cursor(dir, 0);
    assert_eq!(c.read(10).await.unwrap().commits.len(), 10);
    let tree = live_tree(c.pid().expect("pid"));
    c.dispose();
    assert_tree_dies(&tree).await;
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 커서를_그냥_버려도_git_트리가_끝난다() {
    let (_tmp, dir) = big_repo();
    let dir = &dir;
    let c = cursor(dir, 0);
    assert_eq!(c.read(10).await.unwrap().commits.len(), 10);
    let tree = live_tree(c.pid().expect("pid"));
    drop(c);
    assert_tree_dies(&tree).await;
}

// ── 서비스 ──────────────────────────────────────────────

fn mirror_of(entries: &[(&str, &Path)]) -> RepoMirror {
    let m = RepoMirror::default();
    m.apply(1, entries.iter().map(|(id, p)| MirrorRepo { id: (*id).into(), kind: "local".into(), path: path_str(p), distro: None }).collect());
    m
}

fn req(skip: u64, count: usize) -> LogRequest {
    LogRequest { scope: None, count, skip }
}

fn target(dir: &Path) -> Target {
    Target::Local { path: path_str(dir) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 유휴_커서는_닫히고_git_이_끝나며_앵커로_이어_읽는다() {
    let (_tmp, dir) = big_repo();
    let dir = &dir;
    let svc = LogService::with_timing(mirror_of(&[("a", dir)]), Duration::from_millis(400), Duration::from_millis(100));
    let (exec, t, tok) = (local(), target(dir), CancellationToken::new());
    let p1 = svc.log("a", &exec, &t, &req(0, 100), &tok).await.unwrap();
    assert_eq!(p1.commits.len(), 100);
    assert_eq!(svc.cursor_count(), 1);
    #[cfg(windows)]
    let pid = svc.cursor_pids()[0];
    // 훑는 태스크가 유휴 커서를 닫는다
    let deadline = Instant::now() + Duration::from_secs(10);
    while svc.cursor_count() > 0 {
        assert!(Instant::now() < deadline, "유휴 커서가 닫히지 않았다");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    #[cfg(windows)]
    assert_tree_dies(&[pid]).await;
    // 닫혔어도 앵커로 끊김 없이 이어진다
    assert_eq!(svc.anchor_count(), 1);
    let p2 = svc.log("a", &exec, &t, &req(100, 100), &tok).await.unwrap();
    let all = full_log(dir).await;
    assert_eq!(hashes(&p2.commits), hashes(&all[100..200]));
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 유휴로_닫으면_멈춰_있던_git_트리가_끝난다() {
    let (_tmp, dir) = big_repo();
    let dir = &dir;
    // 훑는 태스크는 거의 안 돌게 해 두고(프로세스 목록을 읽는 동안 먼저 닫히지 않게) 시각을 앞당겨 직접 훑는다
    let svc = LogService::with_timing(mirror_of(&[("a", dir)]), Duration::from_secs(3600), Duration::from_secs(3600));
    svc.log("a", &local(), &target(dir), &req(0, 10), &CancellationToken::new()).await.unwrap();
    let tree = live_tree(svc.cursor_pids()[0]);
    assert_eq!(svc.sweep_idle(Instant::now() + Duration::from_secs(7200)), 0);
    assert_tree_dies(&tree).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sweep_는_유휴_시간이_지나야_닫는다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let svc = LogService::new(mirror_of(&[("a", dir)]));
    let (exec, t, tok) = (local(), target(dir), CancellationToken::new());
    svc.log("a", &exec, &t, &req(0, 10), &tok).await.unwrap();
    assert_eq!(svc.sweep_idle(Instant::now()), 1);
    assert_eq!(svc.sweep_idle(Instant::now() + Duration::from_secs(4)), 1);
    assert_eq!(svc.sweep_idle(Instant::now() + Duration::from_secs(6)), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 요청을_취소하면_커서를_버리고_git_이_끝난다() {
    let (_tmp, dir) = big_repo();
    let dir = &dir;
    let svc = Arc::new(LogService::new(mirror_of(&[("a", dir)])));
    let exec = Arc::new(local());
    let mut aborted = 0;
    #[cfg(windows)]
    let mut pids: Vec<u32> = Vec::new();
    for round in 0..6 {
        let tok = CancellationToken::new();
        let (s, e, t, k) = (svc.clone(), exec.clone(), target(dir), tok.clone());
        let task = tokio::spawn(async move { s.log("a", &e, &t, &req(0, 5000), &k).await });
        // git 이 뜬 뒤에 취소한다(그 git 의 pid 를 기억해 두었다가 끝났는지 본다). git 은 읽기 태스크가 락 밖에서 띄우므로
        // 커서가 생긴 뒤 pid 가 보이기까지 시간이 걸린다(부하가 있으면 CreateProcess 가 수백 ms)
        #[cfg(windows)]
        {
            let end = Instant::now() + Duration::from_secs(10);
            while Instant::now() < end {
                if let Some(p) = svc.cursor_pids().first() {
                    pids.push(*p);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(round * 3)).await;
        tok.cancel();
        match task.await.unwrap() {
            Err(LogError::Exec(e)) => {
                assert_eq!(e.envelope_code(), "ABORTED");
                aborted += 1;
                // 읽는 도중 취소되면 커서 위치를 신뢰할 수 없으므로 버려진다
                assert_eq!(svc.cursor_count(), 0, "round {round}");
            }
            Ok(page) => assert_eq!(page.commits.len(), 4000),
            Err(e) => panic!("{e}"),
        }
    }
    eprintln!("[log] 취소 경로 {aborted}/6 회 실행");
    svc.close_all();
    assert_eq!(svc.cursor_count(), 0);
    // 취소로 버린 커서든 끝까지 읽은 커서든 git 프로세스는 남지 않는다
    #[cfg(windows)]
    {
        assert!(!pids.is_empty());
        assert_tree_dies(&pids).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 이미_취소된_요청은_커서를_만들지_않는다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let svc = LogService::new(mirror_of(&[("a", dir)]));
    let tok = CancellationToken::new();
    tok.cancel();
    let r = svc.log("a", &local(), &target(dir), &req(0, 10), &tok).await;
    assert_eq!(r.unwrap_err().envelope_code(), "ABORTED");
    assert_eq!(svc.cursor_count(), 0);
}

// ── 라우터(repos:watch · 미러 · 사이드카 알림) ───────────────────────

fn envelope(r: Routed) -> Value {
    match r {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("Rust 경로를 타야 하는데 사이드카로 갔다"),
    }
}

async fn log_page(router: &Router, id: &str, skip: u64, count: usize) -> Value {
    envelope(router.route("git:log", &json!([id, { "skip": skip, "count": count }, format!("r-{id}-{skip}")])).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repos_watch_는_사이드카로_넘기기_전에_다른_저장소의_커서를_닫는다() {
    let tmp = TempDir::new("watch");
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    linear_repo(&a, 2500);
    linear_repo(&b, 2500);
    // 프로세스 목록을 읽는 동안(느릴 수 있다) 유휴 정리가 먼저 닫지 않게 유휴 시간을 길게 둔다
    let router = Router::new(true, false, mirror_of(&[("a", &a), ("b", &b)])).with_log_timing(Duration::from_secs(3600), Duration::from_secs(3600));
    assert_eq!(log_page(&router, "a", 0, 50).await["ok"], true);
    assert_eq!(router.log_cursor_count(), 1);
    #[cfg(windows)]
    let tree = live_tree(router.log_cursor_pids()[0]);
    // 같은 저장소를 보는 중이면 그대로 둔다
    assert!(matches!(router.route("repos:watch", &json!(["a"])).await, Routed::Sidecar));
    assert_eq!(router.log_cursor_count(), 1);
    // 다른 저장소로 옮기면 닫는다 — 응답은 여전히 사이드카의 몫이다
    assert!(matches!(router.route("repos:watch", &json!(["b"])).await, Routed::Sidecar));
    assert_eq!(router.log_cursor_count(), 0);
    #[cfg(windows)]
    assert_tree_dies(&tree).await;
    // 닫은 뒤에도 이어 읽는다(앵커)
    let p = log_page(&router, "a", 50, 50).await;
    assert_eq!(p["ok"], true);
    assert_eq!(p["data"]["commits"].as_array().unwrap().len(), 50);
    // null(활성 저장소 없음)이나 모양이 틀린 인자는 모두 닫는다
    assert!(matches!(router.route("repos:watch", &json!([null])).await, Routed::Sidecar));
    assert_eq!(router.log_cursor_count(), 0);
    log_page(&router, "a", 100, 10).await;
    assert!(matches!(router.route("repos:watch", &json!([])).await, Routed::Sidecar));
    assert_eq!(router.log_cursor_count(), 0);
    // 스위치가 꺼져 있어도 사이드카로 간다
    let off = Router::new(false, false, RepoMirror::default());
    assert!(matches!(off.route("repos:watch", &json!(["a"])).await, Routed::Sidecar));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 미러에서_사라지거나_바뀐_저장소의_커서와_앵커를_닫는다() {
    let tmp = TempDir::new("mirror");
    let (a, b) = (tmp.join("a"), tmp.join("b"));
    linear_repo(&a, 300);
    linear_repo(&b, 300);
    let mirror = mirror_of(&[("a", &a), ("b", &b)]);
    let router = Router::new(true, false, mirror.clone());
    log_page(&router, "a", 0, 50).await;
    assert_eq!(router.log_cursor_count(), 1);
    // 변화 없는 미러는 건드리지 않는다
    mirror.apply(2, vec![MirrorRepo { id: "a".into(), kind: "local".into(), path: path_str(&a), distro: None }, MirrorRepo { id: "b".into(), kind: "local".into(), path: path_str(&b), distro: None }]);
    router.on_mirror_changed();
    assert_eq!(router.log_cursor_count(), 1);
    // 저장소 a 가 지워졌다
    mirror.apply(3, vec![MirrorRepo { id: "b".into(), kind: "local".into(), path: path_str(&b), distro: None }]);
    router.on_mirror_changed();
    assert_eq!(router.log_cursor_count(), 0);
    // 같은 id 로 다른 경로가 들어와도(지우고 다시 추가) 옛 커서로 읽지 않는다
    mirror.apply(4, vec![MirrorRepo { id: "b".into(), kind: "local".into(), path: path_str(&b), distro: None }, MirrorRepo { id: "a".into(), kind: "local".into(), path: path_str(&a), distro: None }]);
    log_page(&router, "b", 0, 10).await;
    assert_eq!(router.log_cursor_count(), 1);
    mirror.apply(5, vec![MirrorRepo { id: "b".into(), kind: "local".into(), path: path_str(&a), distro: None }]);
    router.on_mirror_changed();
    assert_eq!(router.log_cursor_count(), 0);
    // 종료하면 전부 닫힌다
    log_page(&router, "b", 0, 10).await;
    assert_eq!(router.log_cursor_count(), 1);
    router.shutdown();
    assert_eq!(router.log_cursor_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 사이드카_알림은_첫_페이지와_이력_변경에만_간다() {
    let tmp = TempDir::new("notes");
    let dir = tmp.join("branchy");
    fixture_repo(&dir, "branchy");
    let router = Router::new(true, false, mirror_of(&[("a", &dir)]));
    let notes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = notes.clone();
    router.set_note_sink(Arc::new(move |op: NoteOp, id: &str| sink.lock().unwrap().push((op.as_str().to_string(), id.to_string()))));
    let take = || std::mem::take(&mut *notes.lock().unwrap());
    let first = || vec![("logFirstPage".to_string(), "a".to_string())];

    // skip 0 이면 첫 페이지, 이어 읽기에는 없다
    log_page(&router, "a", 0, 5).await;
    assert_eq!(take(), first());
    log_page(&router, "a", 5, 5).await;
    assert!(take().is_empty());
    // 앵커가 가리키는 위치(10)에서 이어 읽는 요청이 STALE 을 만든다
    // 검증 오류는 알리지 않는다
    envelope(router.route("git:log", &json!(["a", { "skip": 0, "count": "x" }])).await);
    assert!(take().is_empty());
    // 앞쪽 이력이 바뀌면 STALE — logStale 이 가고, 그 요청은 skip 0 이 아니라 첫 페이지 알림이 없다
    envelope(router.route("git:log", &json!(["a", { "skip": 0, "count": 1, "scope": { "solo": ["refs/tags/v1.0"] } }])).await);
    take();
    assert!(raw_git(&dir).args(["update-ref", "-d", "refs/heads/topic"]).status().unwrap().success());
    let env = log_page(&router, "a", 10, 5).await;
    assert_eq!((env["ok"].as_bool(), env["code"].as_str(), env["error"].as_str()), (Some(false), Some("STALE"), Some("commit history changed while paging")));
    assert_eq!(take(), vec![("logStale".to_string(), "a".to_string())]);
    // 처음부터 다시 읽으면 다시 첫 페이지
    log_page(&router, "a", 0, 5).await;
    assert_eq!(take(), first());
    // git 이 실패해도(저장소가 사라졌다) 첫 페이지 알림은 간다 — 오류 화면에서 다른 조회가 막히면 안 된다
    let gone = Router::new(true, false, mirror_of(&[("g", &tmp.join("nope"))]));
    let sink = notes.clone();
    gone.set_note_sink(Arc::new(move |op: NoteOp, id: &str| sink.lock().unwrap().push((op.as_str().to_string(), id.to_string()))));
    let env = log_page(&gone, "g", 0, 5).await;
    assert_eq!(env["code"], "GIT");
    assert_eq!(take(), vec![("logFirstPage".to_string(), "g".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 이미_끝까지_읽은_커서는_다시_열어_그_뒤에_생긴_커밋을_본다() {
    let tmp = TempDir::new("grow");
    let dir = tmp.join("r");
    init(&dir);
    let router = Router::new(true, false, mirror_of(&[("a", &dir)]));
    assert_eq!(log_page(&router, "a", 0, 50).await["data"]["commits"].as_array().unwrap().len(), 0);
    let g = |args: &[&str]| assert!(raw_git(&dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).status().unwrap().success(), "{args:?}");
    g(&["commit", "-q", "--allow-empty", "-m", "first"]);
    let page = log_page(&router, "a", 0, 50).await;
    assert_eq!(page["data"]["commits"][0]["subject"], "first");
}

/// `{"skip":1e300,"count":1}` — skip 은 u64 로 포화하고 위치 예약(`next_position`)의 덧셈·뺄셈도 포화해, debug 빌드에서 패닉하지 않는다.
/// git 이 그 값을 받아 주지 않아 TS 처럼 GIT 오류로 끝난다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 아주_큰_skip_도_패닉하지_않고_결정적으로_끝난다() {
    let (_tmp, dir) = shared_repo();
    let dir = &dir;
    let svc = LogService::new(mirror_of(&[("a", dir)]));
    let (exec, t, tok) = (local(), target(dir), CancellationToken::new());
    for skip in [json!(1e300), json!(u64::MAX as f64), json!(9_007_199_254_740_992.0_f64)] {
        let r = parse_query(Some(&json!({ "skip": skip, "count": 1 }))).unwrap();
        let out = svc.log("a", &exec, &t, &r, &tok).await;
        // git 이 정수로 받아 주지 않는다("not an integer", 128) — TS 도 같은 오류다
        match out {
            Err(LogError::Exec(e)) => assert_eq!(e.envelope_code(), "GIT", "{skip}"),
            other => panic!("{skip}: {other:?}"),
        }
    }
    // 이어서 보통 요청이 정상으로 돌아온다
    assert_eq!(svc.log("a", &exec, &t, &req(0, 5), &tok).await.unwrap().commits.len(), 5);
}
