//! fetch 상태 기계(`antler_lib::git::fetch::service`) 통합 테스트. TS `tests/fetch.test.ts` 의 `FetchService` 시나리오 중 PR C 몫(동시 fetch 합치기·수동 새로고침 확인 흐름·
//! 항상 허용 저장·자격 증명 도우미 승인·TOCTOU)을 실제 git 으로 옮겼고(이름은 `test-map.json`), 주기·백오프·60초 제한·큐·실패 분류는 가짜 git 과 멈춘 시계
//! (`start_paused`)로 검증한다. 멈춘 시계는 런타임이 한가하면 다음 타이머로 건너뛰므로 **프로세스를 띄우는 테스트와 섞지 않는다** — 가짜 git 테스트는 프로세스를 띄우지 않는다.
//!
//! 격리는 `fetch.rs` 와 같다(`isolate`). 추가로 자격 증명 도우미 시나리오를 위해 명령줄 범위 설정(`GIT_CONFIG_COUNT`)으로 도우미를 하나 심는다 —
//! 저장소 안의 값이 아니라 위험 항목으로 걸리지 않고, 전역 git 설정 파일은 건드리지 않는다. 네트워크는 127.0.0.1 만 쓴다.

use std::collections::VecDeque;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::git::exec::{ExecError, GitExecutor, LocalExecutor, RunOpts, RunResult};
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::guard::next_delay_ms;
use antler_lib::git::fetch::run::{inspect_config, GitRunner};
use antler_lib::git::fetch::service::{approve_message, ApprovalKind, FetchHost, FetchPrefs, FetchService, FetchStatus, HostEvent, FETCH_TIMEOUT, FIRST_DELAY};
use antler_lib::git::log_service::NoteOp;
use antler_lib::mirror::{MirrorRepo, RepoMirror};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::Instant;

/// 자격 증명 도우미가 불리면 이 파일에 쓴다(프로세스마다 하나 — 도우미를 쓰는 테스트는 하나뿐이다)
fn helper_marker() -> PathBuf {
    std::env::temp_dir().join(format!("antler-fsvc-helper-{}", std::process::id()))
}

fn posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 테스트 프로세스 환경 격리(fetch.rs 의 `isolate` 와 같다) + 명령줄 범위의 자격 증명 도우미 하나
// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    for k in [
        "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL",
        "GIT_TERMINAL_PROMPT",
    ] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var("GIT_CONFIG_KEY_0", "credential.helper");
    std::env::set_var("GIT_CONFIG_VALUE_0", format!("!echo x > \"{}\"", posix(&helper_marker())));
}

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사: 운영 코드에 플랫폼 주입이 없고, 승인 경로가 하나뿐이다
// ---------------------------------------------------------------------------------------------------------------------

fn read_src(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 주석 줄과 `#[cfg(test)]` 이후를 뺀 운영 코드
fn prod_code(rel: &str) -> String {
    let src = read_src(rel);
    let end = src.find("#[cfg(test)]").unwrap_or(src.len());
    src[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn 운영_코드는_플랫폼을_주입하지_않는다() {
    let service = prod_code("src/git/fetch/service.rs");
    for banned in ["_for_test", "platform_win32", "cfg!(windows)", "hooks_path_for", "fetch_config_for", "fetch_env_for", "process_ssh_env"] {
        assert!(!service.contains(banned), "service.rs 운영 코드에 {banned} 가 있다 — 플랫폼·환경 주입은 테스트 전용이다");
    }
    // 운영 경로는 주입 없는 진입점만 부른다
    assert!(service.contains("run_fetch(r, path, &again, &opts)") && service.contains("inspect_config(r, path"));
    // 주입 버전은 숨겨진 테스트 전용 이름이고, 운영 진입점이 실제 플랫폼을 고정한다
    let run = read_src("src/git/fetch/run.rs");
    for f in ["run_fetch_for_test", "fetch_config_for_test", "fetch_env_for_test"] {
        let at = run.find(&format!("pub async fn {f}")).or_else(|| run.find(&format!("pub fn {f}"))).unwrap_or_else(|| panic!("{f}"));
        assert!(run[..at].trim_end().ends_with("#[doc(hidden)]"), "{f} 는 #[doc(hidden)] 이어야 한다");
    }
    let run_prod = prod_code("src/git/fetch/run.rs");
    assert!(run_prod.contains("run_fetch_for_test(r, repo_path, ins, o, &process_ssh_env(), cfg!(windows))"));
    assert!(run_prod.contains("fetch_config_for_test(kind, no_hooks_dir, cfg!(windows), credential_helpers)"));
    assert!(run_prod.contains("fetch_env_for_test(ins, kind, env, cfg!(windows), allow_file)"));
    let guard = prod_code("src/git/fetch/guard.rs");
    assert!(guard.contains("hooks_path_for_test(local, no_hooks_dir, cfg!(windows), is_empty_dir)"));
    // 그 밖의 운영 소스(앱 코드)도 주입 버전을 부르지 않는다
    for rel in ["src/lib.rs", "src/router.rs", "src/sidecar.rs", "src/commands.rs", "src/git/log_service.rs", "src/git/file_channels.rs", "src/git/commit.rs"] {
        let code = prod_code(rel);
        for banned in ["_for_test", "run_fetch(", "hooks_path_for("] {
            assert!(!code.contains(banned), "{rel} 에 {banned}");
        }
    }
}

#[test]
fn 승인을_보내는_곳은_서비스의_두_지점뿐이고_줄을_만드는_곳은_하나다() {
    let service = prod_code("src/git/fetch/service.rs");
    // 서비스 안: 호스트에 승인을 부탁하는 호출은 send_approval 한 곳이고, send_approval 을 부르는 곳은 정확히 둘(수동 fetch 성공 뒤·"항상 허용" 뒤)
    assert_eq!(service.matches("host.approve(").count(), 1, "host.approve 호출");
    assert_eq!(service.matches("self.send_approval(").count(), 2, "send_approval 호출");
    let manual_at = service.find("self.send_approval(ApprovalKind::Approval").expect("수동 fetch 성공 뒤");
    let run_fetch_at = service.find("run_fetch(r, path, &again, &opts).await?").expect("run_fetch");
    assert!(manual_at > run_fetch_at, "수동 승인은 fetch 가 성공한 뒤에만");
    assert!(service[run_fetch_at..manual_at].contains("if manual {"), "수동 fetch 일 때만");
    let always_at = service.find("self.send_approval(ApprovalKind::Trusted").expect("항상 허용 뒤");
    assert!(service[..always_at].rfind("decision == Decision::Always").is_some_and(|p| always_at - p < 200), "항상 허용 응답일 때만");
    // 프로토콜 줄: "fetchApprove" 리터럴은 service.rs 의 approve_message 한 곳, 그것을 부르는 곳은 sidecar.rs 의 Sidecar::approve 한 곳
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&dir, &mut files);
    let (mut literal, mut callers) = (Vec::new(), Vec::new());
    for f in files {
        let rel = posix(f.strip_prefix(&dir).unwrap());
        let code = prod_code(&format!("src/{rel}"));
        if code.contains("fetchApprove") {
            literal.push(rel.clone());
        }
        if code.contains("approve_message(") && !rel.ends_with("fetch/service.rs") {
            callers.push(rel);
        }
    }
    assert_eq!(literal, ["git/fetch/service.rs"]);
    assert_eq!(callers, ["sidecar.rs"]);
    let sidecar = prod_code("src/sidecar.rs");
    assert_eq!(sidecar.matches("approve_message(").count(), 1);
    // 사이드카로 req 를 보내는 길(request)은 args 를 값으로만 넣고, fetchApprove 를 만들지 않는다
    assert!(!sidecar.contains("\"fetchApprove\""));
    // 모양: 프로토콜의 kind 문자열 두 개뿐
    assert_eq!(ApprovalKind::Approval.as_str(), "approval");
    assert_eq!(ApprovalKind::Trusted.as_str(), "trusted");
    assert_eq!(
        approve_message(ApprovalKind::Trusted, "r", &"a".repeat(64)),
        serde_json::json!({ "t": "fetchApprove", "kind": "trusted", "repoId": "r", "fingerprint": "a".repeat(64) })
    );
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

// ---------------------------------------------------------------------------------------------------------------------
// 공용 도구
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Status(FetchStatus),
    Note(String, String),
    Changed(String),
    Approve(String, String, String),
    Confirm { repo: String, title: String, buttons: Vec<String>, detail: String },
}

enum Reply {
    Now(Option<i64>),
    After(Duration, Option<i64>),
}

type ConfirmFn = Box<dyn FnMut(&ConfirmBox) -> Reply + Send>;

struct HostInner {
    log: Mutex<Vec<Ev>>,
    confirm: Mutex<ConfirmFn>,
}

/// 이벤트·확인·승인·알림을 순서대로 기록하는 호스트. 확인 창의 응답은 `on_confirm` 으로 정한다(기본: 취소 버튼)
#[derive(Clone)]
struct Host(Arc<HostInner>);

impl Host {
    fn new() -> Self {
        Host(Arc::new(HostInner { log: Mutex::new(Vec::new()), confirm: Mutex::new(Box::new(|_| Reply::Now(Some(2)))) }))
    }

    fn on_confirm(&self, f: impl FnMut(&ConfirmBox) -> Reply + Send + 'static) {
        *self.0.confirm.lock().unwrap() = Box::new(f);
    }

    fn log(&self) -> Vec<Ev> {
        self.0.log.lock().unwrap().clone()
    }

    fn statuses(&self) -> Vec<FetchStatus> {
        self.log().into_iter().filter_map(|e| if let Ev::Status(s) = e { Some(s) } else { None }).collect()
    }

    fn confirms(&self) -> usize {
        self.log().iter().filter(|e| matches!(e, Ev::Confirm { .. })).count()
    }

    fn approvals(&self) -> Vec<(String, String, String)> {
        self.log().into_iter().filter_map(|e| if let Ev::Approve(k, r, f) = e { Some((k, r, f)) } else { None }).collect()
    }

    fn push(&self, e: Ev) {
        self.0.log.lock().unwrap().push(e);
    }
}

impl FetchHost for Host {
    fn emit(&self, event: HostEvent) {
        match event {
            HostEvent::Status(s) => self.push(Ev::Status(s)),
            HostEvent::RepoChanged(id) => self.push(Ev::Changed(id)),
        }
    }

    fn confirm(&self, repo_id: &str, confirm: &ConfirmBox) -> impl Future<Output = Option<i64>> + Send {
        self.push(Ev::Confirm { repo: repo_id.to_string(), title: confirm.title.clone(), buttons: confirm.buttons.clone(), detail: confirm.detail.clone() });
        let reply = (self.0.confirm.lock().unwrap())(confirm);
        async move {
            match reply {
                Reply::Now(r) => r,
                Reply::After(d, r) => {
                    tokio::time::sleep(d).await;
                    r
                }
            }
        }
    }

    fn approve(&self, kind: ApprovalKind, repo_id: &str, fingerprint: &str) {
        self.push(Ev::Approve(kind.as_str().to_string(), repo_id.to_string(), fingerprint.to_string()));
    }

    fn note(&self, op: NoteOp, repo_id: &str) {
        self.push(Ev::Note(op.as_str().to_string(), repo_id.to_string()));
    }
}

fn res(code: i32, stdout: &str, stderr: &str) -> RunResult {
    RunResult { stdout: stdout.as_bytes().to_vec(), stderr: stderr.to_string(), code }
}

fn prefs(gen: u64, minutes: f64, approvals: &[(&str, &str)], trusted: &[(&str, &str)]) -> FetchPrefs {
    let map = |v: &[(&str, &str)]| v.iter().map(|(k, f)| ((*k).to_string(), (*f).to_string())).collect();
    FetchPrefs { gen, auto_fetch_minutes: minutes, approvals: map(approvals), trusted: map(trusted) }
}

const FP_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FP_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn mirror_of(repos: &[(&str, &str, &str, Option<&str>)]) -> RepoMirror {
    let m = RepoMirror::default();
    let list = repos.iter().map(|(id, kind, path, distro)| MirrorRepo { id: (*id).into(), kind: (*kind).into(), path: (*path).into(), distro: distro.map(Into::into) }).collect();
    assert!(m.apply(1, list));
    m
}

// ---------------------------------------------------------------------------------------------------------------------
// 가짜 git + 멈춘 시계: 주기·백오프·60초·큐·실패 분류
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Plan {
    Ok(Duration),
    Fail(String),
    /// 취소될 때까지 끝나지 않는다
    Hang,
    Panic,
}

struct FakeState {
    config: String,
    remotes: String,
    urls: String,
    plan: VecDeque<Plan>,
    /// fetch 가 성공할 때마다 ref 가 바뀐 것처럼 보인다
    bump_refs: bool,
}

/// 프로세스를 띄우지 않는 git. 응답은 `FakeState`, 호출은 `calls` 에 남는다
struct FakeGit {
    st: Mutex<FakeState>,
    calls: Mutex<Vec<Vec<String>>>,
    running: AtomicUsize,
    max_running: AtomicUsize,
    refs: AtomicUsize,
}

impl FakeGit {
    fn new() -> Arc<Self> {
        Arc::new(FakeGit {
            st: Mutex::new(FakeState {
                config: "local\0remote.origin.url\nhttps://example.com/x.git\0".into(),
                remotes: "origin\n".into(),
                urls: "https://example.com/x.git\n".into(),
                plan: VecDeque::new(),
                bump_refs: true,
            }),
            calls: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
            refs: AtomicUsize::new(0),
        })
    }

    fn plan(&self, p: Plan) {
        self.st.lock().unwrap().plan.push_back(p);
    }

    fn set(&self, f: impl FnOnce(&mut FakeState)) {
        f(&mut self.st.lock().unwrap());
    }

    fn fetch_calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().iter().filter(|a| subcommand(a) == "fetch").cloned().collect()
    }

    fn fetches(&self) -> usize {
        self.fetch_calls().len()
    }
}

/// 앞의 `-c <값>` 쌍을 건너뛴 첫 인자
fn subcommand(args: &[String]) -> &str {
    let mut i = 0;
    while args.get(i).is_some_and(|a| a == "-c") {
        i += 2;
    }
    args.get(i).map_or("", String::as_str)
}

struct Running<'a>(&'a FakeGit);

impl<'a> Running<'a> {
    fn enter(g: &'a FakeGit) -> Self {
        let now = g.running.fetch_add(1, Ordering::SeqCst) + 1;
        g.max_running.fetch_max(now, Ordering::SeqCst);
        Running(g)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
    }
}

impl GitRunner for FakeGit {
    fn kind(&self) -> &'static str {
        // 훅 차단 경로를 `/dev/null` 로 정하는 원격 환경(앱 소유 디렉터리가 필요 없다)
        "wsl"
    }

    fn run(&self, _repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        self.calls.lock().unwrap().push(args.to_vec());
        let cancel = opts.cancel.clone();
        let sub = subcommand(args).to_string();
        let remote_get_url = args.get(1).is_some_and(|a| a == "get-url");
        let (config, remotes, urls, bump) = {
            let st = self.st.lock().unwrap();
            (st.config.clone(), st.remotes.clone(), st.urls.clone(), st.bump_refs)
        };
        let plan = if sub == "fetch" { Some(self.st.lock().unwrap().plan.pop_front().unwrap_or(Plan::Ok(Duration::ZERO))) } else { None };
        async move {
            if cancel.as_ref().is_some_and(|t| t.is_cancelled()) {
                return Err(ExecError::Aborted);
            }
            match sub.as_str() {
                "--version" => Ok(res(0, "git version 2.49.0.windows.1\n", "")),
                "config" => Ok(res(0, &config, "")),
                "remote" => Ok(res(0, if remote_get_url { &urls } else { &remotes }, "")),
                "for-each-ref" => Ok(res(0, &format!("sig{}\n", self.refs.load(Ordering::SeqCst)), "")),
                "symbolic-ref" => Ok(res(0, "refs/heads/main\n", "")),
                "rev-parse" => Ok(res(0, "abc\n", "")),
                "fetch" => {
                    let _running = Running::enter(self);
                    match plan.expect("fetch 계획") {
                        Plan::Ok(d) => {
                            sleep_or_cancel(d, cancel.as_ref()).await?;
                            if bump {
                                self.refs.fetch_add(1, Ordering::SeqCst);
                            }
                            Ok(res(0, "", ""))
                        }
                        Plan::Fail(stderr) => Ok(res(128, "", &stderr)),
                        Plan::Hang => {
                            sleep_or_cancel(Duration::from_secs(60 * 60 * 24), cancel.as_ref()).await?;
                            Ok(res(0, "", ""))
                        }
                        Plan::Panic => panic!("가짜 git 이 fetch 중에 패닉했다"),
                    }
                }
                other => panic!("가짜 git 이 모르는 명령: {other}"),
            }
        }
    }
}

async fn sleep_or_cancel(d: Duration, cancel: Option<&tokio_util::sync::CancellationToken>) -> Result<(), ExecError> {
    match cancel {
        Some(t) => tokio::select! {
            () = tokio::time::sleep(d) => Ok(()),
            () = t.cancelled() => Err(ExecError::Aborted),
        },
        None => {
            tokio::time::sleep(d).await;
            Ok(())
        }
    }
}

type Svc = FetchService<Host, FakeGit>;

struct Fx {
    svc: Svc,
    host: Host,
    local: Arc<FakeGit>,
    wsl: Arc<FakeGit>,
}

/// 로컬 저장소 `r1`·`r2`(같은 환경)와 WSL 저장소 `w1`(Windows 에서만 Rust 가 다룬다)이 있는 서비스. 자동 fetch 주기는 첫 fetchPrefs 가 와야 정해진다
fn fx() -> Fx {
    let tmp = std::env::temp_dir();
    let (p1, p2) = (tmp.join("antler-fsvc-r1").to_string_lossy().into_owned(), tmp.join("antler-fsvc-r2").to_string_lossy().into_owned());
    let mirror = mirror_of(&[("r1", "local", &p1, None), ("r2", "local", &p2, None), ("w1", "wsl", "/home/me/w1", Some("Ubuntu"))]);
    let (local, wsl) = (FakeGit::new(), FakeGit::new());
    let host = Host::new();
    let (l, w) = (local.clone(), wsl.clone());
    let svc = FetchService::new(host.clone(), mirror, move |t| if matches!(t, antler_lib::mirror::Target::Local { .. }) { l.clone() } else { w.clone() }, None);
    Fx { svc, host, local, wsl }
}

fn minutes(m: u64) -> Duration {
    Duration::from_secs(m * 60)
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn 같은_저장소의_진행_중_요청은_하나로_합쳐진다() {
    let f = fx();
    f.local.plan(Plan::Ok(Duration::from_secs(5)));
    let a = f.svc.fetch("r1", false);
    let b = f.svc.fetch("r1", true); // 두 번째의 manual 은 무시된다(TS 와 같다)
    assert!(a.ptr_eq(&b), "같은 요청이어야 한다");
    let (sa, sb) = (a.await, b.await);
    assert_eq!(sa, sb);
    assert_eq!(f.host.statuses().iter().filter(|s| s.fetching).count(), 1);
    assert_eq!(f.local.fetches(), 1);
    // 끝나면 다음 호출은 새 요청이다
    f.svc.fetch("r1", false).await;
    assert_eq!(f.local.fetches(), 2);
}

#[tokio::test(start_paused = true)]
async fn 기다리는_쪽이_사라져도_fetch_는_끝까지_간다() {
    let f = fx();
    f.local.plan(Plan::Ok(Duration::from_secs(5)));
    drop(f.svc.fetch("r1", false));
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(f.local.fetches(), 1);
    let s = f.svc.status("r1");
    assert!(!s.fetching && s.last_ok_at.is_some(), "{s:?}");
}

#[tokio::test(start_paused = true)]
async fn ref_가_바뀌면_note_가_repo_changed_보다_먼저_나가고_안_바뀌면_둘_다_없다() {
    let f = fx();
    f.svc.fetch("r1", false).await;
    let log = f.host.log();
    let note = log.iter().position(|e| *e == Ev::Note("fetchChanged".into(), "r1".into())).expect("note");
    let changed = log.iter().position(|e| *e == Ev::Changed("r1".into())).expect("repo:changed");
    let last_status = log.iter().rposition(|e| matches!(e, Ev::Status(_))).unwrap();
    assert!(note < changed && changed < last_status, "{log:?}");
    assert_eq!(log.iter().filter(|e| matches!(e, Ev::Note(..))).count(), 1);
    // ref 가 그대로면 알리지 않는다
    f.local.set(|s| s.bump_refs = false);
    let before = f.host.log().len();
    f.svc.fetch("r1", false).await;
    let after = f.host.log()[before..].to_vec();
    assert!(after.iter().all(|e| matches!(e, Ev::Status(_))), "{after:?}");
}

#[tokio::test(start_paused = true)]
async fn 오류_문구는_300_utf16_단위로_자르고_쪼개진_서로게이트는_fffd_다() {
    let f = fx();
    // 299 단위 뒤에 이모지(2 단위)가 오면 300 번째가 이모지의 앞 절반이다
    let stderr = format!("{}😀뒤", "a".repeat(299));
    f.local.plan(Plan::Fail(stderr));
    let s = f.svc.fetch("r1", false).await;
    let e = s.error.expect("오류");
    assert_eq!(e.encode_utf16().count(), 300);
    assert!(e.ends_with('\u{fffd}') && e.starts_with("aaa"), "{e:?}");
    // 짧은 오류는 그대로
    f.local.plan(Plan::Fail("fatal: x\n".into()));
    assert_eq!(f.svc.fetch("r1", false).await.error.as_deref(), Some("fatal: x"));
}

#[tokio::test(start_paused = true)]
async fn 오류_문구_자르기는_protocol_안내_접두까지_포함해_300_단위다() {
    let f = fx();
    f.local.plan(Plan::Fail(format!("fatal: transport 'file' not allowed {}", "z".repeat(400))));
    let s = f.svc.fetch("r1", false).await;
    let e = s.error.unwrap();
    assert!(e.starts_with("자동 fetch 는 로컬 경로(file) remote 를 쓰지 않습니다. 새로고침으로 직접 fetch 하세요."), "{e}");
    assert_eq!(e.encode_utf16().count(), 300);
    assert!(s.auth_paused, "허용 안 된 전송은 재시도해도 소용없어 자동 fetch 를 멈춘다");
}

#[tokio::test(start_paused = true)]
async fn 제한_시간은_60초이고_걸리면_시간_초과_오류이며_백오프로_센다() {
    let f = fx();
    f.local.plan(Plan::Hang);
    let t0 = Instant::now();
    let s = f.svc.fetch("r1", false).await;
    assert_eq!(t0.elapsed(), FETCH_TIMEOUT);
    assert_eq!(s.error.as_deref(), Some("시간 초과(60초)"));
    assert!(!s.fetching && !s.auth_paused, "시간 초과는 네트워크 실패다");
    // 다음 fetch 는 정상이다(타이머·진행 요청이 남지 않는다)
    let s = f.svc.fetch("r1", false).await;
    assert_eq!(s.error, None);
    assert!(s.last_ok_at.is_some());
}

#[tokio::test(start_paused = true)]
async fn 제한_시간에는_확인_창이_떠_있던_시간이_포함된다() {
    let f = fx();
    f.local.set(|s| s.config = "local\0remote.origin.url\nhttps://example.com/x.git\0local\0protocol.ext.allow\nnever\0".into());
    // 확인 창을 61초 동안 열어 둔 뒤 "이번만" 을 누른다 — 창은 취소되지 않지만 닫히고 나면 다음 git 호출이 취소를 본다
    f.host.on_confirm(|_| Reply::After(Duration::from_secs(61), Some(0)));
    let t0 = Instant::now();
    let s = f.svc.fetch("r1", true).await;
    assert_eq!(t0.elapsed(), Duration::from_secs(61));
    assert_eq!(s.error.as_deref(), Some("시간 초과(60초)"));
    assert_eq!(f.local.fetches(), 0, "제한이 지난 뒤에는 fetch 를 시작하지 않는다");
    assert_eq!(f.host.confirms(), 1);
}

#[tokio::test(start_paused = true)]
async fn 제한_시간에는_같은_환경_큐에서_기다린_시간도_포함된다() {
    let f = fx();
    f.local.plan(Plan::Ok(Duration::from_secs(50)));
    f.local.plan(Plan::Ok(Duration::from_secs(20)));
    let t0 = Instant::now();
    let a = f.svc.fetch("r1", false);
    let b = f.svc.fetch("r2", false);
    let (sa, sb) = (a.await, b.await);
    assert_eq!(sa.error, None);
    // r2 는 r1 이 끝난(50초) 뒤에야 시작해 10초 만에 제한(60초)에 걸린다 — 자기 작업은 20초짜리였다
    assert_eq!(sb.error.as_deref(), Some("시간 초과(60초)"));
    assert_eq!(t0.elapsed(), Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn 환경마다_전용_큐가_하나라_같은_환경은_직렬이고_다른_환경은_동시에_돈다() {
    let f = fx();
    f.local.plan(Plan::Ok(Duration::from_secs(10)));
    f.local.plan(Plan::Ok(Duration::from_secs(10)));
    let t0 = Instant::now();
    let (a, b) = (f.svc.fetch("r1", false), f.svc.fetch("r2", false));
    a.await;
    b.await;
    assert_eq!(t0.elapsed(), Duration::from_secs(20), "같은 환경은 하나씩");
    assert_eq!(f.local.max_running.load(Ordering::SeqCst), 1);
    if !cfg!(windows) {
        return; // WSL 저장소는 Windows 에서만 Rust 가 다룬다
    }
    f.local.plan(Plan::Ok(Duration::from_secs(10)));
    f.wsl.plan(Plan::Ok(Duration::from_secs(10)));
    let t1 = Instant::now();
    let (a, b) = (f.svc.fetch("r1", false), f.svc.fetch("w1", false));
    a.await;
    b.await;
    assert_eq!(t1.elapsed(), Duration::from_secs(10), "다른 환경은 서로 기다리지 않는다");
    assert_eq!(f.wsl.fetches(), 1);
}

#[tokio::test(start_paused = true)]
async fn 첫_fetch_prefs_전에는_자동_fetch_를_예약하지_않고_받은_뒤_3초_뒤에_시작한다() {
    let f = fx();
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(f.local.fetches(), 0, "주기를 모르면 예약하지 않는다");
    assert!(f.svc.apply_prefs(prefs(1, 5.0, &[], &[])));
    let t0 = Instant::now();
    tokio::time::sleep(FIRST_DELAY - Duration::from_millis(1)).await;
    assert_eq!(f.local.fetches(), 0);
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(f.local.fetches(), 1, "{:?}", t0.elapsed());
}

#[tokio::test(start_paused = true)]
async fn 자동_fetch_는_3초_뒤_첫_실행_이후_주기마다_돈다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_millis(2_900)).await;
    assert_eq!(f.local.fetches(), 0);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(f.local.fetches(), 1);
    // 시도 시각(3초) + 5분
    tokio::time::sleep(minutes(5) - Duration::from_secs(1)).await;
    assert_eq!(f.local.fetches(), 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.local.fetches(), 2);
    tokio::time::sleep(minutes(5)).await;
    assert_eq!(f.local.fetches(), 3);
}

#[tokio::test(start_paused = true)]
async fn 주기_0_이면_자동_fetch_가_없고_주기를_바꾸면_다시_예약한다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 0.0, &[], &[]));
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(minutes(60)).await;
    assert_eq!(f.local.fetches(), 0);
    f.svc.apply_prefs(prefs(2, 1.0, &[], &[]));
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(f.local.fetches(), 1);
    // 이미 받은 gen 이하는 버린다
    assert!(!f.svc.apply_prefs(prefs(2, 0.0, &[], &[])));
    assert!(!f.svc.apply_prefs(prefs(1, 0.0, &[], &[])));
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert_eq!(f.local.fetches(), 2, "버려진 prefs 는 주기를 바꾸지 않는다");
    f.svc.apply_prefs(prefs(3, 0.0, &[], &[]));
    tokio::time::sleep(minutes(30)).await;
    assert_eq!(f.local.fetches(), 2, "0 으로 바꾸면 예약이 사라진다");
}

#[tokio::test(start_paused = true)]
async fn 네트워크_실패는_지수_백오프이고_성공하면_주기로_돌아온다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.local.plan(Plan::Fail("fatal: unable to access: Could not resolve host\n".into()));
    f.local.plan(Plan::Fail("fatal: unable to access: Could not resolve host\n".into()));
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_millis(3_100)).await;
    assert_eq!(f.local.fetches(), 1);
    let s = f.svc.status("r1");
    assert!(s.error.is_some() && !s.auth_paused, "{s:?}");
    // 실패 1회 → 주기의 2배(10분)
    let d1 = Duration::from_secs_f64(next_delay_ms(5.0, 1) / 1000.0);
    assert_eq!(d1, minutes(10));
    tokio::time::sleep(d1 - Duration::from_secs(1)).await;
    assert_eq!(f.local.fetches(), 1, "5분이 지나도 백오프 중이라 다시 하지 않는다");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.local.fetches(), 2);
    // 실패 2회 → 20분
    tokio::time::sleep(minutes(20) - Duration::from_secs(2)).await;
    assert_eq!(f.local.fetches(), 2);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(f.local.fetches(), 3);
    assert_eq!(f.svc.status("r1").error, None, "성공하면 오류가 지워진다");
    // 성공 뒤에는 다시 5분 주기
    tokio::time::sleep(minutes(5) + Duration::from_secs(1)).await;
    assert_eq!(f.local.fetches(), 4);
}

#[tokio::test(start_paused = true)]
async fn 백오프는_30분을_넘지_않는다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    for _ in 0..6 {
        f.local.plan(Plan::Fail("fatal: Could not resolve host\n".into()));
    }
    f.svc.set_active(Some("r1"));
    // 3초 → 10분 → 20분 → 30분 → 30분 …
    tokio::time::sleep(Duration::from_secs(4)).await;
    let mut gaps = Vec::new();
    for _ in 0..4 {
        let before = f.local.fetches();
        let t = Instant::now();
        while f.local.fetches() == before {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        gaps.push(t.elapsed().as_secs() / 60);
    }
    assert_eq!(gaps, [10, 20, 30, 30]);
}

#[tokio::test(start_paused = true)]
async fn 인증_실패는_수동_새로고침_전까지_자동_fetch_를_멈춘다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.local.plan(Plan::Fail("fatal: Authentication failed for 'https://example.com/x.git/'\n".into()));
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_secs(4)).await;
    let s = f.svc.status("r1");
    assert!(s.auth_paused && s.error.is_some());
    tokio::time::sleep(minutes(120)).await;
    assert_eq!(f.local.fetches(), 1, "인증 실패 뒤에는 자동으로 다시 하지 않는다");
    // 수동 새로고침이 풀고 다시 시도한다
    let s = f.svc.fetch("r1", true).await;
    assert!(!s.auth_paused && s.error.is_none());
    tokio::time::sleep(minutes(5) + Duration::from_secs(1)).await;
    assert_eq!(f.local.fetches(), 3, "풀린 뒤에는 다시 주기로 돈다");
}

#[tokio::test(start_paused = true)]
async fn 승인되지_않은_http_remote_의_자동_인증_실패는_needs_manual_이고_승인된_remote_와_ssh_는_아니다() {
    let auth_fail = || Plan::Fail("fatal: could not read Username for 'https://example.com': terminal prompts disabled\n".into());
    let f = fx();
    f.local.plan(auth_fail());
    let s = f.svc.fetch("r1", false).await;
    assert!(s.auth_paused && s.needs_manual, "{s:?}");
    // 수동 fetch 는 needs_manual 을 세우지 않는다
    f.local.plan(auth_fail());
    let s = f.svc.fetch("r1", true).await;
    assert!(s.auth_paused && !s.needs_manual, "{s:?}");
    // ssh remote 는 수동 승인으로 풀리는 문제가 아니다
    f.local.set(|s| s.urls = "ssh://git@example.com/x.git\n".into());
    f.local.plan(auth_fail());
    let s = f.svc.fetch("r1", false).await;
    assert!(s.auth_paused && !s.needs_manual, "{s:?}");
    // 승인된 remote(지문 일치)에서 실패하면 도우미를 쓰고도 실패한 것이라 안내하지 않는다
    f.local.set(|s| s.urls = "https://example.com/x.git\n".into());
    let fp = inspect_fingerprint(&f).await;
    f.svc.apply_prefs(prefs(1, 5.0, &[("r1", &fp)], &[]));
    f.local.plan(auth_fail());
    let s = f.svc.fetch("r1", false).await;
    assert!(s.auth_paused && !s.needs_manual, "{s:?}");
}

/// 가짜 git 의 remote 지문(서비스가 쓰는 것과 같은 검사)
async fn inspect_fingerprint(f: &Fx) -> String {
    inspect_config(&*f.local, ".", None).await.unwrap().remotes_fingerprint
}

#[tokio::test(start_paused = true)]
async fn 자동_fetch_의_자격_증명_도우미는_지문이_같은_승인에서만_켜진다() {
    let helper_off = |g: &FakeGit| g.fetch_calls().last().unwrap().windows(2).any(|w| w[0] == "-c" && w[1] == "credential.helper=");
    let f = fx();
    f.svc.fetch("r1", false).await;
    assert!(helper_off(&f.local), "승인 전 자동 fetch 는 도우미를 끈다");
    let fp = inspect_fingerprint(&f).await;
    // 다른 지문(URL 이 바뀐 뒤의 옛 승인)은 승인이 아니다
    f.svc.apply_prefs(prefs(1, 5.0, &[("r1", FP_A)], &[]));
    f.svc.fetch("r1", false).await;
    assert!(helper_off(&f.local));
    f.svc.apply_prefs(prefs(2, 5.0, &[("r1", &fp)], &[]));
    f.svc.fetch("r1", false).await;
    assert!(!helper_off(&f.local), "승인된 remote 의 자동 fetch 는 도우미를 쓴다");
    // 수동 fetch 는 승인과 무관하게 도우미를 쓴다
    f.svc.apply_prefs(prefs(3, 5.0, &[], &[]));
    f.svc.fetch("r1", true).await;
    assert!(!helper_off(&f.local));
    // 다른 저장소의 승인은 이 저장소에 적용되지 않는다
    let g = fx();
    g.svc.apply_prefs(prefs(4, 5.0, &[("r2", &fp)], &[]));
    g.svc.fetch("r1", false).await;
    assert!(helper_off(&g.local));
}

#[tokio::test(start_paused = true)]
async fn 설정_차단_remote_없음은_자동_fetch_를_멈추고_수동이_다시_본다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.local.set(|s| s.config = "local\0remote.origin.url\nhttps://example.com/x.git\0local\0core.sshcommand\nevil\0".into());
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_secs(4)).await;
    let s = f.svc.status("r1");
    assert_eq!(s.blocked.as_ref().map(|b| (b.keys.clone(), b.confirmable)), Some((vec!["core.sshcommand".to_string()], true)));
    assert_eq!(f.local.fetches(), 0, "아무것도 실행하지 않는다");
    tokio::time::sleep(minutes(60)).await;
    assert_eq!(f.host.confirms(), 0, "자동 fetch 는 묻지 않는다");
    assert_eq!(f.local.calls.lock().unwrap().iter().filter(|a| subcommand(a) == "config").count(), 1, "자동 재예약이 없다(검사는 첫 실행의 한 번뿐이고 차단되면 재검사에 가지 않는다)");
    // remote 없음
    let g = fx();
    g.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    g.local.set(|s| {
        s.config = String::new();
        s.remotes = String::new();
    });
    g.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_secs(4)).await;
    let s = g.svc.status("r1");
    assert!(s.no_remote && s.error.is_none());
    tokio::time::sleep(minutes(60)).await;
    assert_eq!(g.local.calls.lock().unwrap().iter().filter(|a| subcommand(a) == "config").count(), 1);
}

#[tokio::test(start_paused = true)]
async fn remote_url_은_허용_여부를_묻지_않고_거부한다() {
    let f = fx();
    f.local.set(|s| s.urls = "ext::sh -c touch x\n".into());
    f.host.on_confirm(|_| Reply::Now(Some(0)));
    let s = f.svc.fetch("r1", true).await;
    let b = s.blocked.expect("차단");
    assert!(!b.confirmable && b.keys[0].starts_with("remote.origin.url ("), "{b:?}");
    assert_eq!((f.host.confirms(), f.local.fetches()), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn 창_포커스는_마지막_시도가_주기보다_오래됐을_때만_fetch_한다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.svc.set_active(Some("r1"));
    f.svc.on_focus();
    settle().await;
    assert_eq!(f.local.fetches(), 0, "한 번도 시도하지 않았으면 첫 지연에 맡긴다");
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(f.local.fetches(), 1);
    f.svc.on_focus();
    settle().await;
    assert_eq!(f.local.fetches(), 1, "주기 안이면 하지 않는다");
    tokio::time::advance(minutes(5) - Duration::from_secs(10)).await;
    f.svc.on_focus();
    settle().await;
    // 예약된 타이머와 포커스 중 하나만 돈다(진행 중이면 합류한다)
    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(f.local.fetches(), 2);
    // 활성 저장소가 없으면 무시한다
    f.svc.set_active(None);
    tokio::time::advance(minutes(30)).await;
    f.svc.on_focus();
    settle().await;
    assert_eq!(f.local.fetches(), 2);
}

#[tokio::test(start_paused = true)]
async fn 활성_저장소를_바꾸면_이전_예약은_사라지고_forget_은_활성을_푼다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.svc.set_active(Some("r1"));
    tokio::time::sleep(Duration::from_secs(2)).await;
    f.svc.set_active(Some("r2"));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.local.fetches(), 0, "r1 의 예약은 사라졌고 r2 는 새로 3초를 센다");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(f.local.fetches(), 1);
    assert_eq!(f.host.statuses().iter().filter(|s| s.repo_id == "r2" && s.fetching).count(), 1);
    f.svc.forget("r2");
    tokio::time::sleep(minutes(30)).await;
    assert_eq!(f.local.fetches(), 1, "지운 저장소는 더 이상 예약되지 않는다");
}

#[tokio::test(start_paused = true)]
async fn dispose_는_예약과_진행_중_fetch_를_끊는다() {
    let f = fx();
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    f.local.plan(Plan::Hang);
    let run = f.svc.fetch("r2", false);
    settle().await;
    f.svc.set_active(Some("r1"));
    f.svc.dispose();
    let s = run.await;
    assert_eq!(s.error, None, "취소는 오류가 아니다");
    assert!(!s.fetching);
    tokio::time::sleep(minutes(30)).await;
    assert_eq!(f.local.fetches(), 1, "예약이 사라졌다");
}

#[tokio::test(start_paused = true)]
async fn cancel_은_진행_중_fetch_를_오류_없이_끝낸다() {
    let f = fx();
    f.local.plan(Plan::Hang);
    let run = f.svc.fetch("r1", true);
    settle().await;
    f.svc.cancel("r1");
    let s = run.await;
    assert!(s.error.is_none() && !s.fetching, "{s:?}");
    f.svc.cancel("없는-저장소"); // 아무 일도 없다
}

#[tokio::test(start_paused = true)]
async fn fetch_중_패닉하면_fetching_이_풀리고_다음_fetch_가_된다() {
    let f = fx();
    f.local.plan(Plan::Panic);
    let s = f.svc.fetch("r1", false).await;
    assert!(!s.fetching, "{s:?}");
    assert_eq!(s.error.as_deref(), Some("내부 오류로 fetch 하지 않았습니다"));
    assert!(!f.svc.status("r1").fetching);
    // 진행 요청이 남지 않아 새 요청이 된다
    let s = f.svc.fetch("r1", false).await;
    assert_eq!(s.error, None);
    assert_eq!(f.local.fetches(), 2);
}

#[tokio::test(start_paused = true)]
async fn 모르는_저장소는_오류_상태이고_git_을_부르지_않는다() {
    let f = fx();
    let s = f.svc.fetch("nope", false).await;
    assert_eq!(s.error.as_deref(), Some("unknown repo: nope"));
    assert!(!s.fetching);
    assert!(f.local.calls.lock().unwrap().is_empty());
    // ssh 저장소는 Rust 가 다루지 않는다
    let m = mirror_of(&[("s1", "ssh", "/srv/x", None)]);
    let host = Host::new();
    let g = FakeGit::new();
    let gc = g.clone();
    let svc = FetchService::new(host, m, move |_| gc.clone(), None);
    assert_eq!(svc.fetch("s1", true).await.error.as_deref(), Some("unknown repo: s1"));
    assert!(g.calls.lock().unwrap().is_empty());
}

// ── 확인 창 응답 해석 ──

const DANGEROUS: &str = "local\0remote.origin.url\nhttps://example.com/x.git\0local\0protocol.ext.allow\nnever\0";

#[tokio::test(start_paused = true)]
async fn 확인_응답은_창_종류에_맞게_해석된다() {
    // 0 = 이번만 허용, 1 = 항상 허용(승인 부탁이 나간다), 2·None = 거부
    for (reply, blocked, approvals) in [(Some(0), false, 1), (Some(1), false, 2), (Some(2), true, 0), (None, true, 0), (Some(7), true, 0)] {
        let f = fx();
        f.local.set(|s| s.config = DANGEROUS.into());
        f.host.on_confirm(move |_| Reply::Now(reply));
        let s = f.svc.fetch("r1", true).await;
        assert_eq!(s.blocked.is_some(), blocked, "{reply:?}: {s:?}");
        assert_eq!(f.local.fetches(), usize::from(!blocked), "{reply:?}");
        // 수동 fetch 가 성공하면 remote 승인이 가고, 항상 허용이면 신뢰 지문이 먼저 간다
        let kinds: Vec<String> = f.host.approvals().into_iter().map(|(k, ..)| k).collect();
        assert_eq!(kinds.len(), approvals, "{reply:?}: {kinds:?}");
        if reply == Some(1) {
            assert_eq!(kinds, ["trusted", "approval"]);
        }
        if blocked {
            assert!(s.blocked.unwrap().confirmable);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn 항목이_너무_많으면_닫기만_있는_차단_창이고_어떤_응답도_허용이_아니다() {
    // 13개(한도 12 초과) — 차단 창의 "닫기"는 응답 0 이다. 허용(이번만 = 0)으로 읽히면 위험 설정을 실행한다
    let mut cfg = String::from("local\0remote.origin.url\nhttps://example.com/x.git\0");
    for i in 0..13 {
        cfg.push_str(&format!("local\0protocol.p{i}.allow\nnever\0"));
    }
    for reply in [Some(0), Some(1), Some(2), None] {
        let f = fx();
        f.local.set(|s| s.config = cfg.clone());
        f.host.on_confirm(move |_| Reply::Now(reply));
        let s = f.svc.fetch("r1", true).await;
        assert_eq!(s.blocked.as_ref().map(|b| b.confirmable), Some(true), "{reply:?}");
        assert_eq!(f.local.fetches(), 0, "{reply:?}");
        assert!(f.host.approvals().is_empty(), "{reply:?}");
        let Some(Ev::Confirm { buttons, title, .. }) = f.host.log().into_iter().find(|e| matches!(e, Ev::Confirm { .. })) else { panic!("확인 창이 떠야 한다") };
        assert_eq!((buttons, title.as_str()), (vec!["닫기".to_string()], "fetch 차단"));
    }
}

#[tokio::test(start_paused = true)]
async fn 자동_fetch_는_위험_설정을_묻지_않고_막는다() {
    let f = fx();
    f.local.set(|s| s.config = DANGEROUS.into());
    f.host.on_confirm(|_| Reply::Now(Some(0)));
    let s = f.svc.fetch("r1", false).await;
    assert_eq!(s.blocked.map(|b| b.confirmable), Some(true));
    assert_eq!((f.host.confirms(), f.local.fetches()), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn 항상_허용은_같은_실행에서는_다시_묻지_않고_지우면_사라지며_사이드카가_돌려준_지문으로_다시_통한다() {
    let f = fx();
    f.local.set(|s| s.config = DANGEROUS.into());
    f.host.on_confirm(|_| Reply::Now(Some(1)));
    f.svc.fetch("r1", true).await;
    let (_, _, trusted_fp) = f.host.approvals().into_iter().find(|(k, ..)| k == "trusted").expect("신뢰 부탁");
    f.host.on_confirm(|_| panic!("같은 지문은 다시 묻지 않는다"));
    assert_eq!(f.svc.fetch("r1", false).await.blocked, None);
    // 저장소를 지우면 이번 실행의 허용도 지워진다 — 다음 자동 fetch 는 다시 막힌다
    f.svc.forget("r1");
    assert_eq!(f.svc.fetch("r1", false).await.blocked.map(|b| b.confirmable), Some(true));
    // 사이드카가 저장한 신뢰 지문을 fetchPrefs 로 받으면(다시 켠 앱의 첫 prefs) 확인 없이 통한다
    f.svc.apply_prefs(prefs(1, 5.0, &[], &[("r1", &trusted_fp)]));
    assert_eq!(f.svc.fetch("r1", false).await.blocked, None);
    // 지문이 다르면(설정이 바뀌었거나 다른 값이 저장됐다) 통하지 않는다
    let g = fx();
    g.local.set(|s| s.config = DANGEROUS.into());
    g.svc.apply_prefs(prefs(1, 5.0, &[], &[("r1", FP_B)]));
    assert_eq!(g.svc.fetch("r1", false).await.blocked.map(|b| b.confirmable), Some(true));
    // 다른 저장소의 신뢰는 적용되지 않는다
    g.svc.apply_prefs(prefs(2, 5.0, &[], &[("r2", &trusted_fp)]));
    assert_eq!(g.svc.fetch("r1", false).await.blocked.map(|b| b.confirmable), Some(true));
}

#[tokio::test(start_paused = true)]
async fn 방금_부탁한_승인은_되돌아오기_전까지_오버레이로_통하고_다른_값이_오거나_저장소를_지우면_사라진다() {
    let helper_off = |g: &FakeGit| g.fetch_calls().last().unwrap().windows(2).any(|w| w[0] == "-c" && w[1] == "credential.helper=");
    let f = fx();
    f.svc.fetch("r1", true).await; // 수동 성공 → 승인 부탁(아직 사이드카가 되돌려 주지 않았다)
    let (_, _, fp) = f.host.approvals().into_iter().next().expect("승인 부탁");
    f.svc.fetch("r1", false).await;
    assert!(!helper_off(&f.local), "오버레이로 승인된 remote 의 자동 fetch 는 도우미를 쓴다");
    // 사이드카가 같은 값을 돌려주면 오버레이는 정리되고 스냅샷이 같은 판정을 한다
    f.svc.apply_prefs(prefs(1, 5.0, &[("r1", &fp)], &[]));
    f.svc.fetch("r1", false).await;
    assert!(!helper_off(&f.local));
    // 사이드카가 되돌려 주기 전의 스냅샷(값이 없다)은 오버레이를 지우지 않는다
    let g = fx();
    g.svc.fetch("r1", true).await;
    g.svc.apply_prefs(prefs(1, 5.0, &[], &[]));
    g.svc.fetch("r1", false).await;
    assert!(!helper_off(&g.local), "처리 전의 스냅샷은 오버레이를 지우지 않는다");
    // 다른 값이 오면(덮어써졌다) 오버레이는 사라진다 — 옛 승인은 통하지 않는다
    g.svc.apply_prefs(prefs(2, 5.0, &[("r1", FP_B)], &[]));
    g.svc.fetch("r1", false).await;
    assert!(helper_off(&g.local));
    // 저장소를 지우면 오버레이도 사라진다
    let h = fx();
    h.svc.fetch("r1", true).await;
    h.svc.forget("r1");
    h.svc.fetch("r1", false).await;
    assert!(helper_off(&h.local), "지운 저장소의 승인은 남지 않는다");
}

#[test]
fn fetch_prefs_메시지는_모양이_맞을_때만_읽는다() {
    let ok = serde_json::json!({ "t": "fetchPrefs", "gen": 3, "autoFetchMinutes": 5, "approvals": { "a": FP_A }, "trusted": {} });
    let p = FetchPrefs::from_message(&ok).expect("정상");
    assert_eq!((p.gen, p.auto_fetch_minutes), (3, 5.0));
    assert_eq!(p.approvals.get("a").map(String::as_str), Some(FP_A));
    // 지문 모양이 틀린 항목만 뺀다(없는 것과 같다 = 승인되지 않음)
    let bad_fp = serde_json::json!({ "gen": 1, "autoFetchMinutes": 5, "approvals": { "a": "forged", "b": FP_B, "c": FP_A.to_uppercase(), "d": 5 }, "trusted": {} });
    assert_eq!(FetchPrefs::from_message(&bad_fp).unwrap().approvals.keys().collect::<Vec<_>>(), ["b"]);
    for m in [
        serde_json::json!({ "autoFetchMinutes": 5, "approvals": {}, "trusted": {} }),
        serde_json::json!({ "gen": -1, "autoFetchMinutes": 5, "approvals": {}, "trusted": {} }),
        serde_json::json!({ "gen": 1, "autoFetchMinutes": -1, "approvals": {}, "trusted": {} }),
        serde_json::json!({ "gen": 1, "autoFetchMinutes": "5", "approvals": {}, "trusted": {} }),
        serde_json::json!({ "gen": 1, "autoFetchMinutes": null, "approvals": {}, "trusted": {} }),
        serde_json::json!({ "gen": 1, "autoFetchMinutes": 5, "approvals": [], "trusted": {} }),
        serde_json::json!({ "gen": 1, "autoFetchMinutes": 5, "approvals": {} }),
        serde_json::json!({ "gen": 1, "approvals": {}, "trusted": {} }),
    ] {
        assert!(FetchPrefs::from_message(&m).is_none(), "{m}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 실제 git: TS tests/fetch.test.ts 의 FetchService 시나리오(PR C 몫)
// ---------------------------------------------------------------------------------------------------------------------

#[allow(clippy::disallowed_methods)]
fn raw(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("antler-fsvc-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file_url(p: &Path) -> String {
    let s = posix(p);
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

fn raw_git(dir: &Path, args: &[&str]) -> String {
    let out = raw("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().expect("git 실행");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// bare remote(file://) + 시드 작업 복제본 + 테스트 대상 복제본
struct Case {
    t: TempDir,
    work: PathBuf,
    repo: PathBuf,
    marker: PathBuf,
}

impl Case {
    fn new() -> Self {
        let t = TempDir::new("case");
        let dir = t.0.clone();
        let (bare, work, repo) = (dir.join("remote.git"), dir.join("work"), dir.join("repo"));
        assert!(raw("git").args(["init", "-q", "--bare", "-b", "main"]).arg(&bare).status().unwrap().success());
        assert!(raw("git").args(["clone", "-q", &file_url(&bare)]).arg(&work).output().unwrap().status.success());
        raw_git(&work, &["checkout", "-q", "-b", "main"]);
        raw_git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        raw_git(&work, &["push", "-q", "origin", "main"]);
        assert!(raw("git").args(["clone", "-q", &file_url(&bare)]).arg(&repo).output().unwrap().status.success());
        let marker = dir.join("PWNED");
        Case { t, work, repo, marker }
    }

    fn push_commit(&self, msg: &str) -> String {
        raw_git(&self.work, &["commit", "-q", "--allow-empty", "-m", msg]);
        raw_git(&self.work, &["push", "-q", "origin", "main"]);
        raw_git(&self.work, &["rev-parse", "HEAD"])
    }

    fn git(&self, args: &[&str]) -> String {
        raw_git(&self.repo, args)
    }

    fn repo_str(&self) -> String {
        self.repo.to_string_lossy().into_owned()
    }

    /// 실행되면 마커 파일을 만드는 무해한 페이로드(git 이 셸로 실행하는 설정 값)
    fn payload(&self) -> String {
        format!("echo x > \"{}\"", posix(&self.marker))
    }

    fn marker_exists(&self) -> bool {
        self.marker.exists()
    }

    fn no_hooks(&self) -> String {
        self.t.0.join("no-hooks").to_string_lossy().into_owned()
    }
}

/// 실제 git 을 부르면서 인자를 기록하는 실행기
struct Recording {
    inner: GitExecutor,
    calls: Mutex<Vec<Vec<String>>>,
}

impl Recording {
    fn new() -> Arc<Self> {
        Arc::new(Recording { inner: GitExecutor::Local(LocalExecutor::default()), calls: Mutex::new(Vec::new()) })
    }

    fn fetch_calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().iter().filter(|a| subcommand(a) == "fetch").cloned().collect()
    }

    fn last_fetch_helpers_off(&self) -> bool {
        self.fetch_calls().last().expect("fetch 호출").windows(2).any(|w| w[0] == "-c" && w[1] == "credential.helper=")
    }
}

impl GitRunner for Recording {
    fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    fn run(&self, repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        self.calls.lock().unwrap().push(args.to_vec());
        let inner = &self.inner;
        async move { inner.run(repo_path, args, opts).await }
    }
}

type RealSvc = FetchService<Host, Recording>;

/// 저장소 `r` 하나를 가진 실제 git 서비스
fn real(c: &Case) -> (RealSvc, Host, Arc<Recording>, RepoMirror) {
    let mirror = mirror_of(&[("r", "local", &c.repo_str(), None)]);
    let host = Host::new();
    let rec = Recording::new();
    let (r, nh) = (rec.clone(), c.no_hooks());
    (FetchService::new(host.clone(), mirror.clone(), move |_| r.clone(), Some(nh)), host, rec, mirror)
}

fn real_again(c: &Case, mirror: &RepoMirror) -> (RealSvc, Host, Arc<Recording>) {
    let host = Host::new();
    let rec = Recording::new();
    let (r, nh) = (rec.clone(), c.no_hooks());
    (FetchService::new(host.clone(), mirror.clone(), move |_| r.clone(), Some(nh)), host, rec)
}

async fn local_inspect(c: &Case) -> antler_lib::git::fetch::guard::ConfigInspection {
    inspect_config(&GitExecutor::Local(LocalExecutor::default()), &c.repo_str(), None).await.expect("설정 검사")
}

#[tokio::test]
async fn 실제_git_같은_저장소의_동시_fetch_는_하나로_합쳐진다() {
    let c = Case::new();
    let (svc, host, rec, _) = real(&c);
    let a = svc.fetch("r", false);
    let b = svc.fetch("r", true);
    assert!(a.ptr_eq(&b), "같은 요청이어야 한다");
    a.await;
    b.await;
    assert_eq!(host.statuses().iter().filter(|s| s.fetching).count(), 1);
    // 자동 fetch 는 file remote 를 쓰지 않아 실패하지만 fetch 는 한 번만 시도됐다
    assert_eq!(rec.fetch_calls().len(), 1);
}

#[tokio::test]
async fn 실제_git_수동_새로고침은_거절하면_막힌_채로_두고_허용하면_같은_설정에_한해_한_번만_묻는다() {
    let c = Case::new();
    // 무해한 위험 키(프로토콜 정책)로 허용 경로를 검증한다
    c.git(&["config", "protocol.ext.allow", "never"]);
    let (svc, host, _, _) = real(&c);
    let sha = c.push_commit("second");
    let answer = Arc::new(AtomicI64::new(2)); // 취소
    let a = answer.clone();
    host.on_confirm(move |_| Reply::Now(Some(a.load(Ordering::SeqCst))));

    let mut st = svc.fetch("r", true).await;
    assert_eq!(st.blocked.as_ref().map(|b| b.confirmable), Some(true));
    assert_eq!(host.confirms(), 1);
    let Some(Ev::Confirm { detail, buttons, .. }) = host.log().into_iter().find(|e| matches!(e, Ev::Confirm { .. })) else { panic!("확인 창") };
    assert!(detail.contains("protocol.ext.allow"), "{detail}");
    assert_eq!(buttons, ["이번만 fetch", "이 저장소에서 항상 허용", "취소"]);
    assert_ne!(c.git(&["rev-parse", "origin/main"]), sha);

    answer.store(0, Ordering::SeqCst); // 이번만
    st = svc.fetch("r", true).await;
    assert_eq!(st.blocked, None);
    assert_eq!(st.error, None);
    assert_eq!(c.git(&["rev-parse", "origin/main"]), sha);

    // 허용한 설정은 다시 묻지 않는다. 설정이 바뀌면 다시 묻는다
    svc.fetch("r", true).await;
    assert_eq!(host.confirms(), 2);
    c.git(&["config", "protocol.file.allow", "always"]);
    answer.store(2, Ordering::SeqCst);
    svc.fetch("r", true).await;
    assert_eq!(host.confirms(), 3);
    assert!(!c.marker_exists());
}

#[tokio::test]
async fn 실제_git_항상_허용은_앱을_다시_켜도_묻지_않고_설정이_바뀌면_다시_묻고_저장소를_지우면_사라진다() {
    let c = Case::new();
    c.git(&["config", "protocol.ext.allow", "never"]);
    let ins = local_inspect(&c).await;
    let (first, host1, _, mirror) = real(&c);
    host1.on_confirm(|_| Reply::Now(Some(1)));
    assert_eq!(first.fetch("r", true).await.blocked, None);
    assert_eq!(host1.confirms(), 1);
    // 승인 부탁: 신뢰 지문(위험 항목의 지문)이 먼저, 수동 fetch 성공 뒤 remote 승인이 간다
    let sent = host1.approvals();
    assert_eq!(sent.iter().map(|(k, r, _)| (k.as_str(), r.as_str())).collect::<Vec<_>>(), [("trusted", "r"), ("approval", "r")]);
    assert_eq!(sent[0].2, ins.fingerprint);
    assert_eq!(sent[1].2, ins.remotes_fingerprint);

    // 저장소를 지우면(forget) 이 서비스에서 신뢰가 사라진다 — 다음 자동 fetch 는 다시 막힌다
    first.forget("r");
    assert_eq!(first.fetch("r", false).await.blocked.map(|b| b.confirmable), Some(true));

    // 다시 켠 앱: 사이드카가 저장한 지문을 fetchPrefs 로 받는다. 확인 없이 자동 fetch 까지 통과한다(file remote 라 자동 fetch 는 전송 정책에서만 실패한다)
    let (second, host2, rec2) = real_again(&c, &mirror);
    assert!(second.apply_prefs(prefs(1, 5.0, &[], &[("r", &ins.fingerprint)])));
    let sha = c.push_commit("second");
    assert_eq!(second.fetch("r", false).await.blocked, None);
    host2.on_confirm(|_| Reply::Now(Some(2)));
    assert_eq!(second.fetch("r", true).await.blocked, None);
    assert_eq!(c.git(&["rev-parse", "origin/main"]), sha);
    assert_eq!(host2.confirms(), 0);
    assert_eq!(rec2.fetch_calls().len(), 2);

    // 설정이 바뀌면 지문이 달라져 다시 묻는다
    c.git(&["config", "protocol.file.allow", "always"]);
    assert_eq!(second.fetch("r", true).await.blocked.map(|b| b.confirmable), Some(true));
    assert_eq!(host2.confirms(), 1);
}

#[tokio::test]
async fn 실제_git_자동_fetch_는_승인된_remote_에서만_자격_증명_도우미를_쓴다() {
    let srv = auth_server().await;
    let c = Case::new();
    let marker = helper_marker();
    let _ = fs::remove_file(&marker);
    c.git(&["config", "remote.origin.url", &format!("http://127.0.0.1:{}/x.git", srv.port)]);
    let (svc, _host, rec, _) = real(&c);

    // ① 승인 전: 도우미 없음(그리고 인증 실패로 자동 fetch 가 멈추고, 수동으로 한 번 누르라는 안내 플래그가 선다)
    let mut st = svc.fetch("r", false).await;
    assert!(!marker.exists(), "승인 전에는 도우미를 부르지 않는다");
    assert!(rec.last_fetch_helpers_off());
    assert!(st.auth_paused && st.needs_manual, "{st:?}");

    // ② 승인된 remote(지문 일치): 자동 fetch 도 도우미를 쓴다
    let ins = local_inspect(&c).await;
    assert!(svc.apply_prefs(prefs(1, 5.0, &[("r", &ins.remotes_fingerprint)], &[])));
    st = svc.fetch("r", false).await;
    assert!(marker.exists(), "승인된 remote 의 자동 fetch 는 도우미를 쓴다");
    assert!(!rec.last_fetch_helpers_off());
    assert!(!st.needs_manual, "{st:?}");

    // ③ URL 이 바뀌면 승인이 무효 — 도우미가 다시 꺼진다
    fs::remove_file(&marker).unwrap();
    c.git(&["config", "remote.origin.url", &format!("http://127.0.0.1:{}/other.git", srv.port)]);
    st = svc.fetch("r", false).await;
    assert!(!marker.exists());
    assert!(rec.last_fetch_helpers_off());
    assert!(st.needs_manual, "{st:?}");
    let _ = fs::remove_file(&marker);
    srv.task.abort();
}

#[tokio::test]
async fn 실제_git_수동_fetch_가_성공하면_그_remote_가_승인되고_실패하거나_자동이면_승인하지_않는다() {
    let c = Case::new();
    let (svc, host, _, _) = real(&c);
    svc.fetch("r", false).await; // file remote 라 자동은 실패 — 승인 없음
    assert!(host.approvals().is_empty());
    let st = svc.fetch("r", true).await;
    assert_eq!(st.error, None);
    let ins = local_inspect(&c).await;
    assert_eq!(host.approvals(), [("approval".to_string(), "r".to_string(), ins.remotes_fingerprint.clone())]);
    // 실패한 수동 fetch 는 승인하지 않는다(없는 경로로 바꾸면 fetch 가 실패한다)
    c.git(&["config", "remote.origin.url", &file_url(&c.t.0.join("없는.git"))]);
    let st = svc.fetch("r", true).await;
    assert!(st.error.is_some());
    assert_eq!(host.approvals().len(), 1, "승인이 늘지 않는다");
    // 사이드카가 되돌려 준 승인만이 자동 fetch 의 근거다: 서비스 밖에서 승인 맵을 바꿀 길은 apply_prefs 뿐이다(gen 이 앞서야 한다)
    assert!(svc.apply_prefs(prefs(5, 5.0, &[("r", &ins.remotes_fingerprint)], &[])));
    assert!(!svc.apply_prefs(prefs(5, 5.0, &[("r", FP_A)], &[])));
    assert!(!svc.apply_prefs(prefs(4, 5.0, &[("r", FP_A)], &[])));
}

#[tokio::test]
async fn 실제_git_확인_중에_위험_설정이_늘어나면_실행_직전_재검사에서_멈춘다() {
    let c = Case::new();
    c.git(&["config", "protocol.ext.allow", "never"]);
    let (svc, host, _, _) = real(&c);
    let before = c.git(&["rev-parse", "origin/main"]);
    c.push_commit("second");
    let (repo, payload) = (c.repo.clone(), c.payload());
    host.on_confirm(move |_| {
        // 사용자가 허용을 누르는 사이(또는 첫 검사 직후) 저장소 설정이 바뀐다
        raw_git(&repo, &["config", "core.sshCommand", &payload]);
        Reply::Now(Some(0))
    });
    let st = svc.fetch("r", true).await;
    let b = st.blocked.expect("차단");
    assert!(b.confirmable);
    assert!(b.keys.contains(&"core.sshcommand".to_string()), "{b:?}");
    assert!(!c.marker_exists());
    assert_eq!(c.git(&["rev-parse", "origin/main"]), before);
    assert!(host.approvals().is_empty());
}

#[tokio::test]
async fn 실제_git_확인_중에_remote_url_이_ext_로_바뀌면_허용과_무관하게_거부한다() {
    let c = Case::new();
    c.git(&["config", "protocol.ext.allow", "never"]);
    let (svc, host, _, _) = real(&c);
    let (repo, marker) = (c.repo.clone(), posix(&c.marker));
    host.on_confirm(move |_| {
        raw_git(&repo, &["config", "remote.origin.url", &format!("ext::sh -c \"touch {marker}\"")]);
        Reply::Now(Some(0))
    });
    let st = svc.fetch("r", true).await;
    assert_eq!(st.blocked.map(|b| b.confirmable), Some(false));
    assert!(!c.marker_exists());
    assert!(host.approvals().is_empty());
}

// ── 401 서버(127.0.0.1) ──

struct Srv {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

/// 모든 요청에 401 Basic 챌린지로 답하는 루프백 서버
async fn auth_server() -> Srv {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("루프백 바인드");
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 64 * 1024 {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let out = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Srv { port, task }
}
