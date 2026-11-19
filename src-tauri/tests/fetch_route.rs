//! fetch 라우팅(PR D) 통합 테스트: 라우터가 `git:fetch` 를 어디로 보내는지, 소유권(이중 fetch 없음), 앱 배선 순서, 승인 경로의 소스 검사.
//! 상태 기계 자체는 `fetch_service.rs`, 실제 git 으로 돌린 TS 골든 비교는 `channels.rs`(`git_fetch_시나리오가_ts_골든과_같다`)가 맡는다.
//! 이 파일은 git 을 띄우지 않는다(가짜 서비스·경로 없는 저장소).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::git::exec::{GitExecutor, LocalExecutor};
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::service::{ApprovalKind, FetchControl, FetchFuture, FetchHost, FetchPrefs, FetchService, FetchStatus, HostEvent};
use antler_lib::git::log_service::NoteOp;
use antler_lib::mirror::{MirrorRepo, RepoMirror, Target};
use antler_lib::router::{in_runtime_of, rust_fetch_kinds, Routed, Router, RUST_CHANNELS, UNDEFINED_ARG};
use futures_util::FutureExt;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사
// ---------------------------------------------------------------------------------------------------------------------

fn read_src(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 운영 코드의 끝(바이트 위치) — 들여쓰기 없는 `#[cfg(test)]` 바로 다음 줄이 `mod ` 인 곳이다(메서드 앞의 시험 전용 항목에서는 멈추지 않는다)
fn prod_end(text: &str) -> usize {
    let mut off = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(l) = lines.next() {
        if l.trim_end_matches(['\r', '\n']) == "#[cfg(test)]" && lines.peek().is_some_and(|n| n.starts_with("mod ")) {
            return off;
        }
        off += l.len();
    }
    text.len()
}

/// 주석 줄과 파일 끝의 시험 모듈을 뺀 운영 코드
fn prod_code(rel: &str) -> String {
    let src = read_src(rel);
    src[..prod_end(&src)].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") && !p.to_string_lossy().ends_with("_table.rs") {
            out.push(p);
        }
    }
}

#[test]
fn 승인은_설정에_직접_쓰고_fetch_설정은_라우터_등록_뒤에_첫_반영한다() {
    // Rust 가 설정을 소유한다(Phase 6b PR 9): 승인을 쓰는 곳은 호스트 구현 한 곳이다. `.approve(` 호출은 서비스(`host.approve(`)와 호스트 안의 쓰기 함수뿐이다
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&dir, &mut files);
    let mut hits: Vec<(String, usize)> = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        let n = prod_code(&format!("src/{rel}")).matches(".approve(").count();
        if n > 0 {
            hits.push((rel, n));
        }
    }
    assert_eq!(hits, [("git/fetch/service.rs".to_string(), 1)]);
    assert_eq!(prod_code("src/git/fetch/service.rs").matches("self.host.approve(").count(), 1);
    // 호스트 구현 파일도 플랫폼 주입 버전을 부르지 않는다
    let host = prod_code("src/fetch_host.rs");
    for banned in ["_for_test", "run_fetch(", "hooks_path_for("] {
        assert!(!host.contains(banned), "fetch_host.rs 에 {banned}");
    }
    // fetch 설정(주기·승인)은 설정 변경 알림이 직접 서비스에 넣는다(`config::sync`). 첫 반영은 라우터 등록 뒤다
    let lib = prod_code("src/lib.rs");
    let set_fetch = lib.find("router.set_fetch(").expect("서비스 주입");
    let manage_router = lib.find("app.manage(router)").expect("라우터 등록");
    let first_push = lib.find("config_sync.push(None,").expect("설정의 첫 반영");
    assert!(set_fetch < manage_router && manage_router < first_push);
    // 창 포커스·설정 알림·로그인 변경은 런타임 밖 스레드에서 불린다 — 라우터 호출은 앱 런타임 안에서
    // (미러 반영·fetch 설정 반영 둘, 창 포커스, 그리고 GitHub 로그인 변경 콜백이 부르는 `github_auth_changed`)
    assert_eq!(lib.matches("in_runtime_of(").count(), 4);
}

#[test]
fn fetch_소유_종류는_계산_값이다() {
    assert_eq!(rust_fetch_kinds(false, false), "local,wsl");
    assert_eq!(rust_fetch_kinds(true, false), "local,wsl,ssh");
    assert_eq!(rust_fetch_kinds(false, true), "local,wsl,github");
    assert_eq!(rust_fetch_kinds(true, true), "local,wsl,ssh,github");
}

#[test]
fn rust_채널에_git_fetch_가_있다() {
    assert!(RUST_CHANNELS.contains(&"git:fetch"));
}

// ---------------------------------------------------------------------------------------------------------------------
// 라우팅·소유권
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Calls {
    log: Mutex<Vec<String>>,
}

/// 호출만 기록하는 서비스. `fetch` 는 바로 끝난 상태를 돌려준다
struct Fake(Arc<Calls>);

fn status(id: &str) -> FetchStatus {
    FetchStatus { repo_id: id.into(), fetching: false, last_ok_at: Some(1), error: None, blocked: None, auth_paused: false, needs_manual: false, no_remote: false }
}

impl FetchControl for Fake {
    fn fetch(&self, repo_id: &str, manual: bool) -> FetchFuture {
        self.0.log.lock().unwrap().push(format!("fetch {repo_id} {manual}"));
        let s = status(repo_id);
        async move { s }.boxed().shared()
    }
    fn set_active(&self, repo_id: Option<&str>) {
        self.0.log.lock().unwrap().push(format!("active {repo_id:?}"));
    }
    fn on_focus(&self) {
        self.0.log.lock().unwrap().push("focus".into());
    }
    fn apply_prefs(&self, prefs: FetchPrefs) -> bool {
        self.0.log.lock().unwrap().push(format!("prefs {}", prefs.gen));
        true
    }
    fn sync_mirror(&self) {
        self.0.log.lock().unwrap().push("sync".into());
    }
    fn dispose(&self) {
        self.0.log.lock().unwrap().push("dispose".into());
    }
}

fn repo(id: &str, kind: &str, path: &str, distro: Option<&str>) -> MirrorRepo {
    MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: distro.map(Into::into), ssh_host: None }
}

fn mirror() -> RepoMirror {
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            repo("loc", "local", "C:\\nonexistent-antler\\r", None),
            repo("unc", "local", "\\\\evil\\share\\r", None),
            // 배포판이 없으면 미러 검사에 걸린다 — wsl 로 해석되지만 실행하기 전에 오류다(wsl.exe 를 띄우지 않는다)
            repo("wsl", "wsl", "/home/me/r", None),
            repo("ssh", "ssh", "/srv/r", None),
            repo("gh", "github", "/cache/r", None),
        ],
    );
    m
}

fn prefs(gen: u64) -> FetchPrefs {
    FetchPrefs { gen, auto_fetch_minutes: 5.0, approvals: Default::default(), trusted: Default::default() }
}

fn rig() -> (Router, Arc<Calls>) {
    let router = Router::new(false, mirror());
    let calls = Arc::new(Calls::default());
    router.set_fetch(Arc::new(Fake(calls.clone())));
    (router, calls)
}

fn log(calls: &Calls) -> Vec<String> {
    calls.log.lock().unwrap().clone()
}

fn is_unserved(r: &Routed) -> bool {
    matches!(r, Routed::Unserved)
}

/// `repos:watch`·`req:cancel` 의 응답(라우터가 `{ok:true}` 로 답한다)
fn is_acked(r: &Routed) -> bool {
    matches!(r, Routed::Done(v) if v == &json!({ "ok": true }))
}

fn envelope(r: Routed) -> Value {
    match r {
        Routed::Done(v) => v,
        Routed::Unserved => panic!("Rust 경로를 타야 하는데 풀지 못했다"),
    }
}

#[tokio::test]
async fn git_fetch_는_local_wsl_이면_rust_가_처리하고_그_밖은_풀_수_없다() {
    let (router, calls) = rig();
    // local: Rust 가 처리한다
    let env = envelope(router.route("git:fetch", &json!(["loc", true])).await);
    assert_eq!(env["ok"], true);
    assert_eq!(env["data"]["repoId"], "loc");
    assert_eq!(log(&calls), ["fetch loc true"]);
    // 미러 검사에 걸린 local·wsl 도 오류로 끝낸다
    // (WSL 은 Windows 에서만 Rust 가 맡는다 — 그 밖의 OS 에서는 미러가 풀지 않는다)
    let checked: &[&str] = if cfg!(windows) { &["unc", "wsl"] } else { &["unc"] };
    for id in checked {
        let env = envelope(router.route("git:fetch", &json!([id, true])).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{id}");
    }
    assert_eq!(log(&calls).len(), 1, "검사에 걸린 저장소는 서비스에 닿지 않는다");
    // ssh·github·미러에 없는 id·모양이 틀린 인자는 풀 수 없다
    for args in [json!(["ssh", true]), json!(["gh", true]), json!(["nope", true]), json!([]), json!([null]), json!([5, true]), json!({ "a": 1 })] {
        assert!(is_unserved(&router.route("git:fetch", &args).await), "{args}");
    }
    assert_eq!(log(&calls).len(), 1);
    assert_eq!(router.rust_handled(), 1);
}

#[tokio::test]
async fn 서비스가_없으면_local_fetch_는_오류다() {
    let router = Router::new(false, mirror());
    let env = envelope(router.route("git:fetch", &json!(["loc", true])).await);
    assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")));
}

#[tokio::test]
async fn manual_은_정확히_true_일_때만_참이다() {
    let (router, calls) = rig();
    let marker = json!({ UNDEFINED_ARG: true });
    for (arg, want) in [
        (json!(true), true),
        (json!(false), false),
        (json!("true"), false),
        (json!(1), false),
        (json!([true]), false),
        (json!({ "manual": true }), false),
        (json!(null), false),
        (marker.clone(), false),
    ] {
        calls.log.lock().unwrap().clear();
        router.route("git:fetch", &json!(["loc", arg])).await;
        assert_eq!(log(&calls), [format!("fetch loc {want}")], "{arg}");
    }
    // 두 번째 인자가 아예 없어도 자동 fetch 다
    calls.log.lock().unwrap().clear();
    router.route("git:fetch", &json!(["loc"])).await;
    assert_eq!(log(&calls), ["fetch loc false"]);
}

#[tokio::test]
async fn repos_watch_는_rust_가_맡는_저장소만_자동_fetch_대상으로_삼는다() {
    let (router, calls) = rig();
    for (arg, want) in [
        (json!(["loc"]), "active Some(\"loc\")"),
        // Rust 가 맡지 않는 저장소로 옮겨 가면 Rust 의 예약은 비운다
        (json!(["ssh"]), "active None"),
        (json!(["gh"]), "active None"),
        (json!(["nope"]), "active None"),
        (json!([null]), "active None"),
        // 미러 검사에 걸린 저장소는 Rust 가 fetch 할 수 없다
        (json!(["unc"]), "active None"),
        (json!([5]), "active None"),
    ] {
        calls.log.lock().unwrap().clear();
        assert!(is_acked(&router.route("repos:watch", &arg).await), "{arg}");
        assert_eq!(log(&calls), [want], "{arg}");
    }
    // 같은 소유 판정으로 Rust 감시도 시작됐다 — 존재하지 않는 경로에서 유예 뒤 기준 계산이 돌지 않게 끝내기 전에 멈춘다
    router.shutdown();
}

#[tokio::test]
async fn 포커스_prefs_미러_변경_종료는_서비스로_간다() {
    let (router, calls) = rig();
    router.fetch_on_focus();
    router.apply_fetch_prefs(prefs(3));
    router.on_mirror_changed();
    router.shutdown();
    assert_eq!(log(&calls), ["focus", "prefs 3", "sync", "dispose"]);
}

#[test]
fn 서비스가_주입되기_전에_온_prefs_는_마지막_것이_주입될_때_적용된다() {
    let router = Router::new(false, mirror());
    router.apply_fetch_prefs(prefs(1));
    router.apply_fetch_prefs(prefs(2));
    let calls = Arc::new(Calls::default());
    router.set_fetch(Arc::new(Fake(calls.clone())));
    assert_eq!(log(&calls), ["prefs 2"]);
    // 보관한 것은 한 번만 쓴다
    let calls2 = Arc::new(Calls::default());
    router.set_fetch(Arc::new(Fake(calls2.clone())));
    assert!(log(&calls2).is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------
// 런타임 밖 스레드(창 이벤트·알림 콜백)에서의 호출
// ---------------------------------------------------------------------------------------------------------------------

struct NullHost;

impl FetchHost for NullHost {
    fn emit(&self, _event: HostEvent) {}
    async fn confirm(&self, _repo_id: &str, _confirm: &ConfirmBox) -> Option<i64> {
        None
    }
    fn approve(&self, _kind: ApprovalKind, _repo_id: &str, _fingerprint: &str) {}
    fn note(&self, _op: NoteOp, _repo_id: &str) {}
}

#[test]
fn 활성_저장소가_있을_때_런타임_밖_스레드의_prefs_적용은_런타임에_들어가서만_안전하다() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let m = mirror();
    let router = Arc::new(Router::new(false, m.clone()));
    let make_runner = |_: &Target| Arc::new(GitExecutor::Local(LocalExecutor::default()));
    router.set_fetch(Arc::new(FetchService::new(NullHost, m, make_runner, None)));
    // 활성 저장소를 정해 둔다(예약은 주기를 받기 전이라 아직 없다)
    rt.block_on(router.route("repos:watch", &json!(["loc"])));
    // 런타임에 들어가지 않은 스레드: 예약이 태스크를 만들다 패닉한다 — 가드가 필요한 이유
    let bare = {
        let r = router.clone();
        std::thread::spawn(move || r.apply_fetch_prefs(prefs(1))).join()
    };
    assert!(bare.is_err(), "런타임 밖에서 예약하면 패닉해야 한다(이 테스트의 전제)");
    // 앱이 쓰는 방식: 런타임에 들어가서 부른다 — 패닉 없이 예약된다. 포커스·미러 변경도 같다
    let handle = rt.handle().clone();
    let r = router.clone();
    std::thread::spawn(move || {
        in_runtime_of(&handle, || {
            r.apply_fetch_prefs(prefs(2));
            r.fetch_on_focus();
            r.on_mirror_changed();
        });
    })
    .join()
    .expect("런타임에 들어가서 부르면 패닉하지 않는다");
    router.shutdown();
    rt.shutdown_timeout(Duration::from_secs(2));
}

// ---------------------------------------------------------------------------------------------------------------------
// ssh 의 fetch(Phase 5 PR 4): ssh 가 Rust 로 풀리면(ssh_active) fetch 도 Rust 서비스가 맡는다 — 같은 술어(rust_fetch_kinds)로
// ---------------------------------------------------------------------------------------------------------------------

fn ssh_repo(id: &str, host: &str) -> MirrorRepo {
    MirrorRepo { id: id.into(), kind: "ssh".into(), path: "/srv/r".into(), distro: None, ssh_host: Some(host.into()) }
}

fn ssh_mirror() -> RepoMirror {
    let m = mirror();
    m.apply(
        2,
        vec![repo("loc", "local", "C:\\nonexistent-antler\\r", None), ssh_repo("ssh", "devbox"), ssh_repo("sshbad", "-oProxyCommand=x"), repo("gh", "github", "/cache/r", None)],
    );
    m
}

/// ssh 저장소가 Rust 로 풀리는(풀이 주입된) 라우터. 호스트가 잘못된 ssh 저장소(`sshbad`)도 있다
fn ssh_rig() -> (Router, RepoMirror, Arc<Calls>) {
    let m = ssh_mirror();
    let router = Router::new(false, m.clone());
    let calls = Arc::new(Calls::default());
    router.set_fetch(Arc::new(Fake(calls.clone())));
    router.set_ssh_pool(antler_lib::git::exec::ssh::SshPool::for_home("C:\\nonexistent-antler-home"));
    assert!(router.ssh_active());
    (router, m, calls)
}

#[tokio::test]
async fn ssh_저장소의_git_fetch_는_ssh_활성이면_rust_가_처리한다() {
    let (router, m, calls) = ssh_rig();
    assert!(matches!(m.resolve("ssh"), antler_lib::mirror::Resolved::Rust(Target::Ssh { .. })));
    assert!(matches!(m.resolve("sshbad"), antler_lib::mirror::Resolved::Invalid(_)));
    for manual in [true, false] {
        let env = envelope(router.route("git:fetch", &json!(["ssh", manual])).await);
        assert_eq!((env["ok"].as_bool(), env["data"]["repoId"].as_str()), (Some(true), Some("ssh")), "{manual}");
    }
    assert_eq!(log(&calls), ["fetch ssh true", "fetch ssh false"]);
    // 호스트가 잘못된 ssh 는 오류다(local 의 검사에 걸린 저장소와 같다)
    let env = envelope(router.route("git:fetch", &json!(["sshbad", true])).await);
    assert_eq!((env["ok"].as_bool(), env["code"].as_str(), env["error"].as_str()), (Some(false), Some("ERROR"), Some("잘못된 SSH 호스트입니다")));
    assert_eq!(log(&calls).len(), 2, "검사에 걸린 저장소는 서비스에 닿지 않는다");
    // github·미러에 없는 id 는 풀 수 없다
    for id in ["gh", "nope"] {
        assert!(is_unserved(&router.route("git:fetch", &json!([id, true])).await), "{id}");
    }
    assert_eq!(router.rust_handled(), 2);
    assert_eq!(rust_fetch_kinds(router.ssh_active(), router.github_cache_active()), "local,wsl,ssh");
}

#[tokio::test]
async fn 풀이_없으면_ssh_fetch_는_풀_수_없다() {
    // 풀이 없는 라우터(미러의 ssh 해석이 꺼져 있으므로 fetch 소유도 없다)
    let bare = Router::new(false, ssh_mirror());
    bare.set_fetch(Arc::new(Fake(Arc::new(Calls::default()))));
    assert!(is_unserved(&bare.route("git:fetch", &json!(["ssh", true])).await));
    assert_eq!(rust_fetch_kinds(bare.ssh_active(), bare.github_cache_active()), "local,wsl");
}

#[tokio::test]
async fn ssh_저장소는_자동_fetch_대상이다() {
    let (router, _m, calls) = ssh_rig();
    for (arg, want) in [(json!(["ssh"]), "active Some(\"ssh\")"), (json!(["sshbad"]), "active None"), (json!(["gh"]), "active None"), (json!(["loc"]), "active Some(\"loc\")")] {
        calls.log.lock().unwrap().clear();
        assert!(is_acked(&router.route("repos:watch", &arg).await), "{arg}");
        assert_eq!(log(&calls), [want], "{arg}");
    }
    // ssh 저장소를 활성화하면 Rust 감시도 시작된다 — 유예(1.5초) 뒤 기준 계산이 SSH 접속을 시도하지 않게 끝내기 전에 멈춘다
    router.shutdown();
}

#[derive(Default)]
struct Built(Mutex<Vec<String>>);

/// 이미 닫은 ssh 실행기(풀이 정리한 것) — 어떤 설정 파일도 읽지 않고 모든 호출이 거부된다(연결하지 않는다)
fn closed_ssh(host: &str) -> Arc<GitExecutor> {
    let ex = antler_lib::git::exec::ssh::SshPool::for_home("").get(host);
    ex.dispose();
    Arc::new(GitExecutor::Ssh(ex))
}

/// 진짜 fetch 서비스: ssh 활성이면 ssh 저장소를 소유한다 — 실행기를 만들고, SSH 전송 오류는 network(백오프)다. 닫힌 실행기는 쥐지 않고 새로 받는다
#[tokio::test]
async fn fetch_서비스는_ssh_활성일_때_ssh_를_소유하고_전송_오류는_network_다() {
    let (_router, m, _calls) = ssh_rig();
    let built = Arc::new(Built::default());
    let b = built.clone();
    let make_runner = move |t: &Target| {
        b.0.lock().unwrap().push(t.source_key());
        match t {
            Target::Ssh { host, .. } => closed_ssh(host),
            _ => Arc::new(GitExecutor::Local(LocalExecutor::default())),
        }
    };
    let svc = FetchService::new(NullHost, m.clone(), make_runner, None);
    let s = svc.fetch("ssh", true).await;
    // 닫힌 실행기의 "이미 종료" 는 SSH 전송 오류 — 인증 중단(auth_paused)이 아니라 백오프다
    assert!(s.error.as_deref().is_some_and(|e| e.contains("종료")), "{:?}", s.error);
    assert!(!s.auth_paused && !s.needs_manual && s.blocked.is_none(), "{s:?}");
    assert_eq!(built.0.lock().unwrap().clone(), ["ssh:devbox"]);
    // 닫힌 실행기를 쥐고 있지 않다 — 다음 fetch 는 새로 받는다(풀이 닫았다가 같은 호스트가 다시 추가된 경우)
    svc.fetch("ssh", false).await;
    assert_eq!(built.0.lock().unwrap().clone(), ["ssh:devbox", "ssh:devbox"]);
    // 호스트가 잘못된 ssh 는 실행기를 만들지 않고 오류다
    let bad = svc.fetch("sshbad", true).await;
    assert_eq!(bad.error.as_deref(), Some("잘못된 SSH 호스트입니다"));
    assert_eq!(built.0.lock().unwrap().len(), 2);
    // github·미러에 없는 id 는 서비스가 맡지 않는다
    for id in ["gh", "nope"] {
        assert_eq!(svc.fetch(id, true).await.error.as_deref(), Some(format!("unknown repo: {id}").as_str()), "{id}");
    }
    assert_eq!(built.0.lock().unwrap().len(), 2);
}

/// 풀이 없는(ssh 해석이 꺼진) 미러에서는 서비스도 ssh 를 맡지 않는다
#[tokio::test]
async fn fetch_서비스는_풀이_없으면_ssh_를_소유하지_않는다() {
    let m = ssh_mirror();
    let built = Arc::new(Built::default());
    let b = built.clone();
    let make_runner = move |t: &Target| {
        b.0.lock().unwrap().push(t.source_key());
        Arc::new(GitExecutor::Local(LocalExecutor::default()))
    };
    let svc = FetchService::new(NullHost, m.clone(), make_runner, None);
    for id in ["ssh", "sshbad"] {
        assert_eq!(svc.fetch(id, true).await.error.as_deref(), Some(format!("unknown repo: {id}").as_str()), "{id}");
    }
    assert!(built.0.lock().unwrap().is_empty(), "{:?}", built.0.lock().unwrap());
    // 활성 저장소로 정해 두어도 미러 정리(sync_mirror)가 맡지 않는 것으로 비운다
    svc.set_active(Some("ssh"));
    svc.sync_mirror();
    assert!(built.0.lock().unwrap().is_empty());
}

/// 호스트가 미러에서 빠지면 fetch 서비스가 쥔 실행기도 함께 정리되고, 같은 호스트를 다시 추가하면 새 연결(새 실행기)로 fetch 한다
/// (루프백 거부 포트만 쓴다 — 실제 호스트·agent 에 닿지 않는다. 성공 경로는 `ssh_route.rs` 의 루프백 서버 시험이 본다)
#[tokio::test]
async fn ssh_호스트가_빠졌다_다시_추가되면_fetch_서비스도_새_실행기를_받는다() {
    use antler_lib::git::exec::ssh::{AgentSetting, SshExecutor, SshOptions, SshPool};
    let made = Arc::new(Mutex::new(Vec::<SshExecutor>::new()));
    let sink = made.clone();
    let pool = SshPool::new(move |host| {
        let mut opts = SshOptions::for_home("C:\\nonexistent-antler-home");
        opts.agent = AgentSetting::Disabled;
        opts.env = Arc::new(|_| None);
        let ex = SshExecutor::new(host, opts);
        sink.lock().unwrap().push(ex.clone());
        ex
    });
    let m = RepoMirror::default();
    let host = "127.0.0.1:1";
    m.apply(1, vec![ssh_repo("s", host)]);
    let router = Router::new(false, m.clone());
    router.set_ssh_pool(pool.clone());
    let p = pool.clone();
    let make_runner = move |t: &Target| match t {
        Target::Ssh { host, .. } => Arc::new(GitExecutor::Ssh(p.get(host))),
        _ => Arc::new(GitExecutor::Local(LocalExecutor::default())),
    };
    router.set_fetch(Arc::new(FetchService::new(NullHost, m.clone(), make_runner, None)));

    let fetch = |manual: bool| {
        let router = &router;
        async move { envelope(router.route("git:fetch", &json!(["s", manual])).await) }
    };
    let env = fetch(true).await;
    assert_eq!(env["ok"], true, "{env}");
    assert!(env["data"]["error"].is_string() && env["data"]["authPaused"] == false, "{env}");
    assert_eq!(pool.hosts(), [host]);
    assert_eq!(made.lock().unwrap().len(), 1);

    // 호스트가 빠지면 풀이 연결을 닫고 서비스도 그 실행기를 놓는다
    assert!(m.apply(2, vec![]));
    router.on_mirror_changed();
    assert!(pool.hosts().is_empty());
    assert!(made.lock().unwrap()[0].is_disposed());
    // 같은 호스트가 다시 추가됐다 — 새 실행기로 fetch 한다(닫힌 실행기의 "이미 종료" 가 아니다)
    assert!(m.apply(3, vec![ssh_repo("s", host)]));
    router.on_mirror_changed();
    let env = fetch(true).await;
    assert!(!env["data"]["error"].as_str().unwrap_or_default().contains("종료"), "{env}");
    assert_eq!(made.lock().unwrap().len(), 2, "새 실행기를 만들었다");
    assert!(!made.lock().unwrap()[1].is_disposed());
    assert_eq!(pool.hosts(), [host]);
    router.shutdown();
}

#[test]
fn fetch_소유_판정은_소스_종류만_쓴다() {
    // 서비스·라우터의 fetch 쪽은 mirror.resolve 가 아니라 resolve_fetch 만 부른다 — 해석 결과로 소유를 정하면 조회·감시의 소유(종류 목록)와 어긋난다
    let service = prod_code("src/git/fetch/service.rs");
    assert!(!service.contains("mirror.resolve("), "fetch 서비스가 mirror.resolve 로 소유를 정한다");
    assert_eq!(service.matches("mirror.resolve_fetch(").count(), 2);
    // github 갈래(`service/github.rs`)도 같은 함수만 쓴다
    let github = prod_code("src/git/fetch/service/github.rs");
    assert!(!github.contains("mirror.resolve("), "github 갈래가 mirror.resolve 로 소유를 정한다");
    assert_eq!(github.matches("mirror.resolve_fetch(").count(), 2);
    let router = prod_code("src/router.rs");
    assert_eq!(router.matches("mirror.resolve_fetch(").count(), 4, "git_fetch·repos:watch 의 자동 fetch 대상, 그리고 감시 소유(set_watch·sync_watch — PR 7b, fetch 와 같은 술어)");
    // git_fetch 안에는 해석 함수가 resolve_fetch 하나뿐이다
    let body = &router[router.find("async fn git_fetch").expect("git_fetch")..];
    let body = &body[..body.find("async fn git_file_channel").expect("다음 함수")];
    assert!(body.contains("resolve_fetch(") && !body.contains("mirror.resolve("));
    // 소유 종류는 계산 값 하나(rust_fetch_kinds)다 — 미러의 판정이 이 함수를 부른다. 상수가 남아 있지 않다
    let mirror = prod_code("src/mirror.rs");
    assert!(mirror.contains("rust_fetch_kinds(self.ssh.load(Ordering::SeqCst), self.github.load("));
    assert!(router.contains("pub fn rust_fetch_kinds(") && !router.contains("RUST_FETCH_KINDS"));
    assert_eq!(rust_fetch_kinds(false, false), "local,wsl");
    assert_eq!(rust_fetch_kinds(true, false), "local,wsl,ssh");
    assert_eq!(rust_fetch_kinds(false, true), "local,wsl,github");
    assert_eq!(rust_fetch_kinds(true, true), "local,wsl,ssh,github");
}
