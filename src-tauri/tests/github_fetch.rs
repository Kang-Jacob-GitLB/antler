//! GitHub 원격 전용 저장소(github kind)의 fetch·감시를 Rust 가 맡는지(Phase 6b PR 7) 통합 테스트.
//! 실제 git 을 띄우되 네트워크는 쓰지 않는다 — `file:///` 소스 저장소와 127.0.0.1 의 가짜 HTTP 서버뿐이다(실제 GitHub 에는 닿지 않는다).
//!
//! 보는 것:
//! - fetch: 첫 fetch 의 성공·HEAD 맞추기·기본 브랜치 기록 부탁 1회, 설정이 달라진 캐시·어긋난 레코드의 차단(확인 창 없이, 네트워크 없이),
//!   401·403·404 는 인증 정지(백오프 아님)이고 로그인 변경이 다시 열며, 5xx 는 네트워크 실패, 리다이렉트는 따라가지 않고 토큰이 새지 않는다.
//! - 60분 제한: 아직 ref 가 없는 첫 fetch 는 60분, 아니면 60초이고 큐 슬롯을 얻은 뒤부터 잰다(멈춘 시계 + 가짜 실행기).
//! - 소유 판정: 사이드카 env(`ANTLER_RUST_FETCH_KINDS`)와 미러의 `resolve_fetch` 가 같은 술어 — 이중 fetch·이중 감시·무감시가 없다(`ANTLER_RUST_GIT=0`·서비스 없음 포함).
//! - 감시: bare 캐시 폴더의 ref 변화를 notify 로 알아챈다.
//! - `repos:remove`: 진행 중인 fetch 를 멈추고 끝나기를 기다린 뒤라 캐시 폴더가 바로 지워지고, 감시·새 fetch 도 막힌다.
//! - 비밀: 토큰은 어떤 이벤트·오류에도 없다.
//!
//! 격리: 이 바이너리가 시작될 때(`isolate`) 깨끗한 전역 설정과 `GIT_CONFIG_NOSYSTEM` 을 넣고 프록시·askpass 환경을 비운다.

mod ghcache;

use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use antler_lib::api_host::GithubHost;
use antler_lib::git::exec::{ExecError, GitExecutor, GithubExecutor, LocalExecutor, NetConfig, RunOpts, RunResult};
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::guard::classify_failure;
use antler_lib::git::fetch::run::GitRunner;
use antler_lib::git::fs_watch::NotifyDirs;
use antler_lib::git::fetch::service::{ApprovalKind, FetchHost, FetchService, FetchStatus, HostEvent};
use antler_lib::git::github_cache::{expected_github_config, init_github_repo_for_test, remove_github_cache, GithubCachePath};
use antler_lib::git::log_service::NoteOp;
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use antler_lib::mirror::{GhLink, MirrorRepo, RepoMirror, Resolved, Target};
use antler_lib::net::redact::{git_basic_token, SecretString};
use antler_lib::router::{rust_fetch_kinds, Routed, Router};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use futures_util::future::{BoxFuture, FutureExt};
use ghcache::{link, make_source, raw_git, Cache, Source, Stub, StubServer};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    std::env::set_var("GIT_CONFIG_GLOBAL", Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    for k in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL", "GIT_TERMINAL_PROMPT"] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
}

/// 비노출 시험이 찾는 가짜 토큰
const TOKEN: &str = "ghp_fetch_canary_token_0123456789";
const G: &str = "g";
const REMOVING: &str = "저장소를 삭제하는 중입니다";

// ---------------------------------------------------------------------------------------------------------------------
// 도구
// ---------------------------------------------------------------------------------------------------------------------

/// 조건이 참이 될 때까지(상한 20초) 기다린다. 고정 sleep 으로 가정하지 않는다
async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < until, "시간 초과: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Status(FetchStatus),
    Changed(String),
    Note(String, String),
    Branch(String, String),
    Approve,
    Confirm,
}

#[derive(Clone, Default)]
struct Host(Arc<Mutex<Vec<Ev>>>);

impl Host {
    fn push(&self, e: Ev) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(e);
    }

    fn events(&self) -> Vec<Ev> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn branches(&self) -> Vec<(String, String)> {
        self.events().into_iter().filter_map(|e| if let Ev::Branch(r, b) = e { Some((r, b)) } else { None }).collect()
    }

    fn last_status(&self) -> Option<FetchStatus> {
        self.events().into_iter().rev().find_map(|e| if let Ev::Status(s) = e { Some(s) } else { None })
    }

    /// 모든 이벤트의 직렬화 — 토큰·그 base64 가 어디에도 없어야 한다
    fn assert_no_secret(&self) {
        let all = format!("{:?}", self.events());
        assert!(!all.contains(TOKEN) && !all.contains(&git_basic_token(TOKEN)), "토큰이 이벤트에 있다: {all}");
    }
}

impl FetchHost for Host {
    fn emit(&self, event: HostEvent) {
        match event {
            HostEvent::Status(s) => self.push(Ev::Status(s)),
            HostEvent::RepoChanged(id) => self.push(Ev::Changed(id)),
        }
    }

    fn confirm(&self, _repo_id: &str, _confirm: &ConfirmBox) -> impl Future<Output = Option<i64>> + Send {
        self.push(Ev::Confirm);
        async { None }
    }

    fn approve(&self, _kind: ApprovalKind, _repo_id: &str, _fingerprint: &str) {
        self.push(Ev::Approve);
    }

    fn note(&self, op: NoteOp, repo_id: &str) {
        self.push(Ev::Note(op.as_str().to_string(), repo_id.to_string()));
    }

    fn github_branch(&self, repo_id: &str, branch: &str) {
        self.push(Ev::Branch(repo_id.to_string(), branch.to_string()));
    }
}

/// 저장소도 클라이언트 ID 도 없는 질의(오프라인)
struct NoRepos;

impl RepoQuery for NoRepos {
    fn gh_repo<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        async { Ok(None) }.boxed()
    }
    fn gh_remotes<'a>(&'a self, _: &'a str, _: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        async { Ok(Vec::new()) }.boxed()
    }
    fn pushed_shas_by_author<'a>(&'a self, _: &'a str, _: &'a str, _: usize, _: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        async { Ok(Vec::new()) }.boxed()
    }
    fn invalidate_gh_remotes(&self) {}
    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        async { None }.boxed()
    }
    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        async { Vec::new() }.boxed()
    }
}

fn gh_host() -> GithubHost {
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::new())));
    let service = GitHubService::new(vault, Arc::new(NoRepos), None, ServiceOptions::default()).expect("운영 클라이언트");
    GithubHost { service, opener: Arc::new(|_| Ok(())) }
}

fn token_exec(cache: &Cache, protocol: &str) -> GithubExecutor {
    GithubExecutor::new("git", cache.cache_dir.clone(), cache.hooks.clone(), Arc::new(|| Some(SecretString::new(TOKEN))), Arc::new(Vec::new)).with_protocol_for_test(protocol)
}

fn github_record(id: &str, path: &str) -> MirrorRepo {
    MirrorRepo { id: id.into(), kind: "github".into(), path: path.into(), distro: None, ssh_host: None }
}

type Svc = FetchService<Host, GitExecutor>;

/// 캐시 폴더(init 만 한 빈 bare 저장소)·미러·실행기·fetch 서비스·라우터 한 벌. 원격 URL 의 출처는 시험이 정한다(`file:///…`·`http://127.0.0.1:…`)
struct Rig {
    cache: Cache,
    dir: GithubCachePath,
    mirror: RepoMirror,
    exec: GithubExecutor,
    host: Host,
    svc: Svc,
    router: Arc<Router>,
    /// 감시의 이벤트원(notify) — 살아 있는 디렉터리 핸들 수를 시험이 읽는다
    dirs: NotifyDirs,
    /// 소스 저장소(file 출처일 때만)
    src: Option<Source>,
}

impl Rig {
    /// `file:///` 소스 저장소에서 받는 캐시. `branch` 는 레코드의 기본 브랜치(init 이 HEAD 를 미리 가리킨다)
    async fn file(branch: Option<&str>) -> Self {
        let src = make_source("octocat", "hello");
        let origin = src.origin();
        Self::build("file", origin, Some(src), branch).await
    }

    /// 가짜 HTTP 서버가 출처인 캐시(토큰 헤더가 실제 요청에 실린다)
    async fn http(server: &StubServer) -> Self {
        Self::build("http", server.origin(), None, None).await
    }

    async fn build(protocol: &str, origin: String, src: Option<Source>, branch: Option<&str>) -> Self {
        let cache = Cache::new();
        let dir = cache.repo("octocat", "hello");
        // 준비(init)는 네트워크를 쓰지 않는다 — 실행기의 프로토콜과 무관하다
        let prep = cache.executor("file", None);
        init_github_repo_for_test(&prep, &dir, &link("octocat", "hello", branch), &origin, None).await.expect("init");
        let mirror = RepoMirror::default();
        let links = HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "hello".into() })]);
        let branches: HashMap<String, String> = branch.map(|b| (G.to_string(), b.to_string())).into_iter().collect();
        assert!(mirror.apply_with_branches(1, vec![github_record(G, dir.as_str())], links, branches));
        mirror.set_github(true);
        let exec = token_exec(&cache, protocol);
        let host = Host::default();
        let runner = exec.clone();
        let make_runner = move |t: &Target| match t {
            Target::Github { .. } => Arc::new(GitExecutor::Github(runner.clone())),
            _ => Arc::new(GitExecutor::Local(LocalExecutor::default())),
        };
        let svc = FetchService::new(host.clone(), mirror.clone(), make_runner, Some(cache.hooks.clone())).with_github_cache(cache.cache_dir.clone()).with_github_origin_for_test(origin.clone());
        let dirs = NotifyDirs::new();
        let router = Router::new(true, false, mirror.clone()).with_dir_watch(Arc::new(dirs.clone()));
        router.set_github(gh_host());
        router.set_github_cache(exec.clone());
        router.set_fetch(Arc::new(svc.clone()));
        Rig { cache, dir, mirror, exec, host, svc, router: Arc::new(router), dirs, src }
    }

    fn git(&self, args: &[&str]) -> String {
        raw_git(Path::new(self.dir.as_str()), args)
    }

    fn refs(&self) -> String {
        self.git(&["for-each-ref", "--format=%(refname)"])
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.router.shutdown();
    }
}

async fn route_done(r: &Router, ch: &str, args: Value) -> Value {
    match r.route(ch, &args).await {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("{ch} {args} 가 사이드카로 갔다"),
    }
}

fn is_sidecar(r: &Routed) -> bool {
    matches!(r, Routed::Sidecar)
}

// ---------------------------------------------------------------------------------------------------------------------
// fetch: 성공·HEAD·기본 브랜치
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 첫_fetch_는_받고_head_를_맞추고_기본_브랜치_기록을_한_번_부탁한다() {
    let rig = Rig::file(None).await;
    let status = rig.svc.fetch(G, false).await;
    assert_eq!((status.error.clone(), status.blocked.clone(), status.auth_paused), (None, None, false), "{status:?}");
    assert!(status.last_ok_at.is_some() && !status.fetching && !status.no_remote);
    let src = rig.src.as_ref().unwrap();
    assert_eq!(rig.git(&["rev-parse", "refs/remotes/origin/main"]), src.second);
    // HEAD 가 받아 온 기본 브랜치를 가리킨다(그래서 로그·사이드바가 열린다)
    assert_eq!(rig.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/main");
    assert_eq!(rig.git(&["rev-parse", "HEAD"]), src.second);
    // 레코드의 기본 브랜치가 비어 있었으므로 기록을 1회 부탁한다. ref 가 바뀌었으니 사이드카의 기억을 먼저 버리고(note) 그다음 repo:changed 다
    assert_eq!(rig.host.branches(), [(G.to_string(), "main".to_string())]);
    let evs = rig.host.events();
    let note = evs.iter().position(|e| *e == Ev::Note("fetchChanged".into(), G.into())).expect("note");
    let changed = evs.iter().position(|e| *e == Ev::Changed(G.into())).expect("repo:changed");
    assert!(note < changed);
    // 허용 목록·확인 창·승인은 이 갈래에 없다
    assert!(!evs.contains(&Ev::Confirm) && !evs.contains(&Ev::Approve));
    // 설정은 fetch 뒤에도 정확히 일치한다(다음 fetch 의 검사가 통과한다) — 두 번째 fetch 는 바뀐 것이 없다
    let again = rig.svc.fetch(G, false).await;
    assert!(again.error.is_none() && again.blocked.is_none(), "{again:?}");
    // 사이드카가 기록해 미러가 새 브랜치를 알게 되면(PR 9 전까지 사이드카가 쓴다) 같은 브랜치를 다시 부탁하지 않는다. 미러가 아직 모르는 동안은 한 번 더 갈 수 있다(사이드카가 같은 값이면 아무것도 하지 않는다)
    let (links, branches) = (HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "hello".into() })]), HashMap::from([(G.to_string(), "main".to_string())]));
    assert!(rig.mirror.apply_with_branches(2, vec![github_record(G, rig.dir.as_str())], links, branches));
    let again = rig.svc.fetch(G, false).await;
    assert!(again.error.is_none() && again.blocked.is_none(), "{again:?}");
    let count = rig.host.branches().len();
    let third = rig.svc.fetch(G, false).await;
    assert!(third.error.is_none());
    assert_eq!(rig.host.branches().len(), count, "미러가 아는 브랜치와 같으면 부탁하지 않는다");
    rig.host.assert_no_secret();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 레코드의_기본_브랜치와_같으면_기록을_부탁하지_않는다() {
    let rig = Rig::file(Some("main")).await;
    // init 이 HEAD 를 미리 가리켰고, 레코드(미러)가 같은 브랜치를 안다
    assert_eq!(rig.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/main");
    let status = rig.svc.fetch(G, false).await;
    assert!(status.error.is_none() && status.blocked.is_none(), "{status:?}");
    assert!(rig.host.branches().is_empty(), "{:?}", rig.host.branches());
    assert_eq!(rig.git(&["rev-parse", "HEAD"]), rig.src.as_ref().unwrap().second);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 원격에서_브랜치_이름이_바뀌면_바뀐_이름을_기록해_달라고_부탁한다() {
    let rig = Rig::file(Some("main")).await;
    assert!(rig.svc.fetch(G, false).await.error.is_none());
    raw_git(&rig.src.as_ref().unwrap().dir("octocat", "hello"), &["branch", "-m", "main", "trunk"]);
    let status = rig.svc.fetch(G, false).await;
    assert!(status.error.is_none(), "{status:?}");
    assert_eq!(rig.git(&["symbolic-ref", "HEAD"]), "refs/remotes/origin/trunk");
    assert_eq!(rig.host.branches(), [(G.to_string(), "trunk".to_string())]);
}

// ---------------------------------------------------------------------------------------------------------------------
// fetch 차단: 설정 정확 일치·레코드 일치 — 확인 창 없이, 네트워크 없이
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 설정이_달라진_캐시는_확인_없이_차단하고_받지_않는다() {
    type Tamper = Box<dyn Fn(&Rig)>;
    let cases: Vec<(&str, Tamper, &str)> = vec![
        ("core.sshCommand", Box::new(|r| drop(r.git(&["config", "--local", "core.sshCommand", "evil"]))), "core.sshcommand"),
        ("remote.origin.url", Box::new(|r| drop(r.git(&["config", "--local", "remote.origin.url", "https://evil.example/octocat/hello.git"]))), "remote.origin.url"),
        (
            "include.path",
            Box::new(|r| {
                fs::write(Path::new(r.dir.as_str()).join("extra.cfg"), "[remote \"origin\"]\n\tpromisor = false\n").unwrap();
                r.git(&["config", "--local", "include.path", "extra.cfg"]);
            }),
            "include.path",
        ),
        ("remote 가 둘", Box::new(|r| drop(r.git(&["remote", "add", "other", "https://example.com/x.git"]))), "remote"),
        ("promisor", Box::new(|r| drop(r.git(&["config", "--local", "remote.origin.promisor", "false"]))), "remote.origin.promisor"),
    ];
    for (name, tamper, key) in cases {
        let rig = Rig::file(None).await;
        tamper(&rig);
        // 수동 새로고침이어도 확인 창은 없다
        let status = rig.svc.fetch(G, true).await;
        let blocked = status.blocked.clone().unwrap_or_else(|| panic!("{name}: 막히지 않았다 {status:?}"));
        assert!(!blocked.confirmable, "{name}");
        assert!(blocked.keys.join(",").contains(key), "{name}: {:?}", blocked.keys);
        assert!(status.last_ok_at.is_none() && status.error.is_none(), "{name}: {status:?}");
        assert!(!rig.host.events().contains(&Ev::Confirm) && !rig.host.events().contains(&Ev::Approve), "{name}");
        assert_eq!(rig.refs(), "", "{name}: 네트워크를 쓰지 않았다(받은 ref 가 없다)");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 캐시_경로가_저장소와_맞지_않는_레코드는_일반_흐름으로_가지_않고_차단한다() {
    let rig = Rig::file(None).await;
    let other = rig.cache.repo("someone", "else");
    let outside = std::env::temp_dir().join(format!("antler-gh-outside-{}", std::process::id()));
    fs::create_dir_all(&outside).unwrap();
    let ok_link = GhLink { owner: "octocat".into(), repo: "hello".into() };
    // (경로, 링크) — 다른 저장소의 캐시를 가리키거나, 캐시 밖이거나, 링크가 없다
    let cases: Vec<(String, Option<GhLink>)> = vec![
        (other.as_str().to_string(), Some(ok_link.clone())),
        (outside.to_string_lossy().into_owned(), Some(ok_link.clone())),
        (rig.dir.as_str().to_string(), None),
    ];
    for (i, (path, l)) in cases.into_iter().enumerate() {
        let links: HashMap<String, GhLink> = l.map(|l| (G.to_string(), l)).into_iter().collect();
        assert!(rig.mirror.apply_with_links(2 + i as u64, vec![github_record(G, &path)], links));
        let status = rig.svc.fetch(G, true).await;
        let blocked = status.blocked.clone().unwrap_or_else(|| panic!("{path}: 막히지 않았다 {status:?}"));
        assert!(!blocked.confirmable && blocked.keys.join(",").contains("앱 캐시 경로가 저장소와 맞지 않음"), "{path}: {blocked:?}");
        assert!(status.last_ok_at.is_none() && status.error.is_none(), "{path}: {status:?}");
        assert!(!rig.host.events().contains(&Ev::Confirm));
    }
    assert_eq!(rig.refs(), "");
    let _ = fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------------------------------------------------
// 인증 정지·재개, 네트워크 실패, 리다이렉트: 127.0.0.1 가짜 서버
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 인증_실패는_백오프하지_않고_멈추며_로그인_변경이_다시_연다() {
    // 404 는 본문 없는 가짜 서버에서는 "repository '…' not found" 라 문구 분류에 걸리지 않는다 — 실제 GitHub 의 "Repository not found." 문구는 `실패_분류_…` 시험이 본다
    for code in [401u16, 403] {
        let server = StubServer::start(Stub::Status(code));
        let rig = Rig::http(&server).await;
        // 저장소를 열면(선택) 받은 것이 없어 바로 fetch 한다 — 자동 주기 설정이 없어도 된다
        rig.svc.set_active(Some(G));
        wait_for("인증 정지", || rig.host.last_status().is_some_and(|s| s.auth_paused)).await;
        let paused = rig.host.last_status().unwrap();
        assert!(paused.error.is_some() && !paused.fetching && paused.blocked.is_none(), "{code}: {paused:?}");
        let seen = server.requests().len();
        assert!(seen > 0, "{code}: 요청이 서버에 닿지 않았다");
        // 토큰 헤더는 서버에 간다(github.com 이 아닌 호스트에는 가지 않는다 — 그래서 가짜 서버로는 헤더가 없다)
        assert!(server.requests().iter().all(|r| !r.to_ascii_lowercase().contains("authorization")), "{code}");

        // 로그인 상태가 바뀌었다: 멈춘 상태가 풀리고, 열고 있는 저장소가 비어 있으면 바로 다시 시도한다
        rig.svc.resume_github_auth();
        wait_for("다시 시도", || server.requests().len() > seen).await;
        wait_for("다시 인증 정지", || rig.host.last_status().is_some_and(|s| s.auth_paused && !s.fetching)).await;
        let evs = rig.host.events();
        assert!(evs.iter().any(|e| matches!(e, Ev::Status(s) if !s.auth_paused && s.error.is_some() && !s.fetching)), "{code}: 정지가 풀린 상태 이벤트가 없다");
        rig.host.assert_no_secret();
        assert!(!evs.contains(&Ev::Confirm) && !evs.contains(&Ev::Approve));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 서버_오류는_네트워크_실패라_정지하지_않는다() {
    let server = StubServer::start(Stub::Status(500));
    let rig = Rig::http(&server).await;
    let status = rig.svc.fetch(G, false).await;
    assert!(status.error.is_some() && !status.auth_paused && status.blocked.is_none(), "{status:?}");
    // 수동 새로고침도 같다
    let manual = rig.svc.fetch(G, true).await;
    assert!(manual.error.is_some() && !manual.auth_paused, "{manual:?}");
    rig.host.assert_no_secret();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 리다이렉트는_따라가지_않고_토큰이_다른_곳으로_가지_않는다() {
    let target = StubServer::start(Stub::Status(404));
    let front = StubServer::start(Stub::Redirect(target.port));
    let rig = Rig::http(&front).await;
    let status = rig.svc.fetch(G, true).await;
    assert!(status.error.is_some(), "{status:?}");
    assert!(!front.requests().is_empty(), "가짜 서버가 git 의 요청을 받지 못했다 — 시험이 공허하다");
    assert!(target.requests().is_empty(), "리다이렉트를 따라갔다: {:?}", target.requests());
    for req in front.requests().iter().chain(target.requests().iter()) {
        assert!(!req.to_ascii_lowercase().contains("authorization"), "{req}");
    }
    rig.host.assert_no_secret();
    let text = format!("{status:?}");
    assert!(!text.contains(TOKEN) && !text.contains(&git_basic_token(TOKEN)));
}

#[test]
fn 실패_분류는_github_일_때만_권한_없음을_인증으로_본다() {
    use antler_lib::git::fetch::guard::FetchFailure::{Auth, Network};
    for msg in [
        "remote: Repository not found.\nfatal: repository 'https://github.com/o/r.git/' not found",
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        "fatal: unable to access 'https://github.com/o/r.git/': The requested URL returned error: 403",
        "remote: Invalid username or token. Password authentication is not supported for Git operations.",
    ] {
        assert_eq!(classify_failure(msg, true), Auth, "{msg}");
    }
    assert_eq!(classify_failure("remote: Repository not found.", false), Network);
    assert_eq!(classify_failure("The requested URL returned error: 403", false), Network);
    assert_eq!(classify_failure("fatal: could not read Username", false), Auth);
    assert_eq!(classify_failure("Could not resolve host: github.com", true), Network);
}

// ---------------------------------------------------------------------------------------------------------------------
// 60분 제한: 멈춘 시계 + 가짜 실행기
// ---------------------------------------------------------------------------------------------------------------------

/// 프로세스를 띄우지 않는 github 실행기 대역. 설정·remote 는 앱이 만든 모양 그대로이고, ref 유무는 시험이 정하며, fetch 는 취소될 때까지 끝나지 않는다
struct Scripted {
    has_refs: bool,
}

impl GitRunner for Scripted {
    fn kind(&self) -> &'static str {
        "github"
    }

    fn run(&self, repo_path: &str, args: &[String], opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        let ok = |stdout: String| RunResult { stdout: stdout.into_bytes(), stderr: String::new(), code: 0 };
        let cancel = opts.cancel.clone();
        let reply: Option<RunResult> = if args.iter().any(|a| a == "fetch") {
            None
        } else if args.first().map(String::as_str) == Some("config") {
            Some(ok(expected_github_config(&url_of(repo_path)).iter().map(|(k, v)| format!("local\0{k}\n{v}\0")).collect()))
        } else if args.first().map(String::as_str) == Some("remote") {
            Some(ok("origin\n".to_string()))
        } else if args.contains(&"--count=1".to_string()) {
            Some(ok(if self.has_refs { "refs/remotes/origin/main\n".to_string() } else { String::new() }))
        } else {
            Some(ok(String::new()))
        };
        async move {
            match reply {
                Some(r) => Ok(r),
                None => {
                    // 취소될 때까지 끝나지 않는다
                    match cancel {
                        Some(c) => c.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                    Err(ExecError::Aborted)
                }
            }
        }
    }
}

struct ScriptRig {
    _cache: Cache,
    svc: FetchService<Host, Scripted>,
}

fn scripted_rig(has_refs: bool, repos: &[&str]) -> ScriptRig {
    let cache = Cache::new();
    let mirror = RepoMirror::default();
    let mut records = Vec::new();
    let mut links = HashMap::new();
    for (i, id) in repos.iter().enumerate() {
        let name = format!("repo{i}");
        let dir = cache.repo("octocat", &name);
        records.push(github_record(id, dir.as_str()));
        links.insert((*id).to_string(), GhLink { owner: "octocat".into(), repo: name });
    }
    assert!(mirror.apply_with_links(1, records, links));
    mirror.set_github(true);
    let host = Host::default();
    // 프로덕션 검사 그대로(`https://github.com/<owner>/<repo>.git` 고정)라 가짜 실행기가 `config` 로 돌려줄 URL 은 대상 경로의 저장소 이름에서 만든다(`url_of`)
    let svc = FetchService::new(host.clone(), mirror, move |_: &Target| Arc::new(Scripted { has_refs }), Some(cache.hooks.clone())).with_github_cache(cache.cache_dir.clone());
    ScriptRig { _cache: cache, svc }
}

/// 저장소 폴더(`.../octocat/repoN.git`)에서 앱이 기대하는 원격 URL
fn url_of(repo_path: &str) -> String {
    let name = Path::new(repo_path).file_name().unwrap().to_string_lossy().trim_end_matches(".git").to_string();
    format!("https://github.com/octocat/{name}.git")
}

#[tokio::test(start_paused = true)]
async fn 첫_fetch_는_60분_아니면_60초이고_시간_초과_문구에_실제_제한이_든다() {
    // ref 가 없다 → 60분
    let rig = scripted_rig(false, &[G]);
    let f = rig.svc.fetch(G, true);
    tokio::time::sleep(Duration::from_secs(59 * 60)).await;
    assert!(rig.svc.status(G).fetching, "59분에는 아직 받는 중이다");
    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    let status = f.await;
    assert_eq!(status.error.as_deref(), Some("시간 초과(3600초)"), "{status:?}");
    assert!(!status.fetching && !status.auth_paused, "시간 초과는 네트워크 실패다(백오프): {status:?}");

    // ref 가 있다 → 60초
    let rig = scripted_rig(true, &[G]);
    let f = rig.svc.fetch(G, true);
    tokio::time::sleep(Duration::from_secs(50)).await;
    assert!(rig.svc.status(G).fetching);
    tokio::time::sleep(Duration::from_secs(11)).await;
    let status = f.await;
    assert_eq!(status.error.as_deref(), Some("시간 초과(60초)"), "{status:?}");
}

#[tokio::test(start_paused = true)]
async fn 제한은_큐_슬롯을_얻은_뒤부터_잰다() {
    let (a, b) = ("a", "b");
    let rig = scripted_rig(true, &[a, b]);
    // a 가 슬롯을 쥐고(멈춘 채) b 는 같은 큐에서 기다린다 — github 소스는 큐가 하나다
    let fa = rig.svc.fetch(a, true);
    tokio::time::sleep(Duration::from_millis(10)).await;
    let fb = rig.svc.fetch(b, true);
    tokio::time::sleep(Duration::from_secs(50)).await;
    assert!(rig.svc.status(a).fetching && rig.svc.status(b).fetching);
    // a 를 취소하면 슬롯이 b 로 간다(t≈50). 요청 시점부터 쟀다면 b 는 t=60 에 시간 초과가 된다
    rig.svc.cancel(a);
    let sa = fa.await;
    assert!(sa.error.is_none(), "취소는 오류가 아니다: {sa:?}");
    tokio::time::sleep(Duration::from_secs(59)).await; // t≈109
    assert!(rig.svc.status(b).fetching, "슬롯을 기다린 시간이 제한을 갉아먹었다");
    tokio::time::sleep(Duration::from_secs(3)).await; // 슬롯을 얻은 뒤 60초가 지났다
    let sb = fb.await;
    assert_eq!(sb.error.as_deref(), Some("시간 초과(60초)"), "{sb:?}");
}

// ---------------------------------------------------------------------------------------------------------------------
// 실행 직전 재검사·네트워크 설정 대기: 가짜 실행기(명령 기록)
// ---------------------------------------------------------------------------------------------------------------------

struct ProbeInner {
    /// fetch 가 돈 뒤의 `config` 를 앱이 만든 모양이 아니게 돌려준다(검사 뒤 설정이 바뀐 상황)
    tamper_after_fetch: bool,
    net: Option<NetConfig>,
    fetched: AtomicBool,
    commands: Mutex<Vec<String>>,
    /// fetch 가 돌 때 실행기가 읽은 네트워크 설정(fetch 가 돌지 않았으면 `None`)
    net_at_fetch: Mutex<Option<Vec<(String, String)>>>,
}

#[derive(Clone)]
struct Probe(Arc<ProbeInner>);

impl Probe {
    fn commands(&self) -> Vec<String> {
        self.0.commands.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl GitRunner for Probe {
    fn kind(&self) -> &'static str {
        "github"
    }

    fn run(&self, repo_path: &str, args: &[String], _opts: &RunOpts) -> impl Future<Output = Result<RunResult, ExecError>> + Send {
        let inner = &self.0;
        inner.commands.lock().unwrap_or_else(PoisonError::into_inner).push(args.join(" "));
        let ok = |stdout: String| RunResult { stdout: stdout.into_bytes(), stderr: String::new(), code: 0 };
        let result = if args.iter().any(|a| a == "fetch") {
            inner.fetched.store(true, Ordering::SeqCst);
            *inner.net_at_fetch.lock().unwrap_or_else(PoisonError::into_inner) = Some(inner.net.as_ref().map(NetConfig::get).unwrap_or_default());
            ok(String::new())
        } else if args.first().map(String::as_str) == Some("config") {
            let url = if inner.tamper_after_fetch && inner.fetched.load(Ordering::SeqCst) { "https://evil.example/x.git".to_string() } else { url_of(repo_path) };
            ok(expected_github_config(&url).iter().map(|(k, v)| format!("local\0{k}\n{v}\0")).collect())
        } else if args.first().map(String::as_str) == Some("remote") {
            ok("origin\n".to_string())
        } else if args.contains(&"--count=1".to_string()) {
            ok("refs/remotes/origin/main\n".to_string())
        } else {
            ok(String::new())
        };
        async move { Ok(result) }
    }
}

fn probe_rig(tamper_after_fetch: bool, net: Option<NetConfig>) -> (Cache, FetchService<Host, Probe>, Probe) {
    let cache = Cache::new();
    let dir = cache.repo("octocat", "repo0");
    let mirror = RepoMirror::default();
    let links = HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "repo0".into() })]);
    assert!(mirror.apply_with_links(1, vec![github_record(G, dir.as_str())], links));
    mirror.set_github(true);
    let probe = Probe(Arc::new(ProbeInner { tamper_after_fetch, net: net.clone(), fetched: AtomicBool::new(false), commands: Mutex::default(), net_at_fetch: Mutex::default() }));
    let runner = probe.clone();
    let mut svc = FetchService::new(Host::default(), mirror, move |_: &Target| Arc::new(runner.clone()), Some(cache.hooks.clone())).with_github_cache(cache.cache_dir.clone());
    if let Some(net) = net {
        svc = svc.with_github_net_ready(net);
    }
    (cache, svc, probe)
}

#[tokio::test]
async fn 실행_직전_재검사가_통과하면_head_를_맞추고_어긋나면_맞추지_않고_차단한다() {
    let touches_head = |p: &Probe| p.commands().iter().any(|c| c.contains("refs/remotes/origin/HEAD") || c.contains("ls-remote"));
    // 설정이 그대로면 HEAD 맞추기까지 간다
    let (_cache, svc, probe) = probe_rig(false, None);
    let status = svc.fetch(G, true).await;
    assert!(status.blocked.is_none() && status.error.is_none(), "{status:?}");
    assert!(touches_head(&probe), "재검사가 통과하면 HEAD 를 맞춘다: {:?}", probe.commands());

    // fetch 뒤 설정이 어긋나면: HEAD 맞추기(ls-remote 포함)를 하지 않고 차단 상태가 된다
    let (_cache, svc, probe) = probe_rig(true, None);
    let status = svc.fetch(G, true).await;
    assert!(status.blocked.as_ref().is_some_and(|b| !b.confirmable), "{status:?}");
    assert!(!touches_head(&probe), "재검사가 실패했는데 HEAD 를 맞췄다: {:?}", probe.commands());
    // 시그니처는 검사보다 앞이고, 검사(`config`)와 fetch 사이에는 다른 git 호출이 없다
    let cmds = probe.commands();
    let at = cmds.iter().position(|c| c.split(' ').any(|a| a == "fetch")).expect("fetch");
    assert!(cmds[at - 1].contains("remote"), "검사 직후에 fetch 가 붙는다: {cmds:?}");
}

#[tokio::test]
async fn 첫_fetch_는_네트워크_설정_읽기가_끝나기를_기다렸다가_채워진_값을_쓴다() {
    let net = NetConfig::default();
    let (_cache, svc, probe) = probe_rig(false, Some(net.clone()));
    let running = tokio::spawn({
        let svc = svc.clone();
        async move { svc.fetch(G, true).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(probe.0.net_at_fetch.lock().unwrap_or_else(PoisonError::into_inner).is_none(), "설정을 읽기 전에 fetch 가 돌았다");
    net.set(vec![("http.proxy".to_string(), "http://proxy.example:8080".to_string())]);
    let status = running.await.expect("join");
    assert!(status.error.is_none() && status.blocked.is_none(), "{status:?}");
    let seen = probe.0.net_at_fetch.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(seen, Some(vec![("http.proxy".to_string(), "http://proxy.example:8080".to_string())]));
}

#[tokio::test(start_paused = true)]
async fn 네트워크_설정을_끝내_못_읽어도_5초_뒤에는_그_키_없이_fetch_한다() {
    let net = NetConfig::default();
    let (_cache, svc, probe) = probe_rig(false, Some(net));
    let started = tokio::time::Instant::now();
    let status = svc.fetch(G, true).await;
    assert!(status.error.is_none(), "{status:?}");
    assert!(started.elapsed() >= Duration::from_secs(5) && started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(probe.0.net_at_fetch.lock().unwrap_or_else(PoisonError::into_inner).clone(), Some(Vec::new()));
}

// ---------------------------------------------------------------------------------------------------------------------
// 소유 판정: 조회·fetch·감시가 같은 술어
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn 소유_판정은_종류_목록과_일치하고_이중_fetch_도_무감시도_없다() {
    let cache = Cache::new();
    let dir = cache.repo("octocat", "hello");
    let links = || HashMap::from([(G.to_string(), GhLink { owner: "octocat".into(), repo: "hello".into() })]);
    // (ANTLER_RUST_GIT, ANTLER_RUST_API, 캐시 실행기 주입, GitHub 서비스 주입)
    for (git_on, api_on, with_exec, with_service) in [
        (true, true, true, true),
        (false, true, true, true),   // ANTLER_RUST_GIT=0
        (true, false, true, true),   // ANTLER_RUST_API=0
        (true, true, false, true),   // 실행기 없음
        (true, true, true, false),   // GitHub 서비스 없음(토큰의 출처가 없다)
        (false, false, false, false),
    ] {
        let mirror = RepoMirror::default();
        assert!(mirror.apply_with_links(1, vec![github_record(G, dir.as_str())], links()));
        let router = Router::new(git_on, false, mirror.clone()).with_api(api_on);
        if with_service {
            router.set_github(gh_host());
        }
        if with_exec {
            router.set_github_cache(token_exec(&cache, "file"));
        }
        // 종류 목록의 github(스위치가 켜졌을 때만) ⇔ Rust 가 맡는가(미러의 소유 판정) ⇔ 라우터의 조회 해석
        let kinds = router.enabled().then(|| rust_fetch_kinds(router.ssh_active(), router.github_cache_active()));
        let listed = kinds.is_some_and(|k| k.split(',').any(|x| x == "github"));
        let rust_owns = matches!(mirror.resolve_fetch(G), Resolved::Rust(Target::Github { .. }));
        assert_eq!(rust_owns, listed, "git={git_on} api={api_on} exec={with_exec} service={with_service}: kinds={kinds:?}");
        assert_eq!(rust_owns, matches!(mirror.resolve(G), Resolved::Rust(Target::Github { .. })), "조회 해석과 fetch 소유가 같은 스위치다");
        assert_eq!(rust_owns, git_on && api_on && with_exec && with_service);
        // 되돌리기(스위치 꺼짐)면 사이드카가 전부 맡는다
        if !git_on {
            assert!(!rust_owns);
        }
        // 라우팅: 소유하면 Rust(서비스가 없으면 오류), 아니면 사이드카
        let routed = router.route("git:fetch", &json!([G, true])).await;
        assert_eq!(is_sidecar(&routed), !rust_owns, "git:fetch git={git_on} api={api_on} exec={with_exec} service={with_service}");
        router.shutdown();
    }
    // 종류 목록: 같은 입력이면 같은 문자열(미러의 소유 판정이 한 함수다)
    assert_eq!(rust_fetch_kinds(false, true), "local,wsl,github");
    assert_eq!(rust_fetch_kinds(true, true), "local,wsl,ssh,github");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_fetch_채널은_github_저장소를_rust_서비스가_받는다() {
    let rig = Rig::file(None).await;
    let env = route_done(&rig.router, "git:fetch", json!([G, true])).await;
    assert_eq!(env["ok"], true, "{env}");
    assert_eq!(env["data"]["repoId"], G);
    assert!(env["data"]["lastOkAt"].is_u64() && env["data"]["error"].is_null() && env["data"]["blocked"].is_null(), "{env}");
    assert_eq!(rig.git(&["rev-parse", "HEAD"]), rig.src.as_ref().unwrap().second);
    // 사이드카로 보내지 않았다 — 사이드카는 이 저장소의 fetch 를 맡지 않는다
    assert!(rig.router.rust_handled() >= 1);
}

// ---------------------------------------------------------------------------------------------------------------------
// 감시: bare 캐시 폴더의 ref 변화
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rust_감시가_bare_캐시_저장소의_ref_변화를_알아챈다() {
    let rig = Rig::file(None).await;
    assert!(rig.svc.fetch(G, false).await.error.is_none());
    let changes: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = changes.clone();
    rig.router.set_change_sink(Arc::new(move |id| sink.lock().unwrap_or_else(PoisonError::into_inner).push(id.to_string())));
    // 저장소를 연다: 감시 시작 + 첫 로그 페이지(기준 시그니처가 바로 잡히게)
    assert!(is_sidecar(&rig.router.route("repos:watch", &json!([G])).await));
    let log = route_done(&rig.router, "git:log", json!([G, { "skip": 0, "count": 5 }])).await;
    assert_eq!(log["ok"], true, "{log}");
    assert_eq!(rig.router.watching().as_deref(), Some(G), "github 저장소도 Rust 가 감시한다");
    // 기준 시그니처를 잡은 뒤에야 디렉터리를 연다 — 감시가 열린 것을 보고 변화를 준다(고정 sleep 으로 가정하지 않는다)
    wait_for("감시 디렉터리 열림", || rig.dirs.live() >= 1).await;
    let empty_tree = rig.git(&["mktree"]);
    let commit = rig.git(&["commit-tree", &empty_tree, "-m", "x"]);
    rig.git(&["update-ref", "refs/remotes/origin/main", &commit]);
    wait_for("repo:changed", || !changes.lock().unwrap_or_else(PoisonError::into_inner).is_empty()).await;
    assert_eq!(changes.lock().unwrap_or_else(PoisonError::into_inner)[0], G);
}

// ---------------------------------------------------------------------------------------------------------------------
// repos:remove: fetch·감시를 멈추고 끝나기를 기다린 뒤에 폴더를 지운다
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repos_remove_는_진행_중인_fetch_와_감시를_멈추고_기다려_폴더가_바로_지워진다() {
    let server = StubServer::start(Stub::Hang);
    let rig = Rig::http(&server).await;
    // 감시 중이고(github 도 Rust 가 감시한다), 응답 없는 서버에 fetch 가 걸려 있다
    assert!(is_sidecar(&rig.router.route("repos:watch", &json!([G])).await));
    assert_eq!(rig.router.watching().as_deref(), Some(G));
    let running = rig.svc.fetch(G, false);
    wait_for("fetch 가 서버에 걸림", || !server.requests().is_empty() && rig.svc.status(G).fetching).await;
    let path = rig.dir.as_str().to_string();
    assert!(rig.exec.live_processes(&path) > 0, "git 프로세스가 떠 있다");

    let started = Instant::now();
    // 사이드카가 레코드를 지우기 전의 Rust 쪽 정리다 — 응답은 사이드카가 한다
    assert!(is_sidecar(&rig.router.route("repos:remove", &json!([G])).await));
    assert!(started.elapsed() < Duration::from_secs(4), "forget_and_wait 의 상한(5초)이 아니라 취소로 끝나야 한다: {:?}", started.elapsed());
    // fetch 는 끝났고(취소는 오류가 아니다), 프로세스·감시는 없다
    let status = running.await;
    assert!(!status.fetching && status.error.is_none() && !status.auth_paused, "{status:?}");
    assert_eq!(rig.exec.live_processes(&path), 0);
    assert_eq!(rig.router.watching(), None);
    assert_eq!(rig.dirs.live(), 0, "notify 핸들도 닫혔다");
    // 그래서 운영 삭제 경로(`remove_github_cache` — 백신·인덱서 대비 짧은 재시도 1초 안)로 지워진다. 핸들이 남아 있으면 Windows 는 재시도 내내 지우지 못한다
    assert_eq!(remove_github_cache(&rig.dir), Ok(true), "캐시 폴더 삭제");
    // 레코드가 지워지기 전의 틈: 새 fetch 도 새 감시도 폴더를 다시 쥐지 못한다
    let again = rig.svc.fetch(G, false).await;
    assert_eq!(again.error.as_deref(), Some(REMOVING), "{again:?}");
    assert!(is_sidecar(&rig.router.route("repos:watch", &json!([G])).await));
    assert_eq!(rig.router.watching(), None, "삭제 중인 저장소는 다시 감시하지 않는다");
    rig.host.assert_no_secret();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forget_and_wait_는_진행_중인_fetch_가_끝난_뒤_돌아오고_상태를_비운다() {
    let server = StubServer::start(Stub::Hang);
    let rig = Rig::http(&server).await;
    let running = rig.svc.fetch(G, false);
    wait_for("fetch 가 서버에 걸림", || !server.requests().is_empty()).await;
    rig.svc.forget_and_wait(G).await;
    // 돌아왔을 때 fetch 는 이미 끝났다(`running` 이 바로 나온다)
    let status = tokio::time::timeout(Duration::from_secs(1), running).await.expect("fetch 가 끝나 있어야 한다");
    assert!(!status.fetching);
    assert_eq!(rig.exec.live_processes(rig.dir.as_str()), 0);
    // 진행 중이 아니면 바로 돌아온다
    let t = Instant::now();
    rig.svc.forget_and_wait("nope").await;
    assert!(t.elapsed() < Duration::from_secs(1));
}

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사: 이 갈래의 경계
// ---------------------------------------------------------------------------------------------------------------------

fn read_src(rel: &str) -> String {
    // CI(Windows)의 체크아웃은 CRLF 일 수 있다 — 소스 검사는 LF 기준으로 본다
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace("\r\n", "\n")
}

/// 주석 줄과 파일 끝의 시험 모듈을 뺀 운영 코드
fn prod_code(rel: &str) -> String {
    let src = read_src(rel);
    let end = src.find("\n#[cfg(test)]").unwrap_or(src.len());
    src[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn github_갈래는_허용_목록을_쓰지_않고_일반_갈래는_github_진입점을_쓰지_않는다() {
    let github = prod_code("src/git/fetch/service/github.rs");
    let service = prod_code("src/git/fetch/service.rs");
    // github: 정확 일치 검사는 반드시, 허용 목록·확인 창·승인은 절대 아니다
    assert!(github.contains("inspect_github_repo_at("), "github 갈래가 정확 일치 검사를 부르지 않는다");
    for banned in ["inspect_config(", "run_fetch(", "send_approval", "host.approve(", "host.confirm(", "fetch_confirm_plan", "credential_helpers: true"] {
        assert!(!github.contains(banned), "github 갈래에 {banned}");
    }
    // 일반 갈래: github 진입점을 부르지 않는다(디스패치는 `Target::Github` 에서 `github_task` 로만)
    for banned in ["inspect_github_repo_at(", "run_github_fetch(", "ensure_github_head(", "github_net_args("] {
        assert!(!service.contains(banned), "일반 갈래에 {banned}");
    }
    let dispatch = service.find("Target::Github { .. })").expect("디스패치");
    assert!(service[dispatch..].starts_with("Target::Github { .. }) {\n            return self.github_task("), "github 분기가 곧바로 github_task 로 간다");
    assert!(dispatch < service.find("inspect_config(r, path").expect("일반 검사"), "github 분기가 일반 검사보다 앞이다");
    // 닫힌 fetch 진입점(`run_github_fetch`)을 부르는 운영 코드는 이 갈래 한 곳이다
    let mut files = Vec::new();
    rs_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut callers: Vec<String> = files
        .iter()
        .filter(|p| !p.ends_with("fetch/run.rs"))
        .filter(|p| prod_code(&p.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().to_string_lossy().replace('\\', "/")).contains("run_github_fetch("))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    callers.sort();
    assert_eq!(callers, ["github.rs"], "run_github_fetch 를 부르는 곳");
    // HEAD 쓰기(`ensure_github_head`)도 이 갈래 한 곳이고, 호스트 부탁은 그 결과 한 군데에서만 간다
    assert_eq!(github.matches("ensure_github_head(").count(), 1);
    assert_eq!(github.matches("host.github_branch(").count(), 1);
}

#[test]
fn 원격_url_출처_주입은_test_hooks_뒤에만_있고_프로덕션은_https_github_com_고정이다() {
    let github = read_src("src/git/fetch/service/github.rs");
    let at = github.find("pub fn with_github_origin_for_test").expect("시험용 주입");
    assert!(github[..at].trim_end().ends_with("#[must_use]") && github[..at].contains("#[cfg(feature = \"test-hooks\")]"), "시험용 주입은 test-hooks 뒤여야 한다");
    // `origin` 필드 자체가 test-hooks 뒤에만 있다(배포 빌드의 구조체에는 없다): 선언·생성은 cfg 줄 바로 아래이고, 읽는 곳은 cfg 가 갈린 두 함수뿐이다
    let cfg = "#[cfg(feature = \"test-hooks\")]\n";
    assert_eq!(github.matches("origin: Option<String>,").count(), 1);
    assert!(github.contains(&format!("    {cfg}    origin: Option<String>,")), "필드 선언은 cfg 아래");
    assert_eq!(github.matches("origin: None").count(), 1);
    assert!(github.contains(&format!("            {cfg}            origin: None,")), "생성도 cfg 아래");
    assert_eq!(github.matches("g.origin").count(), 2, "읽기 하나·주입 하나");
    assert!(github.contains(&format!("    {cfg}    fn github_origin(&self) -> Option<&str> {{\n        self.github.as_ref().and_then(|g| g.origin.as_deref())")));
    assert!(github.contains("    #[cfg(not(feature = \"test-hooks\"))]\n    fn github_origin(&self) -> Option<&str> {\n        None\n    }"), "배포 빌드는 출처가 없다");
    // 앱 코드(lib.rs·router.rs)는 이 주입을 부르지 않는다(기존 시험의 `_for_test` 금지가 이미 건다)
    for rel in ["src/lib.rs", "src/router.rs", "src/fetch_host.rs", "src/watch_host.rs"] {
        assert!(!prod_code(rel).contains("with_github_origin_for_test"), "{rel}");
    }
    assert!(read_src("src/git/github_link.rs").contains("pub const GITHUB_ORIGIN: &str = \"https://github.com\";"));
}

#[test]
fn 기본_브랜치는_설정에_직접_쓰고_저장소_소스_이름을_다시_검사한다() {
    // Phase 6b PR 9: Rust 가 설정의 유일한 작성자라 사이드카에 기록을 부탁하는 줄(`githubBranch`)은 더는 만들지 않는다
    assert!(!prod_code("src/sidecar.rs").contains("githubBranch"), "사이드카로 가는 줄이 없다");
    let host = prod_code("src/fetch_host.rs");
    assert_eq!(host.matches(".set_github_default_branch(").count(), 1, "기록은 호스트 구현 한 곳");
    // 쓰기 전에 저장소 존재·github 소스·`is_safe_branch` 를 다시 본다(사이드카 핸들러와 같은 검사)
    let write = &host[host.find("fn write_github_branch(").expect("쓰기 함수")..];
    let (repo, source, safe, set) = (write.find("store\n        .repo(repo_id)").unwrap_or(usize::MAX), write.find("Some(\"github\")").unwrap(), write.find("is_safe_branch(branch)").unwrap(), write.find(".set_github_default_branch(").unwrap());
    assert!(repo != usize::MAX && repo < source && source < set && safe < set);
}

#[test]
fn 앱_배선은_github_실행기_뒤에_fetch_서비스를_만들고_사이드카에는_소유_여부만_넘긴다() {
    let lib = read_src("src/lib.rs");
    let cache = lib.find("router.set_github_cache(").expect("캐시 실행기 주입");
    let service = lib.find("router.set_fetch(").expect("fetch 서비스 주입");
    let spawn = lib.find("Sidecar::spawn(").expect("사이드카 기동");
    assert!(cache < service && service < spawn);
    assert!(!lib.contains("sidecar_fetch_kinds") && !lib.contains("rust_github"), "PR 10: 사이드카에 fetch 소유 종류를 알리지 않는다");
    // 실행기는 라우터가 받은 것의 복제를 fetch 서비스도 쓴다(진행 중 프로세스 표가 같다)
    assert!(lib.contains("router.set_github_cache(exec.clone());") && lib.contains("GitExecutor::Github(exec.clone())"));
    assert!(lib.contains(".with_github_cache("), "서비스에 캐시 기준 폴더를 준다");
    // 로그인 변경이 fetch 서비스에 닿는다
    assert!(lib.contains("r.github_auth_changed()"));
    // 라우터: 삭제는 fetch 를 먼저 멈추고 기다린 뒤 감시를 멈춘다
    let router = prod_code("src/router.rs");
    let release = &router[router.find("async fn release_github_repo").expect("release")..];
    let (wait, unwatch, scope) = (release.find("forget_and_wait(").unwrap(), release.find("unwatch_if(").unwrap(), release.find("scope.cancel.cancel()").unwrap());
    assert!(wait < unwatch && unwatch < scope, "fetch 멈춤 → 감시 멈춤 → 조회 정리 순서");
    // 감시 시작(`set_watch`)의 "삭제 중" 확인·`resolve_fetch` 는 `watch_target` 잠금 안이다(정지와 시작 사이의 틈이 없다)
    let set_watch = &router[router.find("fn set_watch(").expect("set_watch")..];
    let set_watch = &set_watch[..set_watch.find("fn sync_watch(").expect("다음 함수")];
    let (lock, check, resolve) = (set_watch.find("self.watch_target.lock()").unwrap(), set_watch.find("active.filter(|id| !removing(id))").unwrap(), set_watch.find("resolve_fetch(").unwrap());
    assert!(lock < check && lock < resolve, "삭제 중 확인과 resolve_fetch 는 watch_target 잠금 뒤");
    // github fetch: 설정 대기 → 시그니처 → 정확 일치 검사 → fetch 가 붙어 있고, HEAD 맞추기 앞에 재검사가 있다
    let task = prod_code("src/git/fetch/service/github.rs");
    let (ready, sig, inspect, fetch, recheck, head) = (
        task.find("wait_ready(").unwrap(),
        task.find("let before = refs_signature(").unwrap(),
        task.find("inspect_github_repo_at(r, &dir").unwrap(),
        task.find("run_github_fetch(r, &dir").unwrap(),
        task.rfind("inspect_github_repo_at(r, &dir").unwrap(),
        task.find("ensure_github_head(r").unwrap(),
    );
    assert!(ready < sig && sig < inspect && inspect < fetch && fetch < recheck && recheck < head, "github fetch 의 순서");
}
