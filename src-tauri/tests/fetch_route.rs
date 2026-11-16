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
use antler_lib::router::{in_runtime_of, sidecar_fetch_kinds, Routed, Router, RUST_CHANNELS, RUST_FETCH_KINDS, UNDEFINED_ARG};
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
fn 사이드카에_승인을_부탁하는_호출은_호스트_구현_한_곳뿐이다() {
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
    // 서비스가 호스트에 부탁하는 호출(`host.approve(`, send_approval 안의 한 곳)과, 호스트 구현이 사이드카에 부탁하는 호출(한 곳)뿐이다.
    // 그 밖의 어디서도 `.approve(` 를 부르면 renderer 의 값이 승인으로 저장되는 길이 생길 수 있다
    assert_eq!(hits, [("fetch_host.rs".to_string(), 1), ("git/fetch/service.rs".to_string(), 1)]);
    assert_eq!(prod_code("src/fetch_host.rs").matches("sidecar.approve(").count(), 1);
    assert_eq!(prod_code("src/git/fetch/service.rs").matches("self.host.approve(").count(), 1);
    // Sidecar::approve 는 crate 밖에 열지 않고 dead_code 허용도 달지 않는다(실제로 쓰인다)
    let sidecar = prod_code("src/sidecar.rs");
    assert!(sidecar.contains("pub(crate) fn approve("), "approve 는 pub(crate) 이어야 한다");
    assert!(!sidecar.contains("pub fn approve("));
    assert!(!sidecar.contains("allow(dead_code)"));
    // 호스트 구현 파일도 플랫폼 주입 버전을 부르지 않는다
    let host = prod_code("src/fetch_host.rs");
    for banned in ["_for_test", "run_fetch(", "hooks_path_for("] {
        assert!(!host.contains(banned), "fetch_host.rs 에 {banned}");
    }
}

#[test]
fn fetch_prefs_받는_곳은_사이드카를_띄우기_전에_등록한다() {
    let lib = prod_code("src/lib.rs");
    let sink = lib.find("app.manage(FetchPrefsSink").expect("FetchPrefsSink 등록");
    let spawn = lib.find("Sidecar::spawn(").expect("사이드카 기동");
    assert!(sink < spawn, "첫 fetchPrefs 는 ready 전에 한 번만 온다 — 받는 곳이 먼저 등록돼야 한다");
    // 서비스 주입(set_fetch)도 사이드카보다 먼저고, 라우터 등록(sink 가 찾는 상태)도 sink 보다 먼저다
    let set_fetch = lib.find("router.set_fetch(").expect("서비스 주입");
    let manage_router = lib.find("app.manage(router)").expect("라우터 등록");
    assert!(set_fetch < manage_router && manage_router < sink);
    // 사이드카 기동 인자는 라우터 스위치에서 정한 값이다
    assert!(lib.contains("sidecar_fetch_kinds(router.enabled())"));
    // 포커스·sink 는 런타임 밖 스레드에서 불린다 — 라우터 호출은 앱 런타임 안에서
    assert_eq!(lib.matches("in_runtime_of(").count(), 2);
}

#[test]
fn 사이드카_env_는_스위치가_켜졌을_때만_local_wsl_이다() {
    assert_eq!(sidecar_fetch_kinds(true), Some("local,wsl"));
    assert_eq!(RUST_FETCH_KINDS, "local,wsl");
    assert_eq!(sidecar_fetch_kinds(false), None);
    // 사이드카 기동은 값이 없을 때 부모의 env 를 지운다(되돌리기가 뒤집히지 않게)
    assert!(prod_code("src/sidecar.rs").contains("env_remove(\"ANTLER_RUST_FETCH_KINDS\")"));
    // 사이드카가 읽는 이름·토큰이 TS 와 같다
    let ts = read_src("../src/sidecar/index.ts");
    assert!(ts.contains("ANTLER_RUST_FETCH_KINDS"));
    let owns = read_src("../src/sidecar/fetchOwns.ts");
    assert!(owns.contains("['local', 'wsl']"));
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
    MirrorRepo { id: id.into(), kind: kind.into(), path: path.into(), distro: distro.map(Into::into) }
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

fn rig(enabled: bool) -> (Router, Arc<Calls>) {
    let router = Router::new(enabled, false, mirror());
    let calls = Arc::new(Calls::default());
    router.set_fetch(Arc::new(Fake(calls.clone())));
    (router, calls)
}

fn log(calls: &Calls) -> Vec<String> {
    calls.log.lock().unwrap().clone()
}

fn is_sidecar(r: &Routed) -> bool {
    matches!(r, Routed::Sidecar)
}

fn envelope(r: Routed) -> Value {
    match r {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("Rust 경로를 타야 하는데 사이드카로 갔다"),
    }
}

#[tokio::test]
async fn git_fetch_는_local_wsl_이면_절대_사이드카로_가지_않고_그_밖은_사이드카로_간다() {
    let (router, calls) = rig(true);
    // local: Rust 가 처리한다
    let env = envelope(router.route("git:fetch", &json!(["loc", true])).await);
    assert_eq!(env["ok"], true);
    assert_eq!(env["data"]["repoId"], "loc");
    assert_eq!(log(&calls), ["fetch loc true"]);
    // 미러 검사에 걸린 local·wsl 도 사이드카로 보내지 않고 오류로 끝낸다(사이드카는 그 환경을 맡지 않는다 — 보내면 이중 경로가 된다)
    // (WSL 은 Windows 에서만 Rust 가 맡는다 — 그 밖의 OS 에서는 미러가 사이드카로 돌린다)
    let checked: &[&str] = if cfg!(windows) { &["unc", "wsl"] } else { &["unc"] };
    for id in checked {
        let env = envelope(router.route("git:fetch", &json!([id, true])).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{id}");
    }
    assert_eq!(log(&calls).len(), 1, "검사에 걸린 저장소는 서비스에 닿지 않는다");
    // ssh·github·미러에 없는 id·모양이 틀린 인자는 사이드카가 처리한다
    for args in [json!(["ssh", true]), json!(["gh", true]), json!(["nope", true]), json!([]), json!([null]), json!([5, true]), json!({ "a": 1 })] {
        assert!(is_sidecar(&router.route("git:fetch", &args).await), "{args}");
    }
    assert_eq!(log(&calls).len(), 1);
    assert_eq!(router.rust_handled(), 1);
}

#[tokio::test]
async fn 스위치가_꺼지면_모든_fetch_가_사이드카로_가고_서비스는_주입되지_않는다() {
    let (router, calls) = rig(false);
    for id in ["loc", "unc", "wsl", "ssh", "gh", "nope"] {
        assert!(is_sidecar(&router.route("git:fetch", &json!([id, true])).await), "{id}");
    }
    // repos:watch·prefs·포커스·종료도 서비스를 건드리지 않는다(되돌리기에서 Rust 가 자동 fetch 를 예약하면 이중 fetch 다)
    assert!(is_sidecar(&router.route("repos:watch", &json!(["loc"])).await));
    router.apply_fetch_prefs(prefs(1));
    router.fetch_on_focus();
    router.on_mirror_changed();
    router.shutdown();
    assert!(log(&calls).is_empty(), "{:?}", log(&calls));
    assert_eq!(router.rust_handled(), 0);
    assert!(sidecar_fetch_kinds(router.enabled()).is_none());
}

#[tokio::test]
async fn 서비스가_없으면_local_fetch_는_오류이고_사이드카로_가지_않는다() {
    let router = Router::new(true, false, mirror());
    let env = envelope(router.route("git:fetch", &json!(["loc", true])).await);
    assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")));
}

#[tokio::test]
async fn manual_은_정확히_true_일_때만_참이다() {
    let (router, calls) = rig(true);
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
    let (router, calls) = rig(true);
    for (arg, want) in [
        (json!(["loc"]), "active Some(\"loc\")"),
        // 사이드카가 맡는 저장소로 옮겨 가면 Rust 의 예약은 비운다
        (json!(["ssh"]), "active None"),
        (json!(["gh"]), "active None"),
        (json!(["nope"]), "active None"),
        (json!([null]), "active None"),
        // 미러 검사에 걸린 저장소는 Rust 가 fetch 할 수 없다
        (json!(["unc"]), "active None"),
        (json!([5]), "active None"),
    ] {
        calls.log.lock().unwrap().clear();
        assert!(is_sidecar(&router.route("repos:watch", &arg).await), "{arg}");
        assert_eq!(log(&calls), [want], "{arg}");
    }
}

#[tokio::test]
async fn 포커스_prefs_미러_변경_종료는_서비스로_간다() {
    let (router, calls) = rig(true);
    router.fetch_on_focus();
    router.apply_fetch_prefs(prefs(3));
    router.on_mirror_changed();
    router.shutdown();
    assert_eq!(log(&calls), ["focus", "prefs 3", "sync", "dispose"]);
}

#[test]
fn 서비스가_주입되기_전에_온_prefs_는_마지막_것이_주입될_때_적용된다() {
    let router = Router::new(true, false, mirror());
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
// 런타임 밖 스레드(사이드카 읽기 스레드·창 이벤트)에서의 호출
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
    let router = Arc::new(Router::new(true, false, m.clone()));
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
