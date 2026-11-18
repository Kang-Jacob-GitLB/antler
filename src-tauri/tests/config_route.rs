//! 설정 소유 채널 11개(`config:get`·`config:setUi`·`repos:add`·`repos:addMany`·`repos:addGithub`·`repos:update`·`repos:remove`·`repos:watch`·`external:open`·`external:pickEditorExe`·`req:cancel`,
//! Phase 6b PR 9)의 라우팅·입력 검증·결과 통합 테스트. **Rust 가 설정 파일의 유일한 작성자**임을 본다.
//! 기대값은 TS 핸들러(`src/main/ipc/handlers.ts`·`external.ts`)와 `RepoService`·`ConfigStore` 가 낼 봉투·파일이다: 오류는 `{ok:false,error,code}`, void 는 `data` 없는 `{ok:true}`.
//! 파일 바이트는 PR 1 의 골든(`tests/config.rs`)이 TS 와 맞춰 둔 같은 `ConfigStore` 가 쓰므로 여기서는 채널 → 변경 연산의 대응과 그 결과 파일을 값(키 순서 포함)으로 비교한다.
//!
//! **실제 대화상자·프로그램 실행·사용자 설정을 쓰지 않는다**: 파일 선택·확인 창·opener 는 가짜 포트이고(spawn·파일 관리자는 기록만 한다), 설정·캐시는 임시 폴더, GitHub 캐시는 네트워크 없는
//! `git init` 뿐이며, SSH 는 127.0.0.1 의 닫힌 포트(연결 거부)만 쓴다.

mod ghcache;
#[path = "ssh/common.rs"]
mod common;
#[path = "ssh/server.rs"]
mod server;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use antler_lib::api_host::GithubHost;
use antler_lib::config::sync::ConfigSync;
use antler_lib::config::ConfigStore;
use antler_lib::config_host::{ConfigHost, PickExe};
use antler_lib::env_host::EnvHost;
use antler_lib::external::opener::{normalize_canonical, LaunchRequest, OpenerDeps, Stat};
use antler_lib::external::plan::Platform;
use antler_lib::git::exec::ssh::SshPool;
use antler_lib::git::exec::{GitExecutor, GithubExecutor, LocalExecutor};
use antler_lib::git::github_link::cache_path_matches_link;
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use antler_lib::mirror::{RepoMirror, Resolved, Target};
use antler_lib::net::json::{parse, Json};
use antler_lib::repos::manage::{add_github_repos, cleanup_github_cache, github_target_of, restore_missing_github_caches};
use antler_lib::router::{Routed, Router, API_CHANNELS, APP_CHANNELS, CONFIG_CHANNELS, ENV_CHANNELS, ROUTER_CHANNELS, RUST_CHANNELS};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use futures_util::future::{BoxFuture, FutureExt};
use ghcache::{raw_git, TempDir};
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

fn read_src(rel: &str) -> String {
    // CI(Windows)의 체크아웃은 CRLF 일 수 있다 — 소스 검사는 LF 기준으로 본다
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace("\r\n", "\n")
}

/// 주석을 뺀 운영 코드(`#[cfg(test)]` 이후는 제외)
fn prod_code(rel: &str) -> String {
    let text = read_src(rel);
    let end = text.find("\n#[cfg(test)]\nmod tests").unwrap_or(text.len());
    text[..end].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn ipc_channels() -> Vec<String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("tests/golden/exec/channels.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    v["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()
}

// ── 가짜 포트 ─────────────────────────────────────────────────────────────

/// 프로그램 실행·파일 관리자를 기록만 하는 주입물. 파일 시스템은 실제(임시 폴더)다
#[derive(Default)]
struct FakeDeps {
    spawned: Mutex<Vec<LaunchRequest>>,
    opened: Mutex<Vec<String>>,
    shown: Mutex<Vec<String>>,
}

impl OpenerDeps for FakeDeps {
    fn platform(&self) -> Platform {
        Platform::current()
    }
    fn env(&self, _name: &str) -> Option<String> {
        None
    }
    fn realpath(&self, p: &str) -> Option<String> {
        let real = fs::canonicalize(p).ok()?;
        let s = real.to_str()?;
        if cfg!(windows) {
            normalize_canonical(s)
        } else {
            Some(s.to_string())
        }
    }
    fn stat(&self, p: &str) -> Option<Stat> {
        fs::metadata(p).ok().map(|m| Stat { is_dir: m.is_dir(), is_file: m.is_file() })
    }
    fn lstat(&self, p: &str) -> Option<Stat> {
        fs::symlink_metadata(p).ok().map(|m| Stat { is_dir: m.is_dir(), is_file: m.is_file() })
    }
    fn spawn(&self, req: &LaunchRequest) -> Result<(), String> {
        self.spawned.lock().unwrap().push(req.clone());
        Ok(())
    }
    fn open_path(&self, p: &str) -> Result<(), String> {
        self.opened.lock().unwrap().push(p.to_string());
        Ok(())
    }
    fn show_item(&self, p: &str) -> Result<(), String> {
        self.shown.lock().unwrap().push(p.to_string());
        Ok(())
    }
}

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

fn cache_exec(root: &Path) -> GithubExecutor {
    let hooks = root.join("no-hooks");
    fs::create_dir_all(&hooks).unwrap();
    GithubExecutor::new("git", root.join("remote-repos").to_string_lossy().into_owned(), hooks.to_string_lossy().into_owned(), Arc::new(|| None), Arc::new(Vec::new))
}

// ── 장치 ──────────────────────────────────────────────────────────────────

struct Rig {
    dir: TempDir,
    router: Arc<Router>,
    mirror: RepoMirror,
    store: Arc<ConfigStore>,
    host: Arc<ConfigHost>,
    deps: Arc<FakeDeps>,
    /// 확인 창(설정에 없는 SSH 호스트)이 열린 횟수와 응답
    confirms: Arc<AtomicUsize>,
    allow: Arc<AtomicBool>,
    /// 파일 선택 대화상자가 돌려줄 경로(없으면 취소)
    pick: Arc<Mutex<Option<String>>>,
    /// 미러·fetch 설정 반영 횟수
    mirrors: Arc<AtomicUsize>,
    prefs: Arc<Mutex<Vec<antler_lib::git::fetch::service::FetchPrefs>>>,
    /// 캐시 실행기(복구 시험이 같은 것을 넘긴다)
    exec: GithubExecutor,
}

fn seq_ids() -> impl Fn() -> String + Send + Sync + 'static {
    let n = AtomicUsize::new(0);
    move || format!("id-{}", n.fetch_add(1, Ordering::SeqCst) + 1)
}

impl Rig {
    /// 임시 userData 에 설정(없음)을 열고 라우터·미러·동기화·소유자를 lib.rs 와 같은 순서로 배선한다. `github` 면 GitHub 서비스·캐시 실행기도 단다
    fn new(github: bool) -> Self {
        Self::with_file(github, None)
    }

    fn with_file(github: bool, file_text: Option<&str>) -> Self {
        let dir = TempDir::new("cfgroute");
        let cache = dir.path().join("remote-repos");
        if let Some(t) = file_text {
            fs::write(dir.path().join("antler.config.json"), t).unwrap();
        }
        let store = Arc::new(ConfigStore::open_for_test(dir.path().join("antler.config.json"), Some(cache.to_string_lossy().into_owned()), seq_ids(), || 1_700_000_000_000));
        let mirror = RepoMirror::default();
        let router = Arc::new(Router::new(false, mirror.clone()));
        let exec = cache_exec(dir.path());
        if github {
            router.set_github(gh_host());
            router.set_github_cache(exec.clone());
        }
        let (mirrors, prefs): (Arc<AtomicUsize>, Arc<Mutex<Vec<_>>>) = (Arc::default(), Arc::default());
        let (m, p, r1, r2) = (mirrors.clone(), prefs.clone(), router.clone(), router.clone());
        let sync = Arc::new(ConfigSync::new(
            mirror.clone(),
            Arc::new(move || {
                m.fetch_add(1, Ordering::SeqCst);
                r1.on_mirror_changed();
            }),
            Arc::new(move |x| {
                p.lock().unwrap().push(x.clone());
                r2.apply_fetch_prefs(x);
            }),
        ));
        let listener = sync.clone();
        store.set_on_change(Box::new(move |gen, c| listener.push(Some(gen), c)));
        sync.push(None, &store.get());
        let (confirms, allow) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(true)));
        let (c, a) = (confirms.clone(), allow.clone());
        let env = EnvHost::new(
            Arc::new(|| async { vec!["Ubuntu".to_string()] }.boxed()),
            Arc::new(Vec::new),
            Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                a.load(Ordering::SeqCst)
            }),
            Arc::new(|_, _| GitExecutor::Local(LocalExecutor::default())),
        );
        router.set_env(env.clone());
        let pick: Arc<Mutex<Option<String>>> = Arc::default();
        let pk = pick.clone();
        let pick_exe: PickExe = Arc::new(move || Ok(pk.lock().unwrap().clone()));
        let deps = Arc::new(FakeDeps::default());
        let host = Arc::new(ConfigHost::new(store.clone(), env, cache.to_string_lossy().into_owned(), pick_exe, deps.clone()));
        router.set_config(host.clone());
        Rig { dir, router, mirror, store, host, deps, confirms, allow, pick, mirrors, prefs, exec }
    }

    async fn call(&self, ch: &str, args: Value) -> Value {
        // 앱 경계 그대로: 풀지 못한 호출(서비스 없음)도 오류 봉투다
        self.router.invoke(ch, &args).await
    }

    /// 임시 폴더 아래에 git 저장소를 만든다(커밋 없음 — `rev-parse --show-toplevel` 만 필요하다)
    fn repo(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join("work").join(name);
        fs::create_dir_all(&p).unwrap();
        let out = ghcache::raw("git").args(["init", "-q"]).arg(&p).output().expect("git init");
        assert!(out.status.success());
        p
    }

    fn file_json(&self) -> Json {
        parse(&fs::read_to_string(self.dir.path().join("antler.config.json")).unwrap()).unwrap()
    }
}

fn ok(env: &Value) -> &Value {
    assert_eq!(env["ok"], true, "{env}");
    &env["data"]
}

fn fail(env: &Value, msg: &str) {
    assert_eq!((env["ok"].clone(), env["error"].clone(), env["code"].clone()), (json!(false), json!(msg), json!("ERROR")), "{env}");
}

fn canon(p: &str) -> String {
    let c = fs::canonicalize(p).unwrap();
    normalize_canonical(c.to_str().unwrap()).unwrap_or_else(|| c.to_string_lossy().into_owned()).to_lowercase().replace('/', "\\")
}

fn names(c: &Json, key: &str) -> Vec<String> {
    match c.get(key) {
        Some(Json::Arr(a)) => a.iter().filter_map(|r| r.get("name").or_else(|| r.get("id")).and_then(Json::as_str).map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

// ── 목록·라우팅 ────────────────────────────────────────────────────────────

#[test]
fn 설정_소유_채널_목록은_ipc_허용_목록의_부분집합이고_다른_목록과_겹치지_않는다() {
    let ipc = ipc_channels();
    assert_eq!(CONFIG_CHANNELS.len(), 9);
    for ch in CONFIG_CHANNELS {
        assert!(ipc.iter().any(|c| c == ch), "{ch} 가 IPC_CHANNELS 에 없다");
        for (name, list) in [("RUST_CHANNELS", RUST_CHANNELS), ("APP_CHANNELS", APP_CHANNELS), ("ENV_CHANNELS", ENV_CHANNELS), ("API_CHANNELS", API_CHANNELS), ("ROUTER_CHANNELS", ROUTER_CHANNELS)] {
            assert!(!list.contains(ch), "{ch} 가 {name} 와 겹친다");
        }
    }
}

#[tokio::test]
async fn 소유자가_없으면_설정_채널은_풀_수_없고_파일을_만들지_않는다() {
    let bare = Router::new(false, RepoMirror::default());
    let dir = TempDir::new("cfgroute-off");
    for ch in CONFIG_CHANNELS {
        assert!(matches!(bare.route(ch, &json!(["x"])).await, Routed::Unserved), "{ch} (소유자 없음)");
        assert_eq!(bare.invoke(ch, &json!(["x"])).await, json!({ "ok": false, "error": "이 기능을 사용할 수 없습니다", "code": "ERROR" }), "{ch}");
    }
    assert_eq!(bare.rust_handled(), 0);
    assert!(!dir.path().join("c.json").exists(), "소유하지 않으면 파일을 만들지 않는다");
}

#[tokio::test]
async fn 설정을_소유한_실행에서_서비스가_없으면_jira_gh_채널은_오류로_답한다() {
    // 서비스를 만들지 못한 경우(키체인 오류 등): `gh:loginPat` 의 토큰이 어디로도 가지 않고 오류다
    let rig = Rig::new(false);
    let unavailable = json!({ "ok": false, "error": "연결 서비스를 사용할 수 없습니다", "code": "ERROR" });
    for (ch, args) in [("gh:loginPat", json!(["ghp_secret_token"])), ("gh:auth", json!([])), ("jira:connect", json!([{ "site": "https://x.atlassian.net", "email": "a@b.c", "token": "jira_secret" }])), ("gh:pulls", json!("x"))] {
        assert_eq!(rig.call(ch, args).await, unavailable, "{ch}");
    }
}

#[tokio::test]
async fn 소유자가_있으면_설정_채널을_모두_rust_가_응답하고_배열이_아닌_인자도_다른_곳으로_가지_않는다() {
    let rig = Rig::new(false);
    for ch in CONFIG_CHANNELS.iter().chain(ROUTER_CHANNELS) {
        for args in [json!(["x"]), json!("x"), json!({ "a": 1 }), json!(null), json!([])] {
            let r = rig.call(ch, args.clone()).await;
            assert!(r["ok"].is_boolean(), "{ch} {args}: {r}");
        }
    }
    assert_eq!(rig.router.rust_handled(), ((CONFIG_CHANNELS.len() + ROUTER_CHANNELS.len()) * 5) as u64);
    // void 채널은 data 없는 `{ok:true}`
    assert_eq!(rig.call("repos:watch", json!(["nope"])).await, json!({ "ok": true }));
    assert_eq!(rig.call("repos:watch", json!([null])).await, json!({ "ok": true }));
    assert_eq!(rig.call("req:cancel", json!(["req-1"])).await, json!({ "ok": true }));
    assert_eq!(rig.call("req:cancel", json!([])).await, json!({ "ok": true }));
}

// ── config:* ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn config_get_과_set_ui_는_파일에_쓰고_검증은_ts_와_같다() {
    let rig = Rig::new(false);
    let c = rig.call("config:get", json!([])).await;
    assert_eq!(ok(&c)["ui"]["theme"], "system");
    assert!(!rig.dir.path().join("antler.config.json").exists(), "읽기는 파일을 만들지 않는다");
    let mirrors_before = rig.mirrors.load(Ordering::SeqCst);
    let r = rig.call("config:setUi", json!([{ "theme": "dark", "autoFetchMinutes": 9 }])).await;
    assert_eq!(ok(&r)["ui"]["theme"], "dark");
    assert_eq!(rig.mirrors.load(Ordering::SeqCst), mirrors_before, "저장소와 무관한 설정 쓰기는 미러 반영(on_mirror_changed)을 부르지 않는다");
    let file = rig.file_json();
    assert_eq!(file.get("ui").and_then(|u| u.get("autoFetchMinutes")), Some(&Json::Num(9.0)));
    // fetch 설정(주기)이 응답보다 먼저 서비스에 간다
    assert_eq!(rig.prefs.lock().unwrap().last().map(|p| p.auto_fetch_minutes), Some(9.0));
    // 검증: TS 와 같은 문구
    fail(&rig.call("config:setUi", json!([{ "autoFetchMinutes": 1441 }])).await, "잘못된 자동 fetch 주기입니다(0~1440분)");
    fail(&rig.call("config:setUi", json!([{ "editor": "emacs" }])).await, "잘못된 에디터입니다");
    fail(&rig.call("config:setUi", json!([null])).await, "TypeError");
    fail(&rig.call("config:setUi", json!([])).await, "TypeError");
    // 실패한 요청은 파일을 바꾸지 않는다
    assert_eq!(rig.file_json().get("ui").and_then(|u| u.get("autoFetchMinutes")), Some(&Json::Num(9.0)));
}

#[tokio::test]
async fn set_ui_로는_승인과_실행_파일을_위조할_수_없다() {
    let rig = Rig::new(false);
    let fp = "a".repeat(64);
    // 직접 바꿀 수 없는 설정은 거부
    fail(&rig.call("config:setUi", json!([{ "editorExePath": "C:\\evil.exe" }])).await, "이 설정은 직접 바꿀 수 없습니다");
    fail(&rig.call("config:setUi", json!([{ "skippedUpdateVersion": "9.9.9" }])).await, "이 설정은 직접 바꿀 수 없습니다");
    // 승인 지문은 패치가 `ui` 아래로만 들어가므로 최상위 승인 맵은 바뀌지 않는다(승인은 fetch 서비스만 쓴다)
    let r = rig.call("config:setUi", json!([{ "fetchApprovals": { "r": fp }, "trustedConfigs": { "r": fp } }])).await;
    assert_eq!(r["ok"], true);
    let file = rig.file_json();
    assert!(file.get("fetchApprovals").is_none() && file.get("trustedConfigs").is_none(), "최상위 승인 맵이 생기면 안 된다");
    assert!(rig.prefs.lock().unwrap().iter().all(|p| p.approvals.is_empty() && p.trusted.is_empty()), "fetch 설정에 위조 승인이 들어갔다");
    assert!(rig.store.fetch_approval("r").is_none() && rig.store.trusted_config("r").is_none());
}

// ── repos:add · addMany ─────────────────────────────────────────────────────

#[tokio::test]
async fn 저장소_추가는_최상위로_등록하고_미러가_응답보다_먼저_바뀌고_다시_추가하면_같은_항목이다() {
    let rig = Rig::new(false);
    let top = rig.repo("proj");
    let sub = top.join("src").join("deep");
    fs::create_dir_all(&sub).unwrap();
    let added = rig.call("repos:add", json!([{ "kind": "local", "path": sub.to_string_lossy() }])).await;
    let repo = ok(&added);
    assert_eq!(repo["id"], "id-1");
    assert_eq!(repo["name"], "proj");
    assert_eq!(repo["sourceId"], "local");
    assert_eq!(canon(repo["path"].as_str().unwrap()), canon(&top.to_string_lossy()), "하위 폴더를 골라도 최상위 경로로 등록한다");
    // 응답을 받은 시점에 미러는 이미 새 저장소를 안다(renderer 의 다음 호출이 옛 미러를 보지 않는다)
    assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Rust(Target::Local { .. })));
    assert_eq!(rig.mirror.name("id-1").as_deref(), Some("proj"));
    // 다시 추가하면 같은 항목(쓰지 않는다 — 중복 없음), 그룹을 골랐으면 옮긴다
    let again = rig.call("repos:add", json!([{ "kind": "local", "path": top.to_string_lossy(), "group": "  팀  " }])).await;
    assert_eq!(ok(&again)["id"], "id-1");
    assert_eq!(ok(&again)["group"], "팀");
    assert_eq!(names(&rig.file_json(), "repos"), ["proj"]);
    // 저장소가 아닌 폴더는 GIT 오류(TS `GitError`)
    let plain = rig.dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let bad = rig.call("repos:add", json!([{ "kind": "local", "path": plain.to_string_lossy() }])).await;
    assert_eq!((bad["ok"].clone(), bad["code"].clone()), (json!(false), json!("GIT")), "{bad}");
    assert_eq!(names(&rig.file_json(), "repos"), ["proj"]);
}

#[tokio::test]
async fn 저장소_추가_입력_검증은_ts_핸들러와_같다() {
    let rig = Rig::new(false);
    let net = "네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)";
    fail(&rig.call("repos:add", json!([])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "github", "path": "C:\\x" }])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": 5 }])).await, "잘못된 경로입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "  " }])).await, "잘못된 경로입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "C:\\a\u{0}b" }])).await, "잘못된 경로입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "\\\\host\\share\\r" }])).await, net);
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": " \\\\host\\share\\r" }])).await, net);
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "\\??\\UNC\\host\\share" }])).await, net);
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "C:\\r", "group": 3 }])).await, "잘못된 그룹 이름입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "local", "path": "C:\\r", "group": "g".repeat(201) }])).await, "잘못된 그룹 이름입니다");
    fail(&rig.call("repos:add", json!([{ "kind": "wsl", "path": "/home/x", "distro": "Debian" }])).await, "설치된 WSL 배포판이 아닙니다: Debian");
    assert!(!rig.dir.path().join("antler.config.json").exists(), "거부된 입력은 파일을 쓰지 않는다");
}

#[tokio::test]
async fn 한_번에_추가는_실패한_경로가_나머지를_막지_않고_개수와_환경을_검사한다() {
    let rig = Rig::new(false);
    let (a, b) = (rig.repo("a"), rig.repo("b"));
    let plain = rig.dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let paths = [a.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned(), b.to_string_lossy().into_owned(), a.to_string_lossy().into_owned()];
    let r = rig.call("repos:addMany", json!([{ "kind": "local", "paths": paths, "group": "g" }])).await;
    let data = ok(&r);
    assert_eq!(data["added"].as_array().unwrap().iter().map(|x| x["name"].clone()).collect::<Vec<_>>(), [json!("a"), json!("b")], "중복 경로는 한 번만");
    assert_eq!(data["added"][0]["group"], "g");
    assert_eq!(data["failed"].as_array().unwrap().len(), 1);
    assert_eq!(data["failed"][0]["path"], plain.to_string_lossy().as_ref());
    assert!(matches!(rig.mirror.resolve("id-2"), Resolved::Rust(_)));
    // 모양·개수
    fail(&rig.call("repos:addMany", json!([{ "kind": "local", "paths": [] }])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:addMany", json!([{ "kind": "local", "paths": vec!["C:\\x"; 501] }])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:addMany", json!([{ "kind": "local", "paths": [5] }])).await, "잘못된 경로입니다");
    fail(&rig.call("repos:addMany", json!([{ "kind": "wsl", "distro": "Ubuntu", "paths": ["\\\\wsl.localhost\\Ubuntu\\h\\a", "\\\\wsl.localhost\\Debian\\h\\b"] }])).await, "한 번에 추가하는 저장소는 같은 환경이어야 합니다");
}

#[tokio::test]
async fn ssh_호스트_승인은_환경_채널과_추가가_하나의_기억을_나눈다() {
    let rig = Rig::new(false);
    let home = rig.dir.path().join("home");
    fs::create_dir_all(home.join(".ssh")).unwrap();
    rig.router.set_ssh_pool(SshPool::for_home(&home.to_string_lossy()));
    // 127.0.0.1 의 닫힌 포트 — 연결 거부(네트워크 밖으로 나가지 않는다)
    let host = "127.0.0.1:1";
    let input = json!({ "kind": "ssh", "sshHost": host, "path": "~/x" });
    let test = rig.call("env:test", json!([{ "kind": "ssh", "sshHost": host }])).await;
    assert_eq!(test["ok"], false, "{test}");
    assert_eq!(rig.confirms.load(Ordering::SeqCst), 1, "처음 보는 호스트는 한 번 묻는다");
    for (ch, args) in [("repos:add", json!([input])), ("repos:addMany", json!([{ "kind": "ssh", "sshHost": host, "paths": ["~/x"] }])), ("env:test", json!([{ "kind": "ssh", "sshHost": host }]))] {
        let _ = rig.call(ch, args).await;
        assert_eq!(rig.confirms.load(Ordering::SeqCst), 1, "{ch}: 같은 호스트는 다시 묻지 않는다(승인 기억 하나)");
    }
    // 거부한 호스트는 기억하지 않는다 — 다시 시도하면 다시 묻고, 연결하지 않는다
    rig.allow.store(false, Ordering::SeqCst);
    let other = json!({ "kind": "ssh", "sshHost": "127.0.0.1:2", "path": "~/x" });
    fail(&rig.call("repos:add", json!([other.clone()])).await, "연결을 취소했습니다");
    fail(&rig.call("repos:add", json!([other])).await, "연결을 취소했습니다");
    assert_eq!(rig.confirms.load(Ordering::SeqCst), 3);
    assert!(!rig.dir.path().join("antler.config.json").exists(), "연결하지 못한 호스트·저장소는 설정에 남지 않는다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_저장소_추가는_연결을_쥔_채_등록하고_등록한_뒤에도_연결이_남고_삭제하면_닫힌다() {
    // 127.0.0.1 의 프로세스 안 russh 서버·임시 홈만 쓴다(외부 접속·사용자의 ~/.ssh 를 쓰지 않는다). 원격 git 은 로컬 실행기로 돈다
    let client = server::new_key();
    let srv = server::TestServer::start(server::Opts::new(vec![client.public_key().clone()])).await;
    let home = Arc::new(common::Home::new(&client));
    let key = home.key_for_config();
    home.write_config(&format!("Host test\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {key}\n  IdentitiesOnly yes\n", srv.port));
    home.write_known_hosts(&common::known_hosts_line(srv.port, &srv.host_key));
    let h = home.clone();
    let pool = SshPool::new(move |host| h.executor(host));
    let rig = Rig::new(false);
    rig.router.set_ssh_pool(pool.clone());
    let remote = common::make_repo(0);
    let path = remote.path().to_string_lossy().into_owned();
    let added = rig.call("repos:add", json!([{ "kind": "ssh", "sshHost": " test ", "path": path, "group": "원격" }])).await;
    let repo = ok(&added);
    assert_eq!((repo["sourceId"].clone(), repo["group"].clone()), (json!("ssh:test"), json!("원격")));
    assert_eq!(srv.connections(), 1, "한 연결로 확인하고 등록했다");
    // 소스가 저장소와 같은 쓰기로 들어갔고 미러가 ssh 호스트를 푼다
    assert_eq!(names(&rig.file_json(), "sources"), ["local", "ssh:test"]);
    assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Rust(Target::Ssh { ref host, .. }) if host == "test"));
    // 저장 뒤에는 미러가 쓰는 호스트라 `hold` 가 풀려도(그리고 `retain` 이 돌아도) 연결이 닫히지 않는다
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(pool.hosts(), ["test"]);
    assert_eq!(srv.open_connections(), 1, "등록한 저장소의 연결이 닫혔다");
    // 삭제하면 쓰지 않는 호스트의 연결이 정리된다
    ok(&rig.call("repos:remove", json!(["id-1"])).await);
    common::wait_for("삭제한 호스트의 풀 정리", || pool.hosts().is_empty()).await;
    common::wait_for("연결 닫힘", || srv.open_connections() == 0).await;
}

// ── repos:update · repos:remove ─────────────────────────────────────────────

/// 응답의 `data`(설정)를 순서를 보존하는 트리로
fn cfg(env: &Value) -> Json {
    parse(&ok(env).to_string()).unwrap()
}

#[tokio::test]
async fn 이름과_그룹을_바꾸고_삭제하면_설정과_미러에서_함께_사라진다() {
    let rig = Rig::new(false);
    let (a, b) = (rig.repo("a"), rig.repo("b"));
    for p in [&a, &b] {
        ok(&rig.call("repos:add", json!([{ "kind": "local", "path": p.to_string_lossy() }])).await);
    }
    let r = rig.call("repos:update", json!(["id-1", { "name": "  앞  ", "group": " 팀 " }])).await;
    assert_eq!(names(&cfg(&r), "repos"), ["앞", "b"]);
    assert_eq!(ok(&r)["repos"][0]["group"], "팀");
    // 빈 이름은 무시하고, 그룹은 비우면 해제
    let r = rig.call("repos:update", json!(["id-1", { "name": "   ", "group": "" }])).await;
    let repos = ok(&r)["repos"].as_array().unwrap().clone();
    assert_eq!((repos[0]["name"].clone(), repos[0].get("group").cloned()), (json!("앞"), None));
    assert_eq!(rig.mirror.name("id-1").as_deref(), Some("앞"), "이름 변경도 응답 전에 미러에 반영된다");
    // 삭제: 설정·미러에서 사라지고 모르는 id 도 던지지 않는다
    let r = rig.call("repos:remove", json!(["id-1"])).await;
    assert_eq!(names(&cfg(&r), "repos"), ["b"]);
    assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Unserved));
    assert_eq!(rig.call("repos:remove", json!(["nope"])).await["ok"], true);
    assert_eq!(rig.call("repos:remove", json!([5])).await["ok"], true);
    // 마지막 저장소를 지워도 로컬 소스는 남는다
    ok(&rig.call("repos:remove", json!(["id-2"])).await);
    assert_eq!(names(&rig.file_json(), "sources"), ["local"]);
    // TS 처럼 이름·그룹에 문자열이 아닌 값이 오면(아는 저장소에서만) TypeError
    ok(&rig.call("repos:add", json!([{ "kind": "local", "path": a.to_string_lossy() }])).await);
    fail(&rig.call("repos:update", json!(["id-3", { "name": 5 }])).await, "TypeError");
    assert_eq!(rig.call("repos:update", json!(["nope", { "name": 5 }])).await["ok"], true, "모르는 id 는 던지지 않는다(TS 와 같다)");
}

/// 패치의 객체 키가 (깊이 전체에서) 정렬돼 있고 정수형 키가 없는가. `serde_json::Value` 가 키를 정렬하므로 renderer → 채널을 지나도 순서가 같은 사례만 TS 골든과 바이트로 대조할 수 있다
fn is_sorted_deep(v: &Json) -> bool {
    match v {
        Json::Obj(o) => o.iter().all(|(k, _)| !k.starts_with(|c: char| c.is_ascii_digit())) && o.windows(2).all(|w| w[0].0 < w[1].0) && o.iter().all(|(_, x)| is_sorted_deep(x)),
        Json::Arr(a) => a.iter().all(is_sorted_deep),
        _ => true,
    }
}

/// **PR 1 의 TS 골든(`tests/golden/config/store.json` — 실제 TS `ConfigStore` 가 만든 최종 파일 바이트·단계별 오류 문구)을 채널로 재생한다.**
/// 채널로 표현되는 연산(`config:setUi`·`repos:update`·`repos:remove`)만으로 된 사례 중 패치 키가 정렬돼 있는 것을 `Rig` 의 라우터에 그대로 흘려 최종 파일을 **바이트로** 대조한다
/// (키 순서를 보존하는 `ConfigStore` 의 쓰기는 PR 1 이 이미 맞췄고, 여기서 보는 것은 채널 입력 → 변경 연산의 대응과 검증 문구다). `addRepo`·승인·기본 브랜치 연산은 채널로 직접 올 수 없어 이 시험의 대상이 아니다
#[tokio::test]
async fn 설정_골든의_채널_연산_사례를_라우터로_재생하면_최종_파일_바이트와_오류_문구가_ts_와_같다() {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("tests/golden/config/store.json")).unwrap();
    let golden = parse(&text).unwrap();
    let Some(Json::Arr(cases)) = golden.get("cases") else { panic!("cases") };
    let (mut replayed, mut with_file, mut with_error) = (0, 0, 0);
    for c in cases {
        let name = c.get("name").and_then(Json::as_str).unwrap_or_default();
        let input = c.get("input").unwrap();
        let ops = match input.get("ops") {
            Some(Json::Arr(o)) => o,
            _ => panic!("ops"),
        };
        let file_text = input.get("file").and_then(Json::as_str);
        let simple = input.get("fileIsDir") != Some(&Json::Bool(true)) && matches!(input.get("fileHex"), None | Some(Json::Null)) && input.get("cacheDir") != Some(&Json::Bool(true)) && !file_text.is_some_and(|t| t.contains("github"));
        let on_channels = ops.iter().all(|o| match o.get("op").and_then(Json::as_str) {
            Some("setUi") | Some("updateRepo") => o.get("patch").is_some_and(is_sorted_deep) && (o.get("op").and_then(Json::as_str) == Some("setUi") || o.get("id").and_then(Json::as_str).is_some()),
            Some("removeRepo") => o.get("id").and_then(Json::as_str).is_some(),
            _ => false,
        });
        if !simple || !on_channels {
            continue;
        }
        let rig = Rig::with_file(false, file_text);
        assert_eq!(Json::Bool(rig.store.load_failed()), *c.get("loadFailed").unwrap(), "{name}: loadFailed");
        let Some(Json::Arr(steps)) = c.get("steps") else { panic!("steps") };
        let mut expected = c.get("initial").unwrap();
        for (i, (op, step)) in ops.iter().zip(steps).enumerate() {
            let patch = op.get("patch").map(antler_lib::config_host::to_value);
            let id = op.get("id").and_then(Json::as_str);
            let env = match op.get("op").and_then(Json::as_str).unwrap() {
                "setUi" => rig.call("config:setUi", json!([patch])).await,
                "updateRepo" => rig.call("repos:update", json!([id, patch])).await,
                _ => rig.call("repos:remove", json!([id])).await,
            };
            match step.get("error").and_then(Json::as_str) {
                Some(want) => {
                    fail(&env, want);
                    with_error += 1;
                }
                None => {
                    assert_eq!(env["ok"], true, "{name} / 단계 {i}: {env}");
                    // 설정이 바뀐 단계만 `config` 가 있다 — 없으면 직전 설정 그대로다(`tests/config.rs` 와 같은 규칙)
                    if let Some(next) = step.get("config") {
                        expected = next;
                    }
                    assert_eq!(env["data"], antler_lib::config_host::to_value(expected), "{name} / 단계 {i}: 반환한 설정");
                }
            }
        }
        // 최종 파일 바이트(쓰지 않은 사례는 파일이 없다)와 손상 백업 이름
        let file = rig.dir.path().join("antler.config.json");
        match (c.get("file"), fs::read_to_string(&file).ok().filter(|_| file.is_file())) {
            (Some(Json::Null), None) => {}
            (Some(Json::Str(want)), Some(got)) => {
                assert_eq!(&got, want, "{name}: 최종 파일 바이트");
                with_file += 1;
            }
            (want, got) => panic!("{name}: 파일 유무가 다르다 — TS {want:?} / Rust {got:?}"),
        }
        if let Some(Json::Arr(files)) = c.get("files") {
            for f in files.iter().filter_map(Json::as_str).filter(|f| f.ends_with(".bak")) {
                assert!(rig.dir.path().join(f).is_file(), "{name}: 손상 백업 {f}");
            }
        }
        replayed += 1;
    }
    println!("골든 재생 {replayed}개 사례(최종 파일 바이트 대조 {with_file}·오류 문구 대조 {with_error})");
    assert!(replayed >= 150 && with_file >= 60 && with_error >= 40, "재생한 사례가 너무 적다: {replayed}개(파일 {with_file}·오류 {with_error})");
}

#[tokio::test]
async fn 한_요청_안에서_추가_이름_승인_삭제를_거친_설정_파일은_기대한_값과_키_순서까지_같다() {
    let rig = Rig::new(false);
    let a = rig.repo("a");
    let added = rig.call("repos:add", json!([{ "kind": "local", "path": a.to_string_lossy(), "group": "g" }])).await;
    let path = ok(&added)["path"].as_str().unwrap().to_string();
    // 승인은 fetch 서비스 두 지점의 쓰기(호스트 구현)로만 쓰인다 — 여기서는 같은 변경자를 직접 불러 파일 바이트를 본다
    let fp = "b".repeat(64);
    rig.store.set_fetch_approval("id-1", Some(&fp)).unwrap();
    rig.store.set_trusted_config("id-1", &fp).unwrap();
    // 변경 알림이 fetch 설정에 승인을 싣는다(응답 전에)
    let last = rig.prefs.lock().unwrap().last().cloned().unwrap();
    assert_eq!((last.approvals.get("id-1"), last.trusted.get("id-1")), (Some(&fp), Some(&fp)));
    ok(&rig.call("repos:update", json!(["id-1", { "name": "새 이름" }])).await);
    ok(&rig.call("config:setUi", json!([{ "lastRepoId": "id-1" }])).await);
    // 키 순서가 TS 와 같아야 한다(`serde_json::json!` 는 키를 정렬하므로 본문을 문자열로 적는다)
    let path_json = serde_json::to_string(&path).unwrap();
    let expected = parse(&format!(
        r#"{{"version":1,"sources":[{{"id":"local","kind":"local","label":"Local"}}],"repos":[{{"sourceId":"local","path":{path_json},"name":"새 이름","group":"g","id":"id-1"}}],"ui":{{"theme":"system","locale":"ko","paneWidths":[248,380],"lastRepoId":"id-1"}},"fetchApprovals":{{"id-1":"{fp}"}},"trustedConfigs":{{"id-1":"{fp}"}}}}"#
    ))
    .unwrap();
    assert_eq!(rig.file_json(), expected);
    // 삭제하면 저장소·승인·허용 지문이 함께 지워지고 `lastRepoId` 도 파일에서 사라진다
    ok(&rig.call("repos:remove", json!(["id-1"])).await);
    let gone = rig.file_json();
    assert_eq!(gone.get("repos"), Some(&Json::Arr(Vec::new())));
    assert_eq!((gone.get("fetchApprovals"), gone.get("trustedConfigs")), (Some(&Json::Obj(Vec::new())), Some(&Json::Obj(Vec::new()))));
    assert!(gone.get("ui").and_then(|u| u.get("lastRepoId")).is_none());
    assert!(rig.prefs.lock().unwrap().last().is_some_and(|p| p.approvals.is_empty() && p.trusted.is_empty()), "삭제한 저장소의 승인은 fetch 설정에서도 사라진다");
}

// ── repos:addGithub · 삭제 순서 · 고아 정리 ──────────────────────────────────

fn cache_of(rig: &Rig) -> PathBuf {
    rig.dir.path().join("remote-repos")
}

fn is_bare(dir: &Path) -> bool {
    raw_git(dir, &["rev-parse", "--is-bare-repository"]) == "true"
}

#[tokio::test]
async fn 원격_전용_저장소를_추가하면_캐시에_빈_bare_저장소를_만들고_같은_이름이면_owner_를_붙인다() {
    let rig = Rig::new(true);
    let r = rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "Octo", "repo": "Hello" }, { "owner": "..", "repo": "x" }, { "owner": "globex", "repo": "hello", "defaultBranch": "main" }], "group": " 원격 " }])).await;
    let data = ok(&r);
    let added = data["added"].as_array().unwrap();
    assert_eq!(added.iter().map(|a| a["name"].clone()).collect::<Vec<_>>(), [json!("Hello"), json!("globex/hello")]);
    assert_eq!(added[0]["group"], "원격");
    assert_eq!(added[0]["sourceId"], "github");
    assert_eq!(data["failed"].as_array().unwrap().len(), 1);
    assert_eq!(data["failed"][0]["fullName"], "../x");
    let dir = cache_of(&rig).join("octo").join("hello.git");
    assert!(is_bare(&dir), "캐시에 bare 저장소가 만들어진다");
    assert_eq!(raw_git(&dir, &["config", "--local", "remote.origin.url"]), "https://github.com/Octo/Hello.git");
    // 기본 브랜치를 알면 HEAD 가 미리 가리킨다
    assert_eq!(raw_git(&cache_of(&rig).join("globex").join("hello.git"), &["symbolic-ref", "HEAD"]), "refs/remotes/origin/main");
    // 응답 전에 미러에 링크와 함께 반영된다(github 소스는 서비스·캐시 실행기가 있으면 Rust 가 푼다)
    assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Rust(Target::Github { link: Some(_), .. })));
    // 같은 저장소를 다시 추가하면 기존 항목(대소문자만 달라도), 그룹을 새로 골랐으면 옮긴다
    let again = rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }], "group": "다른" }])).await;
    assert_eq!(ok(&again)["added"][0]["id"], "id-1");
    assert_eq!(ok(&again)["added"][0]["group"], "다른");
    assert_eq!(names(&rig.file_json(), "repos").len(), 2);
    // 모양·개수·그룹
    fail(&rig.call("repos:addGithub", json!([{ "repos": [] }])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:addGithub", json!([{ "repos": vec![json!({ "owner": "a", "repo": "b" }); 201] }])).await, "잘못된 입력입니다");
    fail(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "a", "repo": "b" }], "group": 1 }])).await, "잘못된 그룹 이름입니다");
    fail(&rig.call("repos:addGithub", json!([])).await, "잘못된 입력입니다");
}

#[tokio::test]
async fn 같은_경로를_동시에_추가해도_폴더는_하나이고_등록도_하나다() {
    let rig = Rig::new(true);
    let one = json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }]);
    let (a, b, c) = tokio::join!(rig.call("repos:addGithub", one.clone()), rig.call("repos:addGithub", one.clone()), rig.call("repos:addGithub", one.clone()));
    for r in [&a, &b, &c] {
        assert_eq!(ok(r)["added"][0]["id"], "id-1", "{r}");
    }
    assert_eq!(names(&rig.file_json(), "repos").len(), 1);
    assert!(is_bare(&cache_of(&rig).join("octo").join("hello.git")));
}

#[tokio::test]
async fn 등록되지_않은_찌꺼기_폴더는_지우고_새로_만들고_init_이_실패하면_폴더를_남기지_않는다() {
    let rig = Rig::new(true);
    let stray = cache_of(&rig).join("octo").join("hello.git");
    fs::create_dir_all(stray.join("objects")).unwrap();
    fs::write(stray.join("junk.txt"), "x").unwrap();
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }])).await);
    assert!(!stray.join("junk.txt").exists() && is_bare(&stray), "찌꺼기를 치우고 다시 만든다");
    // 폴더를 만들지 못하는 경우: 같은 이름의 *파일* 이 owner 자리에 있다 → 실패하고 아무것도 남지 않으며 남의 파일은 그대로다
    fs::write(cache_of(&rig).join("blocked"), "file").unwrap();
    let r = rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "blocked", "repo": "r" }] }])).await;
    assert_eq!(ok(&r)["added"].as_array().unwrap().len(), 0);
    assert_eq!(ok(&r)["failed"].as_array().unwrap().len(), 1);
    assert!(cache_of(&rig).join("blocked").is_file(), "남의 파일은 건드리지 않는다");
    assert_eq!(names(&rig.file_json(), "repos").len(), 1);
}

#[tokio::test]
async fn 삭제는_캐시_폴더와_빈_owner_폴더를_지우고_다른_저장소는_건드리지_않는다() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }, { "owner": "octo", "repo": "world" }, { "owner": "solo", "repo": "one" }] }])).await);
    let cache = cache_of(&rig);
    assert!(cache.join("solo").join("one.git").exists());
    let r = rig.call("repos:remove", json!(["id-3"])).await;
    assert_eq!(names(&cfg(&r), "repos").len(), 2);
    assert!(!cache.join("solo").exists(), "비게 된 owner 폴더도 정리한다");
    // 같은 owner 의 다른 저장소가 남아 있으면 owner 폴더는 남는다
    ok(&rig.call("repos:remove", json!(["id-1"])).await);
    assert!(!cache.join("octo").join("hello.git").exists() && cache.join("octo").join("world.git").exists());
    assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Unserved));
    // 삭제 직후 같은 저장소를 다시 추가할 수 있다
    let again = rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }])).await;
    assert_eq!(ok(&again)["added"].as_array().unwrap().len(), 1);
    assert!(is_bare(&cache.join("octo").join("hello.git")));
}

#[tokio::test]
async fn 삭제는_레코드의_링크와_캐시_경로가_맞을_때만_폴더를_지운다() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }, { "owner": "other", "repo": "thing" }] }])).await);
    let cache = rig.host.cache_dir().to_string();
    let record = rig.store.repo("id-1").unwrap();
    assert!(github_target_of(&rig.store, &cache, &record).is_some());
    let with = |key: &str, v: Json| match &record {
        Json::Obj(o) => Json::Obj(o.iter().map(|(k, x)| if k == key { (k.clone(), v.clone()) } else { (k.clone(), x.clone()) }).collect()),
        _ => unreachable!(),
    };
    // 다른 저장소의 캐시를 가리키는 레코드(링크는 octo/hello 인데 경로는 other/thing)·캐시 밖 경로·github 가 아닌 소스는 지우지 않는다
    let other_path = cache_of(&rig).join("other").join("thing.git").to_string_lossy().into_owned();
    assert!(!cache_path_matches_link(&cache, &other_path, "octo", "hello"));
    assert!(github_target_of(&rig.store, &cache, &with("path", Json::Str(other_path))).is_none());
    assert!(github_target_of(&rig.store, &cache, &with("path", Json::Str(rig.dir.path().join("elsewhere").to_string_lossy().into_owned()))).is_none());
    assert!(github_target_of(&rig.store, &cache, &with("sourceId", Json::Str("local".into()))).is_none());
    assert!(github_target_of(&rig.store, &cache, &with("github", Json::Obj(vec![("owner".into(), Json::Str("..".into())), ("repo".into(), Json::Str("x".into()))]))).is_none());
    // 지우기는 없는 폴더에서도 던지지 않는다(이미 지워져 있다)
    fs::remove_dir_all(cache_of(&rig).join("octo").join("hello.git")).unwrap();
    ok(&rig.call("repos:remove", json!(["id-1"])).await);
    assert!(cache_of(&rig).join("other").join("thing.git").exists(), "다른 저장소의 캐시는 그대로다");
}

#[tokio::test]
async fn 고아_정리는_레코드가_없는_캐시_모양_폴더만_지우고_설정을_읽지_못했으면_아무것도_지우지_않는다() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "keep" }] }])).await);
    let cache = cache_of(&rig);
    for orphan in ["gone/old.git", "octo/stale.git"] {
        let d = cache.join(orphan);
        fs::create_dir_all(d.join("objects")).unwrap();
    }
    // 모양이 다른 폴더·파일은 건드리지 않는다
    fs::create_dir_all(cache.join("octo").join("notes")).unwrap();
    fs::write(cache.join("octo").join("readme.txt"), "x").unwrap();
    fs::write(cache.join("top.txt"), "x").unwrap();
    let removed = cleanup_github_cache(rig.host.store(), rig.host.cache_dir(), rig.host.locks()).await;
    assert_eq!(removed, 2);
    assert!(cache.join("octo").join("keep.git").exists());
    assert!(!cache.join("gone").exists(), "비게 된 owner 폴더도 정리한다");
    assert!(!cache.join("octo").join("stale.git").exists());
    assert!(cache.join("octo").join("notes").exists() && cache.join("octo").join("readme.txt").exists() && cache.join("top.txt").exists());
    // 설정을 읽지 못한 채 시작했으면(저장소 목록이 빈 것이 사실이 아니다) 아무것도 지우지 않는다
    let broken = Rig::with_file(true, Some("{ 손상"));
    assert!(broken.store.load_failed());
    let bc = cache_of(&broken);
    fs::create_dir_all(bc.join("x").join("y.git")).unwrap();
    assert_eq!(cleanup_github_cache(broken.host.store(), broken.host.cache_dir(), broken.host.locks()).await, 0);
    assert!(bc.join("x").join("y.git").exists());
}

#[tokio::test]
async fn 소유자_폴더가_링크면_따라가지도_지우지도_않는다() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "keep" }] }])).await);
    let cache = cache_of(&rig);
    let outside = rig.dir.path().join("outside");
    fs::create_dir_all(outside.join("v.git").join("objects")).unwrap();
    let link = cache.join("evil");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
    if !made {
        eprintln!("링크를 만들 권한이 없어 건너뜀");
        return;
    }
    assert_eq!(cleanup_github_cache(rig.host.store(), rig.host.cache_dir(), rig.host.locks()).await, 0);
    assert!(outside.join("v.git").join("objects").exists(), "링크 너머의 폴더는 지우지 않는다");
}

#[tokio::test]
async fn 폴더가_없는_레코드는_시작_때_빈_캐시로_복구한다_electron_설정_가져오기_뒤() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello", "defaultBranch": "dev" }] }])).await);
    let dir = cache_of(&rig).join("octo").join("hello.git");
    fs::remove_dir_all(&dir).unwrap();
    let made = restore_missing_github_caches(rig.host.store(), Some(rig.exec.clone()), rig.host.cache_dir(), rig.host.locks()).await;
    assert_eq!(made, 1);
    assert!(is_bare(&dir));
    assert_eq!(raw_git(&dir, &["symbolic-ref", "HEAD"]), "refs/remotes/origin/dev");
    // 이미 있는 폴더는 건드리지 않는다
    assert_eq!(restore_missing_github_caches(rig.host.store(), Some(rig.exec.clone()), rig.host.cache_dir(), rig.host.locks()).await, 0);
}

#[tokio::test]
async fn init_이_실패하면_만든_폴더를_지우고_아무것도_등록하지_않는다() {
    let rig = Rig::new(true);
    // 훅 디렉터리를 쓸 수 없는 실행기는 git 을 띄우지 않는다 — 폴더를 만든 뒤 init 이 실패하는 길
    let cache = cache_of(&rig);
    let broken = GithubExecutor::new("git", cache.to_string_lossy().into_owned(), rig.dir.path().join("no-such-hooks").to_string_lossy().into_owned(), Arc::new(|| None), Arc::new(Vec::new));
    let input = parse(r#"{"repos":[{"owner":"octo","repo":"hello"}]}"#).unwrap();
    let r = add_github_repos(&rig.store, Some(broken), rig.host.cache_dir(), rig.host.locks(), Some(&input)).await.unwrap();
    assert_eq!(r["added"].as_array().unwrap().len(), 0);
    assert_eq!(r["failed"].as_array().unwrap().len(), 1, "{r}");
    assert!(!cache.join("octo").join("hello.git").exists(), "만든 폴더를 지운다");
    assert!(!cache.join("octo").exists(), "비게 된 owner 폴더도 정리한다");
    assert_eq!(names(&rig.store.get(), "repos").len(), 0);
    // 실행기가 없으면(GitHub 서비스 없음) 만들지 않는다
    let none = add_github_repos(&rig.store, None, rig.host.cache_dir(), rig.host.locks(), Some(&input)).await;
    assert_eq!(none.unwrap_err(), "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
}

#[tokio::test]
async fn 폴더를_지우지_못해도_레코드는_지우고_던지지_않는다() {
    if !cfg!(windows) {
        return; // 열린 파일이 폴더 삭제를 막는 것은 Windows 의 동작이다(Pack 핸들)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let rig = Rig::new(true);
        ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }])).await);
        let dir = cache_of(&rig).join("octo").join("hello.git");
        // 다른 프로세스가 쥐고 있는 파일(삭제 공유 거부)
        let held = fs::OpenOptions::new().read(true).share_mode(0).open(dir.join("HEAD")).expect("파일 열기");
        let r = rig.call("repos:remove", json!(["id-1"])).await;
        assert_eq!(names(&cfg(&r), "repos").len(), 0, "폴더를 못 지워도 레코드는 지운다");
        assert!(dir.exists(), "쥐고 있는 폴더는 남는다");
        assert!(matches!(rig.mirror.resolve("id-1"), Resolved::Unserved));
        drop(held);
        // 남은 폴더는 다음 시작 때 고아 정리가 지운다
        assert_eq!(cleanup_github_cache(rig.host.store(), rig.host.cache_dir(), rig.host.locks()).await, 1);
        assert!(!dir.exists());
    }
}

#[tokio::test]
async fn 로드_때_버려진_레코드의_폴더도_고아로_지운다() {
    // 링크가 틀린 github 레코드(owner 가 `..`)는 로드가 버린다 — 그 레코드의 폴더는 아무 레코드도 가리키지 않는 고아다
    let text = r#"{"version":1,"sources":[{"id":"local","kind":"local","label":"Local"},{"id":"github","kind":"github","label":"GitHub"}],"repos":[{"id":"bad","sourceId":"github","path":"C:\\x\\zed\\repo.git","name":"x","github":{"owner":"..","repo":"repo"}}],"ui":{}}"#;
    let rig = Rig::with_file(true, Some(text));
    assert!(!rig.store.load_failed());
    assert_eq!(names(&rig.store.get(), "repos").len(), 0, "링크가 틀린 레코드는 버려진다");
    let orphan = cache_of(&rig).join("zed").join("repo.git");
    fs::create_dir_all(orphan.join("objects")).unwrap();
    assert_eq!(cleanup_github_cache(rig.host.store(), rig.host.cache_dir(), rig.host.locks()).await, 1);
    assert!(!orphan.exists() && !cache_of(&rig).join("zed").exists());
}

// ── external:* ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn 외부_열기는_설정의_저장소만_열고_입력과_원격_전용_저장소를_거부한다() {
    let rig = Rig::new(true);
    let a = rig.repo("a");
    ok(&rig.call("repos:add", json!([{ "kind": "local", "path": a.to_string_lossy() }])).await);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }])).await);
    fail(&rig.call("external:open", json!(["id-1"])).await, "잘못된 입력입니다");
    fail(&rig.call("external:open", json!(["id-1", { "action": "format" }])).await, "잘못된 입력입니다");
    fail(&rig.call("external:open", json!(["id-1", { "action": "editor", "path": null }])).await, "잘못된 입력입니다");
    fail(&rig.call("external:open", json!(["id-1", { "action": "editor", "path": 5 }])).await, "잘못된 입력입니다");
    fail(&rig.call("external:open", json!(["nope", { "action": "fileManager" }])).await, "저장소를 찾을 수 없습니다");
    fail(&rig.call("external:open", json!([5, { "action": "fileManager" }])).await, "저장소를 찾을 수 없습니다");
    fail(&rig.call("external:open", json!(["id-2", { "action": "fileManager" }])).await, "원격 전용 저장소는 작업 폴더가 없어 열 수 없습니다");
    // 저장소 밖으로 나가는 상대 경로는 계획 단계에서 거부된다(프로그램을 띄우지 않는다)
    let r = rig.call("external:open", json!(["id-1", { "action": "terminal", "path": "..\\..\\x" }])).await;
    assert_eq!(r["ok"], false, "{r}");
    assert!(rig.deps.spawned.lock().unwrap().is_empty() && rig.deps.opened.lock().unwrap().is_empty() && rig.deps.shown.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 외부_열기는_저장소_루트를_설정에서_읽어_파일_관리자와_에디터를_연결한다() {
    let rig = Rig::new(false);
    let a = rig.repo("a");
    fs::create_dir_all(a.join("src")).unwrap();
    fs::write(a.join("src").join("x.rs"), "fn main() {}").unwrap();
    let top = ok(&rig.call("repos:add", json!([{ "kind": "local", "path": a.to_string_lossy() }])).await)["path"].as_str().unwrap().to_string();
    // 폴더 열기: 설정의 루트(링크를 푼 실제 경로)를 파일 관리자에 준다. renderer 는 경로를 정하지 못한다
    ok(&rig.call("external:open", json!(["id-1", { "action": "fileManager" }])).await);
    assert_eq!(rig.deps.opened.lock().unwrap().iter().map(|p| canon(p)).collect::<Vec<_>>(), [canon(&top)]);
    // 파일이면 열지 않고 폴더에서 보여 준다
    ok(&rig.call("external:open", json!(["id-1", { "action": "fileManager", "path": "src/x.rs" }])).await);
    assert_eq!(rig.deps.shown.lock().unwrap().iter().map(|p| canon(p)).collect::<Vec<_>>(), [canon(&a.join("src").join("x.rs").to_string_lossy())]);
    // 에디터: 고른 실행 파일이 없으면 안내 오류, 고르면 그 파일을 절대 경로 인자로 띄운다
    if cfg!(windows) {
        ok(&rig.call("config:setUi", json!([{ "editor": "custom" }])).await);
        let r = rig.call("external:open", json!(["id-1", { "action": "editor" }])).await;
        fail(&r, "에디터 실행 파일을 먼저 고르세요(설정)");
        let exe = rig.dir.path().join("my-editor.exe");
        fs::write(&exe, "MZ").unwrap();
        *rig.pick.lock().unwrap() = Some(exe.to_string_lossy().into_owned());
        let picked = rig.call("external:pickEditorExe", json!([])).await;
        assert_eq!(ok(&picked)["ui"]["editorExePath"], exe.to_string_lossy().as_ref());
        ok(&rig.call("external:open", json!(["id-1", { "action": "editor", "path": "src/x.rs" }])).await);
        let spawned = rig.deps.spawned.lock().unwrap().clone();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].file, exe.to_string_lossy());
        assert!(spawned[0].args.iter().all(|a| !a.starts_with('-')) && spawned[0].args.iter().any(|a| a.ends_with("x.rs")), "{:?}", spawned[0].args);
        assert_eq!(spawned[0].env_remove, ["ELECTRON_RUN_AS_NODE"]);
    }
}

#[tokio::test]
async fn 에디터_실행_파일_선택은_확인한_뒤에만_설정에_저장한다() {
    let rig = Rig::new(false);
    // 취소: null, 파일을 쓰지 않는다
    assert_eq!(rig.call("external:pickEditorExe", json!([])).await, json!({ "ok": true, "data": null }));
    assert!(!rig.dir.path().join("antler.config.json").exists());
    if !cfg!(windows) {
        return;
    }
    let exe = rig.dir.path().join("idea64.EXE");
    fs::write(&exe, "MZ").unwrap();
    for (picked, msg) in [
        ("relative\\x.exe".to_string(), "로컬 폴더의 실행 파일만 고를 수 있습니다"),
        (rig.dir.path().join("a.cmd").to_string_lossy().into_owned(), ".exe 파일만 고를 수 있습니다(.cmd·.bat 은 안전하게 실행할 수 없습니다)"),
        (rig.dir.path().join("missing.exe").to_string_lossy().into_owned(), "실행 파일을 찾을 수 없습니다. 설정에서 다시 선택하세요"),
        ("\\\\host\\share\\x.exe".to_string(), "로컬 폴더의 실행 파일만 고를 수 있습니다"),
    ] {
        *rig.pick.lock().unwrap() = Some(picked);
        fail(&rig.call("external:pickEditorExe", json!([])).await, msg);
        assert!(!rig.dir.path().join("antler.config.json").exists(), "{msg}: 거부된 선택은 저장하지 않는다");
    }
    *rig.pick.lock().unwrap() = Some(exe.to_string_lossy().into_owned());
    let r = rig.call("external:pickEditorExe", json!([])).await;
    assert_eq!(ok(&r)["ui"]["editorExePath"], exe.to_string_lossy().as_ref());
    assert_eq!(rig.file_json().get("ui").and_then(|u| u.get("editorExePath")).and_then(Json::as_str), exe.to_str());
}

// ── 소스 검사 ───────────────────────────────────────────────────────────────

/// 지금 설정의 원격 전용 저장소 id 들
fn github_ids(rig: &Rig) -> Vec<String> {
    match rig.store.get() {
        Json::Obj(cfg) => match cfg.iter().find(|(k, _)| k == "repos") {
            Some((_, Json::Arr(repos))) => repos.iter().filter_map(|r| r.get("id").and_then(Json::as_str).map(str::to_string)).collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 삭제와_재추가가_겹쳐도_새_레코드의_캐시를_지우지_않고_레코드와_폴더가_함께_있거나_함께_없다() {
    let rig = Rig::new(true);
    let dir = cache_of(&rig).join("octo").join("hello.git");
    let add = json!([{ "repos": [{ "owner": "octo", "repo": "hello" }] }]);
    for round in 0..6 {
        ok(&rig.call("repos:addGithub", add.clone()).await);
        let id = github_ids(&rig).first().cloned().expect("등록된 저장소");
        // 같은 저장소를 두 번 지우면서 같은 owner/repo 를 다시 추가한다(`repos:remove` 와 `repos:addGithub` 겹침)
        let (a, b, c) = tokio::join!(rig.call("repos:remove", json!([id.clone()])), rig.call("repos:remove", json!([id])), rig.call("repos:addGithub", add.clone()));
        for r in [&a, &b, &c] {
            assert_eq!(r["ok"], true, "{round}: {r}");
        }
        // 불변식: 레코드가 있으면 폴더도 있고 bare 저장소다(새 레코드의 캐시가 지워지지 않았다). 레코드가 없으면 폴더도 없다(삭제 중 레코드를 등록된 것으로 보고 건너뛰지 않았다)
        let ids = github_ids(&rig);
        assert!(ids.len() <= 1, "{round}: 레코드가 둘 이상이다 {ids:?}");
        if ids.is_empty() {
            assert!(!dir.exists(), "{round}: 레코드가 없는데 폴더가 남았다");
        } else {
            assert!(is_bare(&dir), "{round}: 레코드가 있는데 캐시가 없거나 깨졌다");
            assert!(matches!(rig.mirror.resolve(&ids[0]), Resolved::Rust(Target::Github { .. })));
        }
        // 다음 회차의 시작 상태를 맞춘다
        for id in ids {
            ok(&rig.call("repos:remove", json!([id])).await);
        }
        assert!(!dir.exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 서로_다른_저장소의_삭제와_추가는_서로_막지_않고_각자_폴더를_지키며_삭제_중인_레코드를_등록된_것으로_보지_않는다() {
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "keep" }, { "owner": "octo", "repo": "gone" }] }])).await);
    let ids = github_ids(&rig);
    let (r, a) = tokio::join!(rig.call("repos:remove", json!([ids[1].clone()])), rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "gone" }, { "owner": "octo", "repo": "new" }] }])));
    assert_eq!((r["ok"].clone(), a["ok"].clone()), (json!(true), json!(true)));
    let cache = cache_of(&rig);
    assert!(is_bare(&cache.join("octo").join("keep.git")) && is_bare(&cache.join("octo").join("new.git")));
    // `gone` 은 삭제와 추가 중 어느 쪽이 이겼든 레코드와 폴더가 함께 있거나 함께 없다
    let has_record = names(&rig.file_json(), "repos").iter().any(|n| n == "gone");
    assert_eq!(has_record, cache.join("octo").join("gone.git").exists());
}

#[tokio::test]
async fn 설정에_든_unc_와_nt_네임스페이스_루트는_열기를_거부하고_프로그램을_띄우지_않는다() {
    let text = r#"{"version":1,"sources":[{"id":"local","kind":"local","label":"Local"}],"repos":[{"id":"u1","sourceId":"local","path":"\\\\127.0.0.1\\share\\r","name":"u1"},{"id":"u2","sourceId":"local","path":"\\??\\UNC\\127.0.0.1\\share\\r","name":"u2"},{"id":"u3","sourceId":"local","path":"C:\\ok\u0000bad","name":"u3"}],"ui":{}}"#;
    let rig = Rig::with_file(false, Some(text));
    let net = "네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)";
    for (id, action, msg) in [("u1", "fileManager", net), ("u1", "editor", net), ("u1", "terminal", net), ("u2", "fileManager", net), ("u3", "terminal", "잘못된 경로입니다")] {
        fail(&rig.call("external:open", json!([id, { "action": action }])).await, msg);
    }
    assert!(rig.deps.spawned.lock().unwrap().is_empty() && rig.deps.opened.lock().unwrap().is_empty() && rig.deps.shown.lock().unwrap().is_empty());
}

#[tokio::test]
async fn 고아_정리는_userdata_경로에_점점과_중복_구분자가_있어도_등록된_폴더를_지키고_고아만_지운다() {
    if !cfg!(windows) {
        return;
    }
    let rig = Rig::new(true);
    ok(&rig.call("repos:addGithub", json!([{ "repos": [{ "owner": "octo", "repo": "keep" }] }])).await);
    let cache = cache_of(&rig);
    fs::create_dir_all(cache.join("octo").join("stale.git").join("objects")).unwrap();
    // 같은 폴더를 `..`·중복 구분자로 가리키는 기준 폴더
    let weird = format!("{}\\..\\remote-repos\\\\", cache.to_string_lossy());
    assert_eq!(cleanup_github_cache(rig.host.store(), &weird, rig.host.locks()).await, 1);
    assert!(is_bare(&cache.join("octo").join("keep.git")), "등록된 폴더가 고아로 지워졌다");
    assert!(!cache.join("octo").join("stale.git").exists());
}

#[tokio::test]
async fn 추가_실패_목록의_이름은_js_string_변환_규칙을_따른다() {
    let rig = Rig::new(true);
    let r = rig.call("repos:addGithub", json!([{ "repos": [{ "owner": ["a", ["b", null], "c"], "repo": "x" }, { "owner": null, "repo": 5 }, { "owner": true, "repo": { "k": 1 } }, { "repo": "only" }, 7] }])).await;
    let labels: Vec<String> = ok(&r)["failed"].as_array().unwrap().iter().map(|f| f["fullName"].as_str().unwrap().to_string()).collect();
    assert_eq!(labels, ["a,b,,c/x", "null/5", "true/[object Object]", "undefined/only", "?"]);
}

#[tokio::test]
async fn wsl_저장소_추가는_대소문자만_다른_기존_소스를_재사용해_소스와_레코드가_갈라지지_않는다() {
    if !cfg!(windows) {
        return;
    }
    // Electron 에서 가져온 소스가 `ubuntu` 로 적혀 있다(설치 목록의 표기는 `Ubuntu`)
    let text = r#"{"version":1,"sources":[{"id":"local","kind":"local","label":"Local"},{"id":"wsl:ubuntu","kind":"wsl","label":"WSL · ubuntu","distro":"ubuntu"}],"repos":[],"ui":{}}"#;
    let rig = Rig::with_file(false, Some(text));
    let a = rig.repo("a");
    // 실행기는 가짜(로컬 git)라 임시 폴더로 추가를 끝까지 돌린다
    let r = rig.call("repos:add", json!([{ "kind": "wsl", "distro": "UBUNTU", "path": a.to_string_lossy() }])).await;
    assert_eq!(ok(&r)["sourceId"], "wsl:ubuntu", "{r}");
    let again = rig.call("repos:add", json!([{ "kind": "wsl", "distro": "Ubuntu", "path": a.to_string_lossy() }])).await;
    assert_eq!(ok(&again)["id"], ok(&r)["id"], "같은 저장소가 둘로 갈라졌다");
    assert_eq!(names(&rig.file_json(), "sources"), ["local", "wsl:ubuntu"]);
    assert_eq!(names(&rig.file_json(), "repos").len(), 1);
}

#[test]
fn 설정_파일을_여는_곳은_앱_시작의_한_곳이다() {
    // 배선 순서: 변경 알림을 단 뒤에 라우터를 등록하고, 첫 반영은 라우터 등록 뒤다. D1 가져오기 판정은 `should_import` 한 곳이다
    let lib = prod_code("src/lib.rs");
    let listen = lib.find("config_store.set_on_change(").expect("변경 알림");
    let manage = lib.find("app.manage(router)").expect("라우터 등록");
    let first = lib.find("config_sync.push(None,").expect("첫 반영");
    assert!(listen < manage && manage < first);
    assert!(lib.contains("config::import::should_import(std::env::var_os(\"ANTLER_USER_DATA\").as_deref(), &file)"));
    assert_eq!(lib.matches("import_electron_config(").count(), 1);
    // ConfigStore 를 만드는 호출은 `open_config` 안 한 곳이고, 그 함수는 앱 시작에서 한 번만 부른다
    assert_eq!(lib.matches("ConfigStore::open(").count(), 1);
    assert_eq!(lib.matches("open_config(").count(), 2, "정의 하나 + 호출 하나");
    assert!(lib.contains("Some(cache_dir.to_string())"), "계약: ConfigStore::open 의 cache_dir 는 반드시 Some");
    for rel in ["src/router.rs", "src/env_host.rs", "src/app_host.rs", "src/watch_host.rs", "src/api_host.rs"] {
        assert!(!prod_code(rel).contains("ConfigStore::open"), "{rel} 가 설정 파일을 연다");
    }
    // 설정 소유 채널의 가로채기는 배열이 아닌 인자를 거르는 `as_array() else` 보다 앞이다
    let router = prod_code("src/router.rs");
    let intercept = router.find("CONFIG_CHANNELS.contains(&channel)").expect("가로채기");
    let array_guard = router.find("let Some(raw) = args.as_array() else { return Routed::Unserved };").expect("배열 검사");
    assert!(intercept < array_guard);
}

#[test]
fn 설정_소유자는_승인을_쓰지_않고_삭제는_링크_검사_뒤에_한다() {
    // 승인 변경자는 fetch 호스트 한 곳만 부른다(채널 처리기 어디에도 없다 — `config:setUi` 로는 승인을 위조할 수 없다)
    let host = prod_code("src/config_host.rs");
    for banned in ["set_fetch_approval", "set_trusted_config", "set_github_default_branch", "set_skipped_update_version"] {
        assert!(!host.contains(banned), "config_host.rs 가 {banned} 를 부른다");
    }
    // repos:remove 의 순서: (fetch 정지·감시 정지·조회 드레인) → 링크·경로 일치 검사 → 폴더 삭제 → 레코드 삭제
    let release = host.find("router.release_github_repo(&id).await").expect("정리");
    let target = host.find("github_target_of(&host.store").expect("링크 검사");
    let lock = host.find("host.locks.lock(&canonical_key(").expect("경로 잠금");
    let reread = host.find("host.store.repo(&id).and_then(|r| github_target_of").expect("잠금 안 재확인");
    let remove = host.find("remove_cache_dir(&target)").expect("폴더 삭제");
    let record = host.find("store.remove_repo(&id)").expect("레코드 삭제");
    // 경로 잠금 안에서 레코드를 다시 확인한 뒤 폴더·레코드를 모두 지운다(잠금 밖 별도 삭제 없음)
    assert!(release < target && target < lock && lock < reread && reread < remove && remove < record);
    assert_eq!(host.matches("store.remove_repo(&id)").count(), 2, "github 갈래(잠금 안)와 그 밖의 갈래");
    // 폴더를 지우는 함수는 `GithubCachePath` 만 받고 직접 `remove_dir_all` 을 쓰는 곳은 github_cache.rs 한 곳이다
    for rel in ["src/config_host.rs", "src/repos/manage.rs", "src/router.rs", "src/lib.rs"] {
        assert!(!prod_code(rel).contains("remove_dir_all"), "{rel} 가 폴더를 직접 지운다");
    }
    // 미러의 링크·기본 브랜치는 `apply_message` 의 검증 경로로만 들어간다(`apply_with_links`·`apply_with_branches` 는 시험·이 경로 밖에서 쓰지 않는다)
    for rel in ["src/config/sync.rs", "src/config_host.rs", "src/lib.rs", "src/fetch_host.rs"] {
        let code = prod_code(rel);
        assert!(!code.contains("apply_with_links") && !code.contains("apply_with_branches"), "{rel} 가 검증 없는 미러 진입점을 쓴다");
    }
    assert!(prod_code("src/config/sync.rs").contains("apply_message(&repos_message("));
}

#[test]
fn 프로세스를_띄우는_곳은_주입_하나이고_설정_소유자는_실행기를_만들지_않는다() {
    let host = prod_code("src/config_host.rs");
    for banned in ["Command::new", "std::process", "tokio::process", "launch::", "SystemDeps", "tauri_plugin"] {
        assert!(!host.contains(banned), "config_host.rs 에 {banned}");
    }
    let manage = prod_code("src/repos/manage.rs");
    assert!(!manage.contains("Command::new") && !manage.contains("std::process"));
    // 확인 창의 승인 기억은 EnvHost 하나 — 소유자가 자기 승인 집합을 두지 않는다
    assert!(!host.contains("HashSet") && !host.contains("approved"));
    // 같은 EnvHost 의 복제를 라우터(env 채널)와 소유자가 나눈다
    let lib = prod_code("src/lib.rs");
    assert!(lib.contains("router.set_env(env_host.clone())") && lib.contains("env_host.clone(), cache_dir.clone()"));
}

