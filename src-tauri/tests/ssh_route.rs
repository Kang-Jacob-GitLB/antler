//! ssh 저장소 라우팅(Phase 5 PR 3) 통합 테스트: 미러의 `sshHost` 검사, 스위치(`ANTLER_RUST_SSH`·`ANTLER_RUST_GIT`), 쓰지 않는 호스트의 연결 정리,
//! 호스트당 연결 하나, 앱 배선(소스 검사). TS 골든과의 비교는 `channels.rs` 의 `*_ssh_에서도_*` 시험이 맡는다.
//!
//! **127.0.0.1 의 프로세스 안 `russh::server` 만 쓴다**(`tests/ssh/server.rs`). 실행기에는 임시 홈과 `AgentSetting::Disabled` 를 주고 사용자의 `~/.ssh`·agent 는 읽지 않는다.
//! 시간 의존은 `wait_for`(조건 + 상한)로 기다린다.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use antler_lib::git::exec::queue::SSH_LIMIT;
use antler_lib::git::exec::ssh::SshPool;
use antler_lib::git::exec::GitExecutor;
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::service::{ApprovalKind, FetchHost, FetchService, HostEvent};
use antler_lib::git::log_service::NoteOp;
use antler_lib::mirror::{MirrorRepo, RepoMirror, Target};
use antler_lib::router::{Routed, Router};
use serde_json::{json, Value};

#[path = "ssh/common.rs"]
mod common;
#[path = "ssh/server.rs"]
mod server;

use common::{known_hosts_line, make_repo, wait_for, Home, TempDir};
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
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 서버 하나 + 별칭 두 개(`a`·`b` — 같은 서버를 가리키지만 호스트 이름이 달라 풀에서 따로 연결된다) + 풀
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
        let block = |alias: &str| {
            format!("Host {alias}\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {}\n  IdentitiesOnly yes\n", server.port, home.key_for_config())
        };
        home.write_config(&format!("{}{}", block("a"), block("b")));
        home.write_known_hosts(&known_hosts_line(server.port, &server.host_key));
        let h = home.clone();
        let pool = SshPool::new(move |host| h.executor(host));
        Rig { server, pool, repo: make_repo(0), _home: home }
    }

    fn path(&self) -> String {
        self.repo.path().to_string_lossy().into_owned()
    }

    fn repo(&self, id: &str, host: &str) -> MirrorRepo {
        MirrorRepo { id: id.into(), kind: "ssh".into(), path: self.path(), distro: None, ssh_host: Some(host.into()) }
    }

    fn router(&self, mirror: &RepoMirror) -> Router {
        let router = Router::new(true, false, mirror.clone());
        router.set_ssh_pool(self.pool.clone());
        router
    }
}

fn envelope(r: Routed) -> Value {
    match r {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("Rust 경로를 타야 하는데 사이드카로 갔다"),
    }
}

/// 저장소의 첫 페이지를 읽어 연결이 살아 있는지 본다
async fn log_ok(router: &Router, id: &str) -> bool {
    let env = envelope(router.route("git:log", &json!([id, { "skip": 0, "count": 5 }])).await);
    env["ok"] == true && env["data"]["commits"].as_array().is_some_and(|c| c.len() == 1)
}

#[test]
fn ssh_큐_상한은_4_이다() {
    assert_eq!(SSH_LIMIT, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 잘못된_ssh_호스트는_연결하지_않고_거부한다() {
    let rig = Rig::new().await;
    let long = "h".repeat(256);
    let bad: Vec<(&str, Option<&str>)> = vec![
        ("space", Some("a b")),
        ("tab", Some("a\tb")),
        ("dash", Some("-oProxyCommand=x")),
        ("dash-after-space", Some("  -x")),
        ("nul", Some("a\0b")),
        ("ctrl", Some("a\u{7}b")),
        ("long", Some(&long)),
        ("empty", Some("")),
        ("blank", Some("   ")),
        ("none", None),
    ];
    let m = RepoMirror::default();
    m.apply(
        1,
        bad.iter()
            .map(|(id, host)| MirrorRepo { id: (*id).into(), kind: "ssh".into(), path: rig.path(), distro: None, ssh_host: host.map(Into::into) })
            .collect(),
    );
    let router = rig.router(&m);
    for (id, _) in &bad {
        for (ch, args) in [
            ("git:commit", json!([id, "abcd1234"])),
            ("git:log", json!([id, { "skip": 0, "count": 5 }])),
            ("git:compare", json!([id, "abcd1234", "abcd1234"])),
            ("git:fileDiff", json!([id, { "hash": "abcd1234", "path": "a.txt" }])),
            ("git:fileHistory", json!([id, { "path": "a.txt", "skip": 0, "count": 1 }])),
            ("git:blame", json!([id, { "path": "a.txt" }])),
            ("git:resolveRev", json!([id, "HEAD"])),
            ("git:tree", json!([id, { "rev": "a".repeat(40), "dir": "" }])),
            ("git:blob", json!([id, { "rev": "a".repeat(40), "path": "a.txt" }])),
            ("git:onRemote", json!([id, "a".repeat(40)])),
            ("git:refs", json!([id])),
            ("git:refsDetails", json!([id])),
            ("git:search", json!([id, "abc"])),
            ("git:activity", json!([id])),
            ("git:repoExtras", json!([id])),
            ("git:lfsPaths", json!([id, { "paths": ["a.txt"] }])),
            ("gh:repo", json!([id])),
            ("git:wipStatus", json!([id])),
            ("git:wipDiff", json!([id, { "area": "unstaged", "path": "a.txt" }])),
        ] {
            let env = envelope(router.route(ch, &args).await);
            assert_eq!((env["ok"].as_bool(), env["code"].as_str(), env["error"].as_str()), (Some(false), Some("ERROR"), Some("잘못된 SSH 호스트입니다")), "{id} {ch}");
        }
    }
    assert_eq!(rig.server.connections(), 0, "검사에 걸린 호스트로는 연결하지 않는다");
    assert!(rig.pool.hosts().is_empty());
    assert_eq!(router.rust_handled(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 호스트_앞뒤_공백은_지우고_그_값으로_연결한다() {
    let rig = Rig::new().await;
    let m = RepoMirror::default();
    m.apply(1, vec![rig.repo("s", "  a\t")]);
    let router = rig.router(&m);
    assert!(log_ok(&router, "s").await);
    assert_eq!(rig.pool.hosts(), ["a"], "검사한 값(공백을 지운 값)이 풀의 키다");
    router.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 스위치가_꺼지면_ssh_는_사이드카이고_연결하지_않는다() {
    let rig = Rig::new().await;
    let m = RepoMirror::default();
    m.apply(1, vec![rig.repo("s", "a")]);
    // ANTLER_RUST_SSH=0
    let off = Router::new(true, false, m.clone()).with_ssh(false);
    off.set_ssh_pool(rig.pool.clone());
    assert!(!off.ssh_active() && off.ssh_pool().is_none());
    // ANTLER_RUST_GIT=0(라우터 전체가 꺼지면 ssh 도 함께 꺼진다)
    let git_off = Router::new(false, false, m.clone());
    git_off.set_ssh_pool(rig.pool.clone());
    assert!(!git_off.ssh_active());
    for r in [&off, &git_off] {
        for ch in ["git:commit", "git:log", "git:compare", "git:fileDiff", "git:fileHistory", "git:blame", "git:resolveRev", "git:tree", "git:blob", "git:onRemote", "git:refs", "git:refsDetails", "git:search", "git:activity", "git:repoExtras", "git:lfsPaths", "gh:repo", "git:wipStatus", "git:wipDiff"] {
            assert!(matches!(r.route(ch, &json!(["s", { "skip": 0, "count": 1 }])).await, Routed::Sidecar), "{ch}");
        }
    }
    assert_eq!(rig.server.connections(), 0);
    assert_eq!((off.rust_handled(), git_off.rust_handled()), (0, 0));
    // 풀이 주입되지 않은 기본 라우터도 같다
    let bare = Router::new(true, false, m);
    assert!(!bare.ssh_active());
    assert!(matches!(bare.route("git:log", &json!(["s", { "skip": 0, "count": 1 }])).await, Routed::Sidecar));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 같은_호스트는_연결_하나를_나눠_쓰고_쓰지_않는_호스트는_닫힌다() {
    let rig = Rig::new().await;
    let m = RepoMirror::default();
    m.apply(1, vec![rig.repo("s1", "a"), rig.repo("s2", "a"), rig.repo("t1", "b")]);
    let router = rig.router(&m);
    assert!(router.ssh_active());
    for id in ["s1", "s2", "t1"] {
        assert!(log_ok(&router, id).await, "{id}");
    }
    // 호스트 a 의 저장소 둘은 연결 하나를 나눠 쓴다(a, b 둘이라 연결은 둘)
    assert_eq!(rig.server.connections(), 2);
    assert_eq!(rig.pool.hosts(), ["a", "b"]);

    // a 의 저장소 하나만 빠지면 a 는 아직 쓰므로 그대로다
    assert!(m.apply(2, vec![rig.repo("s2", "a"), rig.repo("t1", "b")]));
    router.on_mirror_changed();
    assert_eq!(rig.pool.hosts(), ["a", "b"]);
    assert_eq!(rig.server.open_connections(), 2);

    // b 의 마지막 저장소가 빠지면 b 의 연결을 닫고 풀에서도 뺀다
    assert!(m.apply(3, vec![rig.repo("s2", "a")]));
    router.on_mirror_changed();
    assert_eq!(rig.pool.hosts(), ["a"]);
    wait_for("b 연결 닫힘", || rig.server.open_connections() == 1).await;
    // 닫힌 호스트의 저장소는 이제 미러에 없어 사이드카가 처리한다
    assert!(matches!(router.route("git:log", &json!(["t1", { "skip": 0, "count": 5 }])).await, Routed::Sidecar));

    // 같은 호스트를 다시 추가하면 새 연결이 맺어져 동작한다(닫은 실행기는 영구히 거부하므로 풀·큐에서 빼 두지 않으면 계속 실패한다)
    let before = rig.server.connections();
    assert!(m.apply(4, vec![rig.repo("s2", "a"), rig.repo("t2", "b")]));
    router.on_mirror_changed();
    assert!(log_ok(&router, "t2").await, "다시 추가한 호스트");
    assert_eq!(rig.server.connections(), before + 1);
    assert!(log_ok(&router, "s2").await, "계속 쓰던 호스트는 연결을 그대로 쓴다");
    assert_eq!(rig.server.connections(), before + 1);

    // 모든 ssh 저장소가 빠지면 모두 닫힌다
    assert!(m.apply(5, vec![]));
    router.on_mirror_changed();
    assert!(rig.pool.hosts().is_empty());
    wait_for("모든 연결 닫힘", || rig.server.open_connections() == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_호스트가_바뀐_저장소는_옛_호스트_연결을_닫는다() {
    let rig = Rig::new().await;
    let m = RepoMirror::default();
    m.apply(1, vec![rig.repo("s", "a")]);
    let router = rig.router(&m);
    assert!(log_ok(&router, "s").await);
    // 같은 저장소 id 의 호스트가 바뀌었다 — a 는 더 쓰지 않는다
    m.apply(2, vec![rig.repo("s", "b")]);
    router.on_mirror_changed();
    assert_eq!(rig.pool.hosts(), Vec::<String>::new());
    wait_for("a 연결 닫힘", || rig.server.open_connections() == 0).await;
    assert!(log_ok(&router, "s").await);
    assert_eq!(rig.pool.hosts(), ["b"]);
    router.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 종료하면_모든_ssh_연결이_닫힌다() {
    let rig = Rig::new().await;
    let m = RepoMirror::default();
    m.apply(1, vec![rig.repo("s1", "a"), rig.repo("t1", "b")]);
    let router = rig.router(&m);
    assert!(log_ok(&router, "s1").await && log_ok(&router, "t1").await);
    assert_eq!(rig.server.open_connections(), 2);
    router.shutdown();
    assert!(rig.pool.hosts().is_empty());
    wait_for("모든 연결 닫힘", || rig.server.open_connections() == 0).await;
}

// ── ssh fetch(Phase 5 PR 4) ────────────────────────────────

struct NullHost;

impl FetchHost for NullHost {
    fn emit(&self, _event: HostEvent) {}
    async fn confirm(&self, _repo_id: &str, _confirm: &ConfirmBox) -> Option<i64> {
        None
    }
    fn approve(&self, _kind: ApprovalKind, _repo_id: &str, _fingerprint: &str) {}
    fn note(&self, _op: NoteOp, _repo_id: &str) {}
}

/// `origin` 의 복제본(remote 가 origin 의 경로다 — file 전송이라 수동 fetch 만 받는다). 준비용 날 git 이다(제품 경로가 아니다)
#[allow(clippy::disallowed_methods)]
fn clone_of(origin: &TempDir) -> TempDir {
    let dest = TempDir::new("clone");
    let out = std::process::Command::new("git").args(["clone", "-q"]).arg(origin.path()).arg(dest.path()).output().expect("git 실행");
    assert!(out.status.success(), "git clone: {}", String::from_utf8_lossy(&out.stderr));
    dest
}

/// 루프백 서버 너머의 저장소를 Rust 서비스가 수동 fetch 한다. 풀을 라우터와 나눠 쓰므로 호스트당 연결이 하나이고,
/// 호스트가 미러에서 빠지면 연결과 서비스가 쥔 실행기가 함께 정리돼 같은 호스트를 다시 추가한 뒤의 fetch 도 새 연결로 성공한다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_fetch_는_라우터와_같은_연결을_쓰고_호스트를_지웠다_다시_추가해도_성공한다() {
    let rig = Rig::new().await;
    let origin = make_repo(0);
    let clone = clone_of(&origin);
    let mk = || MirrorRepo { id: "s".into(), kind: "ssh".into(), path: clone.path().to_string_lossy().into_owned(), distro: None, ssh_host: Some("a".into()) };
    let m = RepoMirror::default();
    m.apply(1, vec![mk()]);
    let router = rig.router(&m);
    let pool = rig.pool.clone();
    let make_runner = move |t: &Target| match t {
        Target::Ssh { host, .. } => Arc::new(GitExecutor::Ssh(pool.get(host))),
        _ => unreachable!("ssh 저장소만 있다"),
    };
    router.set_fetch(Arc::new(FetchService::new(NullHost, m.clone(), make_runner, None)));
    let fetch = || async { envelope(router.route("git:fetch", &json!(["s", true])).await) };

    let env = fetch().await;
    assert!(env["data"]["error"].is_null() && env["data"]["lastOkAt"].is_u64(), "{env}");
    // 라우터의 git:log 도 같은 연결을 쓴다(호스트당 하나)
    assert!(log_ok_at(&router, "s", 1).await);
    assert_eq!(rig.server.connections(), 1);

    // 호스트가 빠진다 — 연결이 닫히고 풀에서도 빠진다
    assert!(m.apply(2, vec![]));
    router.on_mirror_changed();
    assert!(rig.pool.hosts().is_empty());
    wait_for("연결 닫힘", || rig.server.open_connections() == 0).await;
    // 같은 호스트를 다시 추가하면 새 연결로 fetch 가 성공한다(낡은 실행기의 "이미 종료" 가 아니다)
    assert!(m.apply(3, vec![mk()]));
    router.on_mirror_changed();
    let env = fetch().await;
    assert!(env["data"]["error"].is_null() && env["data"]["lastOkAt"].is_u64(), "{env}");
    assert_eq!(rig.server.connections(), 2);
    router.shutdown();
    wait_for("모든 연결 닫힘", || rig.server.open_connections() == 0).await;
}

/// `log_ok` 의 커밋 수를 정할 수 있는 형태(복제본은 커밋이 1개다)
async fn log_ok_at(router: &Router, id: &str, commits: usize) -> bool {
    let env = envelope(router.route("git:log", &json!([id, { "skip": 0, "count": 5 }])).await);
    env["ok"] == true && env["data"]["commits"].as_array().is_some_and(|c| c.len() == commits)
}

// ── 앱 배선(소스 검사) ─────────────────────────────────────

#[test]
fn 앱은_풀을_fetch_서비스와_사이드카_env_보다_먼저_주입하고_판정은_ssh_active_하나다() {
    let lib = read_src("src/lib.rs");
    let pool = lib.find("router.set_ssh_pool(").expect("풀 주입");
    let guard = lib.find("if router.ssh_enabled()").expect("스위치 확인");
    let fetch = lib.find("router.set_fetch(").expect("fetch 서비스 주입");
    let active = lib.find("let rust_ssh = router.ssh_active();").expect("사이드카 env 값(미러 해석과 같은 판정)");
    let kinds = lib.find("let fetch_kinds = router::sidecar_fetch_kinds(router.enabled(), rust_ssh, rust_github);").expect("fetch 소유 종류도 같은 판정(ssh_active)으로 정한다");
    let manage = lib.find("app.manage(router)").expect("라우터 등록");
    let spawn = lib.find("Sidecar::spawn(").expect("사이드카 기동");
    assert!(guard < pool && pool < fetch && fetch < active && active < kinds && kinds < manage && manage < spawn);
    assert_eq!(lib.matches("router.set_ssh_pool(").count(), 1);
    assert!(lib.contains(", fetch_kinds, rust_api, rust_ssh, config_owned)"));
    // 홈을 알 수 없으면 풀을 주입하지 않는다(사이드카가 처리한다) — 빈 홈으로 만들지 않는다
    assert!(lib.contains("app.path().home_dir()"));
    // 하위 스위치 `ANTLER_RUST_SSH` 는 Phase 6b PR 9 부터 은퇴했다(결정 D5) — 라우터도 앱·사이드카도 이 값을 읽지 않는다(사이드카 env 로 *내보내기만* 한다)
    let router = read_src("src/router.rs");
    assert!(!router.contains("var(\"ANTLER_RUST_SSH\")") && !router.contains("fn rust_ssh_enabled"));
    for rel in ["src/lib.rs", "src/sidecar.rs", "src/mirror.rs", "src/git/fetch/service.rs"] {
        assert!(!read_src(rel).contains("var(\"ANTLER_RUST_SSH\")"), "{rel} 가 스위치를 직접 읽는다");
    }
}

#[test]
fn 사이드카_env_는_ssh_active_일_때만_1_이고_아니면_부모_값도_지운다() {
    let sidecar = read_src("src/sidecar.rs");
    assert!(sidecar.contains("cmd.env(\"ANTLER_RUST_SSH\", \"1\")"));
    assert!(sidecar.contains("cmd.env_remove(\"ANTLER_RUST_SSH\")"));
    assert!(sidecar.contains("rust_ssh: bool"));
}
