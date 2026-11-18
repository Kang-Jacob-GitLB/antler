//! 저장소 감시의 라우팅·통합 시험(Phase 6a PR 7b): 실제 git 저장소와 실제 파일 변화(notify), 127.0.0.1 의 프로세스 안 SSH 서버(폴링·포커스), 소유 판정,
//! 정지·종료 뒤 notify 핸들 0. 상태 기계(디바운스·최소 간격·포커스 배율)는 `src/git/watcher.rs` 의 멈춘 시계 시험이 맡는다.
//!
//! 파일 감시·실제 소켓 시험은 타이밍에 약하다 — 고정 `sleep` 으로 가정하지 않고 **조건 + 넉넉한 상한(20초)** 으로 기다린다(`wait_for`).
//! 부정 단정("알리지 않는다")은 다음 양성 이벤트 뒤의 **정확한 알림 수**로 본다. 외부 접속·사용자의 `~/.ssh`·agent 는 쓰지 않는다(`tests/ssh/`).

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use antler_lib::git::exec::ssh::SshPool;
use antler_lib::git::fs_watch::NotifyDirs;
use antler_lib::git::watcher::{DirWatch, PollTimes, WatchEvent};
use antler_lib::mirror::{MirrorRepo, RepoMirror, Resolved};
use antler_lib::router::{rust_fetch_kinds, Routed, Router};
use serde_json::{json, Value};

#[path = "ssh/common.rs"]
mod common;
#[path = "ssh/server.rs"]
mod server;

use common::{git, known_hosts_line, make_repo, wait_for, Home, TempDir};
use server::{new_key, Opts, TestServer};

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
}

fn read_src(rel: &str) -> String {
    // CI(Windows)의 체크아웃은 CRLF 일 수 있다 — 소스 검사는 LF 기준으로 본다
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace("\r\n", "\n")
}

type Changes = Arc<Mutex<Vec<String>>>;

fn count(changes: &Changes) -> usize {
    changes.lock().unwrap_or_else(PoisonError::into_inner).len()
}

fn seen(changes: &Changes) -> Vec<String> {
    changes.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn local_repo(id: &str, dir: &Path) -> MirrorRepo {
    MirrorRepo { id: id.into(), kind: "local".into(), path: dir.to_string_lossy().into_owned(), distro: None, ssh_host: None }
}

fn mirror_of(repos: Vec<MirrorRepo>) -> RepoMirror {
    let m = RepoMirror::default();
    m.apply(1, repos);
    m
}

/// 이벤트원은 notify(실제)이고 살아 있는 수를 시험이 읽는다. `repo:changed` 싱크는 수집기다
fn watched(router: Router, dirs: &NotifyDirs) -> (Router, Changes) {
    let router = router.with_dir_watch(Arc::new(dirs.clone()));
    let changes: Changes = Arc::default();
    let sink = changes.clone();
    router.set_change_sink(Arc::new(move |id| sink.lock().unwrap_or_else(PoisonError::into_inner).push(id.to_string())));
    (router, changes)
}

fn local_router(mirror: &RepoMirror, dirs: &NotifyDirs) -> (Router, Changes) {
    watched(Router::new(true, false, mirror.clone()), dirs)
}

fn is_sidecar(r: &Routed) -> bool {
    matches!(r, Routed::Sidecar)
}

/// 저장소를 활성화한다(`repos:watch` — 응답은 사이드카가 하므로 `Sidecar`) 그리고 첫 로그 페이지를 읽어 첫 페이지 문을 연다(기준 시그니처가 바로 잡히게)
async fn activate(router: &Router, id: &str) {
    assert!(is_sidecar(&router.route("repos:watch", &json!([id])).await));
    let env = match router.route("git:log", &json!([id, { "skip": 0, "count": 5 }])).await {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("log 는 Rust 경로여야 한다"),
    };
    assert_eq!(env["ok"], true, "{env}");
}

async fn activity_total(router: &Router, id: &str) -> u64 {
    match router.route("git:activity", &json!([id])).await {
        Routed::Done(v) => {
            assert_eq!(v["ok"], true, "{v}");
            v["data"]["total"].as_u64().unwrap()
        }
        Routed::Sidecar => panic!("activity 는 Rust 경로여야 한다"),
    }
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["commit", "-q", "--allow-empty", "-m", msg]);
}

// ── 로컬: 실제 git · 실제 파일 변화 ────────────────────────────

/// TS `repoWatcher.test.ts` 의 "외부에서 커밋·브랜치를 만들면 알리고, .git 안의 다른 변화로는 알리지 않는다" 이식.
/// 부정은 정확한 알림 수로 본다(설정 변경이 끼어도 커밋·브랜치 두 번 + 이후 한 번뿐이다). 멈춘 뒤에는 notify 스레드가 0 이다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 외부에서_커밋_브랜치를_만들면_알리고_git_안의_다른_변화로는_알리지_않는다() {
    let repo = make_repo(0);
    let dirs = NotifyDirs::new();
    let (router, changes) = local_router(&mirror_of(vec![local_repo("a", repo.path())]), &dirs);
    activate(&router, "a").await;
    // 기준 시그니처를 잡은 뒤에야 디렉터리를 연다 — 감시가 열린 것을 보고 변화를 준다
    wait_for("감시 디렉터리 열림", || dirs.live() == 1).await;
    assert_eq!(router.watching().as_deref(), Some("a"));

    // ref 가 바뀌지 않는 변화(설정 파일 수정)는 알리지 않는다
    git(repo.path(), &["config", "antler.test", "1"]);
    commit(repo.path(), "second");
    wait_for("커밋 알림", || count(&changes) >= 1).await;
    assert_eq!(seen(&changes), ["a"]);

    git(repo.path(), &["branch", "feature"]);
    wait_for("브랜치 알림", || count(&changes) >= 2).await;

    // 다시 설정만 바꾸고 커밋: 설정 변경은 알림을 더하지 않으므로 정확히 3 이다
    git(repo.path(), &["config", "antler.test", "2"]);
    commit(repo.path(), "third");
    wait_for("세 번째 알림", || count(&changes) >= 3).await;
    assert_eq!(seen(&changes), ["a", "a", "a"], "커밋·브랜치·커밋 세 번뿐이다(.git 안의 다른 변화로는 알리지 않는다)");

    // 감시를 멈추면(다른 저장소 없이 null) 감시자도 notify 핸들·스레드도 없다
    assert!(is_sidecar(&router.route("repos:watch", &json!([Value::Null])).await));
    assert_eq!(router.watching(), None);
    wait_for("notify 스레드 0", || dirs.live() == 0).await;
    commit(repo.path(), "after-stop");
    assert_eq!(count(&changes), 3, "멈춘 뒤에는 알리지 않는다");
}

/// 감시가 쓰는 읽기 전용 git 조회(`for-each-ref`·`symbolic-ref`·`rev-parse`)가 `.git` 에 확인을 부르는 이벤트를 만들지 않는다 — 만들면 확인이 확인을 부르는 고리가 된다(inotify 는 열기·쓰기 닫기를 알린다).
/// 표식 파일을 쓴 뒤 그 이벤트가 올 때까지 기다려(이벤트는 순서대로 온다) 그 앞에 확인을 부르는 이벤트가 없음을 본다. Windows 백엔드는 접근 이벤트를 내지 않으므로 이 시험은 Linux 에서 더 의미가 크다
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 읽기_전용_git_조회는_감시_이벤트를_만들지_않는다() {
    let repo = make_repo(0);
    let git_dir = repo.path().join(".git");
    let dirs = NotifyDirs::new();
    let events: Arc<Mutex<Vec<WatchEvent>>> = Arc::default();
    let sink = events.clone();
    let handle = dirs.watch(&git_dir.to_string_lossy(), Arc::new(move |e| sink.lock().unwrap_or_else(PoisonError::into_inner).push(e))).await.expect("감시 등록");
    git(repo.path(), &["for-each-ref", "--format=%(objectname) %(refname)"]);
    git(repo.path(), &["symbolic-ref", "--quiet", "HEAD"]);
    git(repo.path(), &["rev-parse", "--verify", "--quiet", "HEAD"]);
    fs::write(git_dir.join("refs").join("heads").join("marker"), "0000000000000000000000000000000000000000\n").unwrap();
    let rel = |e: &WatchEvent| e.path.clone().unwrap_or_default().replace('\\', "/");
    wait_for("표식 이벤트", || events.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|e| rel(e).ends_with("refs/heads/marker"))).await;
    let got = events.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let marker = got.iter().position(|e| rel(e).ends_with("refs/heads/marker")).unwrap();
    // 센 것에서 뺀다: ① 표식 파일을 만들 때 그 부모 디렉터리(`refs`·`refs/heads`)의 변경 알림이 먼저 올 수 있다(표식 자신의 일). ② `objects` 디렉터리 자체의 알림 —
    // 직전 커밋이 만든 객체 디렉터리의 수정 시각 갱신이 NTFS 에서 늦게 오는 것이 관찰됐다(TS 의 `startsWith('objects/')` 도 이것은 거르지 않는다 — 확인 한 번이 더 돌 뿐 시그니처가 같아 알리지 않는다)
    let before: Vec<String> = got[..marker]
        .iter()
        .filter(|e| e.should_kick())
        .map(rel)
        .filter(|r| r != "objects" && (r.is_empty() || !"refs/heads/marker".starts_with(r.as_str())))
        .collect();
    assert!(before.is_empty(), "읽기 전용 git 조회가 확인을 부르는 이벤트를 만들었다: {before:?}");
    drop(handle);
    wait_for("notify 스레드 0", || dirs.live() == 0).await;
}

/// 감시가 확인마다 시그니처 기억을 갱신하므로, 알림(`logStale`·`fetchChanged`) 없이도 활동이 바뀐 이력을 센다(재사용을 켠 것이 낡은 값을 만들지 않는다)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 감시가_변화를_알면_활동은_알림_없이도_새로_센다() {
    let repo = make_repo(0);
    let dirs = NotifyDirs::new();
    let (router, changes) = local_router(&mirror_of(vec![local_repo("a", repo.path())]), &dirs);
    activate(&router, "a").await;
    wait_for("감시 디렉터리 열림", || dirs.live() == 1).await;
    assert_eq!(activity_total(&router, "a").await, 1);
    let before = router.repo_state().known_signature("a");
    assert!(before.is_some());
    commit(repo.path(), "watched");
    wait_for("커밋 알림", || count(&changes) >= 1).await;
    // 알림은 기억을 갱신한 뒤에 나간다(확인이 시그니처를 기록하고서 알린다)
    assert_ne!(router.repo_state().known_signature("a"), before, "감시가 기억한 시그니처를 갱신하지 않았다");
    assert_eq!(activity_total(&router, "a").await, 2);
    router.shutdown();
    wait_for("notify 스레드 0", || dirs.live() == 0).await;
}

/// 활성 저장소를 바꾸면 이전 감시는 닫히고 새 저장소만 알린다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활성_저장소를_바꾸면_이전_감시는_닫히고_새_저장소의_변화만_알린다() {
    let (a, b) = (make_repo(0), make_repo(0));
    let dirs = NotifyDirs::new();
    let (router, changes) = local_router(&mirror_of(vec![local_repo("a", a.path()), local_repo("b", b.path())]), &dirs);
    activate(&router, "a").await;
    wait_for("a 감시", || dirs.live() == 1).await;
    activate(&router, "b").await;
    assert_eq!(router.watching().as_deref(), Some("b"));
    // 이전 핸들이 닫히고 새 핸들이 열려 하나다 — b 의 기준을 잡을 때까지 기다린다
    wait_for("b 기준", || router.repo_state().known_signature("b").is_some() && dirs.live() == 1).await;
    commit(a.path(), "a-ignored");
    commit(b.path(), "b-seen");
    wait_for("b 알림", || count(&changes) >= 1).await;
    assert_eq!(seen(&changes), ["b"], "이전 저장소의 변화는 알리지 않는다");
    router.shutdown();
    wait_for("notify 스레드 0", || dirs.live() == 0).await;
}

// ── 소유 판정 ────────────────────────────────────────────

/// Rust 는 fetch 와 같은 술어(종류 대조)로 맡는 저장소만 감시한다. github·미러에 없는 id·검사에 걸린 경로·ssh(풀 없음)는 감시하지 않고 이전 감시도 멈춘다.
/// 스위치가 꺼져 있으면(`ANTLER_RUST_GIT=0`) 감시자가 만들어지지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_가_맡는_저장소만_감시하고_스위치가_꺼지면_감시기가_없다() {
    let repo = make_repo(0);
    let github = MirrorRepo { id: "g".into(), kind: "github".into(), path: repo.path().to_string_lossy().into_owned(), distro: None, ssh_host: None };
    let ssh = MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv/r".into(), distro: None, ssh_host: Some("h".into()) };
    let unc = MirrorRepo { id: "u".into(), kind: "local".into(), path: "\\\\host\\share\\r".into(), distro: None, ssh_host: None };
    let mirror = mirror_of(vec![local_repo("a", repo.path()), github, ssh, unc]);
    let dirs = NotifyDirs::new();
    let (router, _changes) = local_router(&mirror, &dirs);

    assert!(is_sidecar(&router.route("repos:watch", &json!(["a"])).await));
    assert_eq!(router.watching().as_deref(), Some("a"));
    wait_for("a 감시 디렉터리", || dirs.live() == 1).await;
    // 사이드카가 맡는 저장소로 바꾸면 Rust 감시는 멈춘다(두 쪽이 동시에 감시하지 않는다)
    for id in ["g", "s", "u", "nope"] {
        assert!(is_sidecar(&router.route("repos:watch", &json!(["a"])).await));
        assert_eq!(router.watching().as_deref(), Some("a"), "{id}");
        assert!(is_sidecar(&router.route("repos:watch", &json!([id])).await));
        assert_eq!(router.watching(), None, "{id} 는 Rust 가 감시하지 않는다");
        wait_for("이전 감시의 notify 핸들 닫힘", || dirs.live() == 0).await;
    }
    // 인자가 저장소 id 가 아니면 멈춘다
    assert!(is_sidecar(&router.route("repos:watch", &json!(["a"])).await));
    assert!(is_sidecar(&router.route("repos:watch", &json!([5])).await));
    assert_eq!(router.watching(), None);
    router.shutdown();

    // 스위치 끔: 어떤 저장소도 감시하지 않는다(사이드카가 전부 감시한다 — env 가 없다)
    let off_dirs = NotifyDirs::new();
    let (off, _c) = watched(Router::new(false, false, mirror), &off_dirs);
    assert!(is_sidecar(&off.route("repos:watch", &json!(["a"])).await));
    assert_eq!(off.watching(), None);
    assert_eq!(off_dirs.live(), 0);
}

/// Rust 가 감시를 맡는 종류 = 사이드카 env(`ANTLER_RUST_FETCH_KINDS`)의 종류 — 같은 한 술어(`rust_fetch_kinds`)다. 이중 감시도 무감시도 없다
#[test]
fn 감시_소유는_사이드카_env_의_종류와_같은_술어다() {
    let kinds = |ssh: bool, github: bool| -> Vec<&'static str> { rust_fetch_kinds(ssh, github).split(',').collect() };
    for (ssh_active, github_active) in [(false, false), (true, false), (false, true), (true, true)] {
        let mirror = RepoMirror::default();
        mirror.set_ssh(ssh_active);
        mirror.set_github(github_active);
        let repos: Vec<MirrorRepo> = ["local", "wsl", "ssh", "github"]
            .iter()
            .map(|k| MirrorRepo {
                id: (*k).into(),
                kind: (*k).into(),
                path: "/p".into(),
                distro: (*k == "wsl").then(|| "Ubuntu".to_string()),
                ssh_host: (*k == "ssh").then(|| "h".to_string()),
            })
            .collect();
        mirror.apply(1, repos);
        for kind in ["local", "wsl", "ssh", "github"] {
            if kind == "wsl" && !cfg!(windows) {
                continue; // WSL 은 Windows 에서만 연다(미러가 그 밖의 OS 에서는 사이드카로 보낸다)
            }
            let rust_watches = matches!(mirror.resolve_fetch(kind), Resolved::Rust(_));
            assert_eq!(rust_watches, kinds(ssh_active, github_active).contains(&kind), "{kind} ssh_active={ssh_active} github_active={github_active}");
        }
    }
    // 사이드카는 같은 술어를 감시에도 쓴다(소스 검사 — 둘에 같은 `ownsRepo` 를 준다)
    let ts = read_src("../src/sidecar/index.ts");
    assert!(ts.contains("const ownsRepo = fetchOwner(() => config.get(), parseRustFetchKinds(process.env['ANTLER_RUST_FETCH_KINDS']))"));
    assert!(ts.contains("new RepoWatcher(repos, (repoId) => emit('repo:changed', { repoId }), ownsRepo)"));
    assert!(ts.contains("  noHooksDir,\n  ownsRepo\n)"), "FetchService 도 같은 술어다");
    assert_eq!(ts.matches("fetchOwner(").count(), 1, "소유 판정은 한 곳에서 만든다");
}

#[test]
fn 앱은_변화_싱크를_사이드카_기동_전에_달고_포커스를_감시에_알린다() {
    let lib = read_src("src/lib.rs");
    let sink = lib.find("router.set_change_sink(").expect("변화 싱크");
    let manage = lib.find("app.manage(router)").expect("라우터 등록");
    assert!(sink < manage);
    // 포커스는 얻을 때도 잃을 때도 알린다(폴링 배율). 런타임 밖 스레드에서 불러도 되는 호출이라 `in_runtime_of` 밖이다
    assert_eq!(lib.matches("r.watch_focus(*focused)").count(), 1);
    // (설정 알림의 미러·fetch 설정 반영 둘(Phase 6b PR 9), 창 포커스의 fetch, 그리고 GitHub 로그인 변경 콜백 — Phase 6b PR 7)
    assert_eq!(lib.matches("in_runtime_of(").count(), 4);
    // 감시 이벤트는 사이드카의 `repo:changed` 와 같은 모양이다
    assert!(read_src("src/fetch_host.rs").contains("evt(\"repo:changed\", json!({ \"repoId\": repo_id }))"));
}

// ── 미러 변화: 제거·대상 변경·종료 ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 저장소가_제거되면_감시를_멈추고_대상이_바뀌면_다시_시작한다() {
    let (one, two) = (make_repo(0), make_repo(0));
    // 두 저장소의 시그니처가 다르게(같은 초에 같은 커밋이 만들어질 수 있다)
    commit(two.path(), "extra");
    let mirror = mirror_of(vec![local_repo("a", one.path())]);
    let dirs = NotifyDirs::new();
    let (router, changes) = local_router(&mirror, &dirs);
    activate(&router, "a").await;
    wait_for("a 감시", || dirs.live() == 1).await;
    let first = router.repo_state().known_signature("a");
    assert!(first.is_some());

    // 같은 id 가 다른 경로를 가리키면 다시 시작한다 — 새 경로의 기준을 잡고 그 저장소의 변화를 알린다
    assert!(mirror.apply(2, vec![local_repo("a", two.path())]));
    router.on_mirror_changed();
    assert_eq!(router.watching().as_deref(), Some("a"));
    wait_for("새 경로의 기준", || router.repo_state().known_signature("a") != first && dirs.live() == 1).await;
    commit(two.path(), "in-new-path");
    wait_for("새 경로의 알림", || count(&changes) >= 1).await;
    assert_eq!(seen(&changes), ["a"]);

    // 미러에 그대로이고 대상이 같으면 건드리지 않는다
    assert!(mirror.apply(3, vec![local_repo("a", two.path())]));
    router.on_mirror_changed();
    assert_eq!(router.watching().as_deref(), Some("a"));
    assert_eq!(dirs.live(), 1);

    // 저장소가 제거되면 `unwatch_if` 로 멈춘다
    assert!(mirror.apply(4, vec![]));
    router.on_mirror_changed();
    assert_eq!(router.watching(), None);
    wait_for("제거된 저장소의 notify 핸들 닫힘", || dirs.live() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 종료하면_감시자도_notify_스레드도_남지_않고_이후_변화는_알리지_않는다() {
    let repo = make_repo(0);
    let dirs = NotifyDirs::new();
    let (router, changes) = local_router(&mirror_of(vec![local_repo("a", repo.path())]), &dirs);
    activate(&router, "a").await;
    wait_for("감시 디렉터리 열림", || dirs.live() == 1).await;
    router.shutdown();
    assert_eq!(router.watching(), None);
    wait_for("notify 스레드 0", || dirs.live() == 0).await;
    commit(repo.path(), "after-shutdown");
    assert_eq!(count(&changes), 0);
    // 종료를 또 불러도 안전하다
    router.shutdown();
    // 라우터를 버려도 같다
    let repo2 = make_repo(0);
    let dirs2 = NotifyDirs::new();
    let (router2, _c) = local_router(&mirror_of(vec![local_repo("b", repo2.path())]), &dirs2);
    activate(&router2, "b").await;
    wait_for("b 감시", || dirs2.live() == 1).await;
    drop(router2);
    wait_for("라우터를 버리면 감시도 멈춘다", || dirs2.live() == 0).await;
}

// ── SSH: 루프백 서버, 폴링만(notify 없음) ──────────────────────

struct Rig {
    server: TestServer,
    pool: Arc<SshPool>,
    repo: TempDir,
    _home: Arc<Home>,
}

impl Rig {
    async fn new() -> Rig {
        let client = new_key();
        let server = TestServer::start(Opts::new(vec![client.public_key().clone()])).await;
        let home = Arc::new(Home::new(&client));
        let block = format!("Host a\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {}\n  IdentitiesOnly yes\n", server.port, home.key_for_config());
        home.write_config(&block);
        home.write_known_hosts(&known_hosts_line(server.port, &server.host_key));
        let h = home.clone();
        let pool = SshPool::new(move |host| h.executor(host));
        Rig { server, pool, repo: make_repo(0), _home: home }
    }

    fn mirror(&self) -> RepoMirror {
        mirror_of(vec![MirrorRepo { id: "s".into(), kind: "ssh".into(), path: self.repo.path().to_string_lossy().into_owned(), distro: None, ssh_host: Some("a".into()) }])
    }
}

/// SSH 저장소는 notify 없이 폴링으로 감시한다. 주기를 짧게 주입해(멈춘 시계는 실제 소켓 대기 중에 자동 전진해 제한 시간을 헛발화시킨다) 외부 커밋을 알리고,
/// 감시·조회가 **호스트당 연결 하나**를 나눠 쓰는지 본다. 저장소가 제거되면 감시가 멈추고 연결이 닫힌다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_저장소는_폴링으로_감시하고_연결은_호스트당_하나다() {
    let rig = Rig::new().await;
    let mirror = rig.mirror();
    let dirs = NotifyDirs::new();
    let polls = PollTimes { local: Duration::from_secs(60), wsl: Duration::from_secs(4), ssh: Duration::from_millis(300) };
    let router = Router::new(true, false, mirror.clone()).with_watch_poll(polls);
    router.set_ssh_pool(rig.pool.clone());
    let (router, changes) = watched(router, &dirs);
    assert!(router.ssh_active());
    activate(&router, "s").await;
    wait_for("기준 시그니처", || router.repo_state().known_signature("s").is_some()).await;
    assert_eq!(router.watching().as_deref(), Some("s"));
    commit(rig.repo.path(), "remote-commit");
    wait_for("폴링이 변화를 알림", || count(&changes) >= 1).await;
    assert_eq!(seen(&changes), ["s"]);
    assert_eq!(dirs.live(), 0, "SSH 는 notify 를 열지 않는다(폴링만)");
    assert_eq!(rig.server.connections(), 1, "감시·조회가 연결 하나를 나눠 써야 한다");
    assert_eq!(rig.pool.hosts(), ["a"]);

    // 저장소가 제거되면 감시가 멈추고 연결도 닫힌다(감시가 지워진 호스트에 다시 접속하지 않는다)
    assert!(mirror.apply(2, vec![]));
    router.on_mirror_changed();
    assert_eq!(router.watching(), None);
    wait_for("연결 닫힘", || rig.server.open_connections() == 0).await;
    router.shutdown();
}

/// 포커스를 얻으면 폴링 주기(30초)를 기다리지 않고 바로 확인한다 — 다른 도구에서 커밋하고 돌아온 경우
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_저장소는_포커스를_얻으면_바로_확인한다() {
    let rig = Rig::new().await;
    let dirs = NotifyDirs::new();
    let router = Router::new(true, false, rig.mirror());
    router.set_ssh_pool(rig.pool.clone());
    let (router, changes) = watched(router, &dirs);
    activate(&router, "s").await;
    wait_for("기준 시그니처", || router.repo_state().known_signature("s").is_some()).await;
    commit(rig.repo.path(), "while-away");
    // 기준을 잡는 중의 포커스는 버려지므로(TS 와 같다) 알림이 올 때까지 신호를 되풀이한다 — 확인은 진행 중이면 건너뛰므로 되풀이해도 안전하다
    router.watch_focus(false);
    let mut tries = 0;
    while count(&changes) == 0 {
        router.watch_focus(true);
        tokio::time::sleep(Duration::from_millis(100)).await;
        tries += 1;
        assert!(tries < 200, "포커스 확인이 변화를 알리지 않았다");
    }
    assert_eq!(seen(&changes), ["s"]);
    assert_eq!(rig.server.connections(), 1);
    router.shutdown();
}
