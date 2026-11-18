//! 채널 패리티 통합 테스트. TS(`tests/goldenChannels.test.ts`)가 같은 fixture(`tests/fixtures/repos/*.fi`)·시나리오로 만든 골든
//! (`tests/golden/channels/commit.json`·`log.json`·`logArgs.json`·`compare.json`·`fileDiff.json`·`fileHistory.json`·`blame.json`)을 Rust 라우터의 Rust 경로(`Router::route`)가 그대로 재현하는지 본다. 오류는 code 만 비교한다.
//! 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts`.
//!
//! 격리: `tests/exec.rs` 의 `isolate` 와 같다 — 이 테스트 바이너리가 시작될 때 `GIT_CONFIG_GLOBAL`(커밋된 `src-tauri/tests/gitconfig`)·
//! `GIT_CONFIG_NOSYSTEM`·`GCM_INTERACTIVE` 를 프로세스 환경에 넣어 사용자의 실제 git 설정을 읽지 않는다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use antler_lib::git::exec::ssh::SshPool;
use antler_lib::git::exec::{GitExecutor, LocalExecutor};
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::service::{ApprovalKind, FetchHost, FetchService, HostEvent};
use antler_lib::git::log_service::NoteOp;
use antler_lib::git::scope::{log_args, parse_scope};
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::mirror::{MirrorRepo, RepoMirror};
use antler_lib::router::{Routed, Router, RUST_CHANNELS, UNDEFINED_ARG};
use antler_lib::routed_repos::RoutedRepos;
use futures_util::future::{BoxFuture, FutureExt};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

// jira:*(PR 6)·gh:*(PR 9) 채널 골든. 루프백 http 를 허용하는 테스트 훅이 필요해 `test-hooks` feature 가 있을 때만 빌드한다(CI 는 항상 켠다). 목 서버는 둘이 나눠 쓴다
#[cfg(feature = "test-hooks")]
#[path = "channels/mock.rs"]
mod mock;
#[cfg(feature = "test-hooks")]
#[path = "channels/jira.rs"]
mod jira;
#[cfg(feature = "test-hooks")]
#[path = "channels/github.rs"]
mod github;

// SSH 대상 시나리오(Phase 5 PR 3): 같은 골든을 127.0.0.1 의 프로세스 안 russh 서버 뒤에서 한 번 더 돈다. 임시 홈·`AgentSetting::Disabled` 만 쓴다(사용자의 ~/.ssh·agent 를 건드리지 않는다)
#[path = "ssh/common.rs"]
mod ssh_common;
#[path = "ssh/server.rs"]
mod ssh_server;

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    let global = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig");
    std::env::set_var("GIT_CONFIG_GLOBAL", global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
    // git:fetch 시나리오가 루프백 서버(127.0.0.1)에 붙는다 — 사용자의 프록시·자격 증명 환경을 타지 않게 한다
    for k in [
        "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH", "GIT_SSH_COMMAND", "GIT_ALLOW_PROTOCOL",
        "GIT_TERMINAL_PROMPT",
    ] {
        std::env::remove_var(k);
    }
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri 의 상위").to_path_buf()
}

fn read_json(rel: &str) -> Value {
    let text = fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 테스트 준비용 날 프로세스(저장소 만들기·해시 풀기). **제품 코드는 이 길을 쓰지 않는다**(clippy.toml 의 disallowed-methods)
#[allow(clippy::disallowed_methods)]
fn raw_git(dir: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("git");
    c.current_dir(dir);
    c
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("antler-chan-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        for _ in 0..10 {
            if fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// `<repo>.fi` 를 `git init` 한 새 디렉터리에 import 한다
fn import(root: &Path, repo: &str) -> PathBuf {
    import_at(root.join(repo), repo)
}

/// `<repo>.fi` 를 `dir`(없으면 만든다)에 import 한다
fn import_at(dir: PathBuf, repo: &str) -> PathBuf {
    fs::create_dir_all(&dir).unwrap();
    let ok = raw_git(&dir).args(["init", "-q", "-b", "main"]).status().unwrap().success();
    assert!(ok, "git init");
    let fi = fs::File::open(repo_root().join("tests/fixtures/repos").join(format!("{repo}.fi"))).unwrap();
    let ok = raw_git(&dir).args(["fast-import", "--quiet"]).stdin(Stdio::from(fi)).status().unwrap().success();
    assert!(ok, "git fast-import");
    dir
}

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = raw_git(dir).args(["rev-parse", rev]).output().unwrap();
    assert!(out.status.success(), "rev-parse {rev}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// 시나리오를 도는 대상. Local 은 라우터가 이 PC 의 git 을 직접 돌리고, Ssh 는 같은 저장소를 루프백 SSH 서버(`~/.ssh/config` 의 `Host test`)를 거쳐 돌린다.
/// 서버·연결 풀은 시험 하나가 공유한다(시나리오마다 라우터를 새로 만들어도 연결은 하나다)
enum Mode {
    Local,
    Ssh(SshRig),
}

struct SshRig {
    server: ssh_server::TestServer,
    pool: Arc<SshPool>,
    _home: Arc<ssh_common::Home>,
}

impl Mode {
    async fn ssh() -> Mode {
        let client = ssh_server::new_key();
        let server = ssh_server::TestServer::start(ssh_server::Opts::new(vec![client.public_key().clone()])).await;
        let home = Arc::new(ssh_common::Home::new(&client));
        home.write_config(&format!(
            "Host test\n  HostName 127.0.0.1\n  Port {}\n  User tester\n  IdentityFile {}\n  IdentitiesOnly yes\n",
            server.port,
            home.key_for_config()
        ));
        home.write_known_hosts(&ssh_common::known_hosts_line(server.port, &server.host_key));
        let h = home.clone();
        let pool = SshPool::new(move |host| h.executor(host));
        Mode::Ssh(SshRig { server, pool, _home: home })
    }

    /// 이 대상으로 `entries`(id, 경로)를 여는 라우터
    fn router(&self, entries: &[(&str, &Path)]) -> Router {
        match self {
            Mode::Local => Router::new(true, false, mirror_of(entries)),
            Mode::Ssh(rig) => {
                let m = RepoMirror::default();
                let repos = entries
                    .iter()
                    .map(|(id, p)| MirrorRepo {
                        id: (*id).to_string(),
                        kind: "ssh".into(),
                        path: p.to_string_lossy().into_owned(),
                        distro: None,
                        ssh_host: Some("test".into()),
                    })
                    .collect();
                m.apply(1, repos);
                let router = Router::new(true, false, m);
                router.set_ssh_pool(rig.pool.clone());
                assert!(router.ssh_active());
                router
            }
        }
    }

    /// 끝낼 때: 풀이 호스트당 연결 하나를 나눠 썼는지 보고 SSH 연결을 닫는다
    fn finish(&self) {
        if let Mode::Ssh(rig) = self {
            assert_eq!(rig.server.connections(), 1, "풀이 호스트당 연결 하나를 나눠 쓴다");
            rig.pool.dispose_all();
        }
    }
}

fn mirror_of(entries: &[(&str, &Path)]) -> RepoMirror {
    let m = RepoMirror::default();
    let repos = entries
        .iter()
        .map(|(id, p)| MirrorRepo { id: (*id).to_string(), kind: "local".into(), path: p.to_string_lossy().into_owned(), distro: None, ssh_host: None })
        .collect();
    m.apply(1, repos);
    m
}

fn envelope(r: Routed) -> Value {
    match r {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("Rust 경로를 타야 하는데 사이드카로 갔다"),
    }
}

/// TS 골든과 같은 규칙: 파일이 많으면 개수와 처음·마지막만 남기고, 오류는 code 만 남긴다
fn normalize(env: &Value) -> Value {
    if env["ok"] == true {
        let mut data = env["data"].clone();
        let files = data["files"].as_array().expect("files").clone();
        if files.len() > 20 {
            data["files"] = json!({ "count": files.len(), "first": files[0], "last": files[files.len() - 1] });
        }
        json!({ "ok": true, "data": data })
    } else {
        json!({ "ok": false, "code": env["code"] })
    }
}

#[test]
fn 격리() {
    assert!(std::env::var_os("GIT_CONFIG_GLOBAL").is_some_and(|v| v.to_string_lossy().ends_with("gitconfig")));
    assert_eq!(std::env::var("GIT_CONFIG_NOSYSTEM").as_deref(), Ok("1"));
}

#[test]
fn rust_채널은_ipc_채널의_부분집합이다() {
    let golden = read_json("tests/golden/exec/channels.json");
    let all: Vec<&str> = golden["channels"].as_array().expect("channels").iter().map(|c| c.as_str().expect("채널 이름")).collect();
    for ch in RUST_CHANNELS {
        assert!(all.contains(ch), "RUST_CHANNELS 의 {ch} 가 IPC_CHANNELS 에 없다(tests/golden/exec/channels.json)");
    }
    assert!(all.contains(&"req:cancel"), "라우터가 특별 취급하는 req:cancel 이 IPC_CHANNELS 에 없다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_commit_시나리오가_ts_골든과_같다() {
    commit_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_commit_시나리오가_ssh_에서도_ts_골든과_같다() {
    commit_scenarios(&Mode::ssh().await).await;
}

async fn commit_scenarios(mode: &Mode) {
    let scenarios = read_json("tests/fixtures/repos/commit.scenarios.json");
    let golden = read_json("tests/golden/channels/commit.json");
    let scenarios = scenarios["scenarios"].as_array().expect("scenarios");
    let golden = golden["scenarios"].as_array().expect("golden scenarios");
    assert_eq!(scenarios.len(), golden.len(), "시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");

    let tmp = TempDir::new("golden");
    let mut names: Vec<String> = scenarios.iter().map(|s| s["repo"].as_str().unwrap().to_string()).collect();
    names.sort();
    names.dedup();
    let dirs: Vec<(String, PathBuf)> = names.iter().map(|n| (n.clone(), import(&tmp.0, n))).collect();
    let entries: Vec<(&str, &Path)> = dirs.iter().map(|(n, d)| (n.as_str(), d.as_path())).collect();
    let router = mode.router(&entries);

    let mut diffs = Vec::new();
    for (i, (s, g)) in scenarios.iter().zip(golden).enumerate() {
        let repo = s["repo"].as_str().unwrap();
        let dir = &dirs.iter().find(|(n, _)| n == repo).unwrap().1;
        let hash: Value = match s.get("rev").and_then(Value::as_str) {
            Some(rev) => {
                let mut h = rev_parse(dir, rev);
                if let Some(n) = s.get("short").and_then(Value::as_u64) {
                    h.truncate(n as usize);
                }
                if s.get("upper") == Some(&json!(true)) {
                    h = h.to_uppercase();
                }
                let (pre, suf) = (s.get("prefix").and_then(Value::as_str).unwrap_or(""), s.get("suffix").and_then(Value::as_str).unwrap_or(""));
                json!(format!("{pre}{h}{suf}"))
            }
            None => s["hash"].clone(),
        };
        let name = s["name"].as_str().unwrap();
        assert_eq!(g["name"], s["name"], "{name}: 골든의 시나리오 순서가 다르다");
        // 해시(보낸 값)가 TS 와 같아야 같은 입력이다 — 같은 .fi 라 해시가 결정적이다
        assert_eq!(g["hash"], hash, "{name}: 해시가 다르다(fixture 가 바뀌었는가)");
        let env = envelope(router.route("git:commit", &json!([repo, hash, format!("req-{i}")])).await);
        let got = normalize(&env);
        if got != g["result"] {
            diffs.push(format!("{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&g["result"])));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!(router.rust_handled() as usize, scenarios.len());
    mode.finish();
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 600 {
        format!("{}…", s.chars().take(600).collect::<String>())
    } else {
        s
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_해시는_git_을_띄우지도_파일을_쓰지도_않는다() {
    let tmp = TempDir::new("hostile");
    let dir = import(&tmp.0, "basic");
    let router = Router::new(true, false, mirror_of(&[("basic", dir.as_path())]));
    let cwd_before = fs::read_dir(".").unwrap().count();
    for h in ["--output=pwned.txt", "-calias.x=!echo>pwned2.txt", "-x", "--output", "-c", "HEAD", "main", "abc\n"] {
        let env = envelope(router.route("git:commit", &json!(["basic", h, "r"])).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{h}");
    }
    for p in [dir.join("pwned.txt"), dir.join("pwned2.txt"), PathBuf::from("pwned.txt"), PathBuf::from("pwned2.txt")] {
        assert!(!p.exists(), "{p:?}");
    }
    assert_eq!(fs::read_dir(".").unwrap().count(), cwd_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 같은_해시_요청은_합쳐지고_결과는_캐시에서_나온다() {
    let tmp = TempDir::new("cache");
    let dir = import(&tmp.0, "basic");
    let hash = rev_parse(&dir, "main~3");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("basic", dir.as_path())])));
    // 동시에 여러 번(대소문자 섞어서) — 모두 같은 결과
    let mut tasks = Vec::new();
    for i in 0..8 {
        let r = router.clone();
        let h = if i % 2 == 0 { hash.clone() } else { hash.to_uppercase() };
        tasks.push(tokio::spawn(async move { envelope(r.route("git:commit", &json!(["basic", h, format!("c{i}")])).await) }));
    }
    let mut results = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    assert_eq!(results[0]["ok"], true);
    assert!(results.iter().all(|r| *r == results[0]));
    // 저장소를 지워도 캐시된 결과가 나온다(해시 기준 불변 캐시)
    drop(tmp);
    if !dir.exists() {
        let again = envelope(router.route("git:commit", &json!(["basic", hash])).await);
        assert_eq!(again, results[0]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_요청을_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("cancel");
    let dir = import(&tmp.0, "many");
    let hash = rev_parse(&dir, "main");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("many", dir.as_path())])));
    let r = router.clone();
    let h = hash.clone();
    let task = tokio::spawn(async move { envelope(r.route("git:commit", &json!(["many", h, "slow"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(matches!(router.route("req:cancel", &json!(["slow"])).await, Routed::Sidecar));
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
    // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
    match env["ok"].as_bool() {
        Some(true) => {}
        _ => assert_eq!(env["code"], "ABORTED"),
    }
    // 취소한 뒤에도 같은 해시를 다시 받을 수 있다(합쳐진 요청이 오염되지 않는다)
    let again = envelope(router.route("git:commit", &json!(["many", hash])).await);
    assert_eq!(again["ok"], true);
}

#[tokio::test]
async fn 사이드카로_가야_하는_요청은_건드리지_않는다() {
    let tmp = TempDir::new("side");
    let dir = import(&tmp.0, "basic");
    let router = Router::new(true, false, mirror_of(&[("basic", dir.as_path())]));
    assert!(matches!(router.route("git:commit", &json!(["unknown-id", "abcd1234"])).await, Routed::Sidecar));
    // 사이드카에 남는 비 git 채널(PR 6·7 까지 남는 git 채널은 피한다)
    assert!(matches!(router.route("config:get", &json!(["basic"])).await, Routed::Sidecar));
    let off = Router::new(false, false, mirror_of(&[("basic", dir.as_path())]));
    assert!(matches!(off.route("git:commit", &json!(["basic", "abcd1234"])).await, Routed::Sidecar));
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

// ── git:log ────────────────────────────────────────────

/// 시나리오 값의 '@rev:<식>'·$gen·$str·$upper·$short·$cat 을 푼다(`tests/goldenChannels.test.ts` 의 `expand` 와 같은 규칙)
fn expand(v: &Value, rev: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => match s.strip_prefix("@rev:") {
            Some(expr) => json!(rev(expr)),
            None => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| expand(x, rev)).collect()),
        Value::Object(o) if o.len() == 1 && o.contains_key("$gen") => {
            let a = o["$gen"].as_array().unwrap();
            let (prefix, n) = (a[0].as_str().unwrap(), a[1].as_u64().unwrap());
            Value::Array((0..n).map(|i| json!(format!("{prefix}{i}"))).collect())
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("$upper") => json!(expand(&o["$upper"], rev).as_str().unwrap().to_uppercase()),
        Value::Object(o) if o.len() == 1 && o.contains_key("$short") => {
            let a = o["$short"].as_array().unwrap();
            json!(expand(&a[0], rev).as_str().unwrap().chars().take(a[1].as_u64().unwrap() as usize).collect::<String>())
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("$cat") => {
            json!(expand(&o["$cat"], rev).as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<String>())
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("$str") => {
            let a = o["$str"].as_array().unwrap();
            json!(format!("{}{}", a[0].as_str().unwrap(), a[1].as_str().unwrap().repeat(a[2].as_u64().unwrap() as usize)))
        }
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x, rev))).collect()),
        _ => v.clone(),
    }
}

/// 한 줄 요약(`tests/goldenChannels.test.ts` 의 `commitLine` 과 같은 모양)
fn commit_line(c: &Value) -> String {
    let parents: Vec<String> = c["parents"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().chars().take(7).collect()).collect();
    let refs: Vec<String> = c["refs"].as_array().unwrap().iter().map(|r| format!("{}:{}", r["kind"].as_str().unwrap(), r["name"].as_str().unwrap())).collect();
    let hash: String = c["hash"].as_str().unwrap().chars().take(12).collect();
    format!("{hash} {} {} {}", parents.join(","), c["subject"].as_str().unwrap(), refs.join(","))
}

fn log_result(env: &Value, full: bool) -> Value {
    if env["ok"] == true {
        let page = &env["data"];
        let commits = page["commits"].as_array().unwrap();
        let commits: Vec<Value> = if full { commits.clone() } else { commits.iter().map(|c| json!(commit_line(c))).collect() };
        json!({ "ok": true, "data": { "commits": commits, "hasMore": page["hasMore"] } })
    } else if env["code"] == "ERROR" {
        json!({ "ok": false, "code": "ERROR", "error": env["error"] })
    } else {
        json!({ "ok": false, "code": env["code"] })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_log_시나리오가_ts_골든과_같다() {
    log_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_log_시나리오가_ssh_에서도_ts_골든과_같다() {
    log_scenarios(&Mode::ssh().await).await;
}

async fn log_scenarios(mode: &Mode) {
    let scenarios = read_json("tests/fixtures/repos/log.scenarios.json");
    let golden = read_json("tests/golden/channels/log.json");
    let scenarios = scenarios["scenarios"].as_array().expect("scenarios");
    let golden = golden["scenarios"].as_array().expect("golden scenarios");
    assert_eq!(scenarios.len(), golden.len(), "시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");

    let tmp = TempDir::new("log");
    let mut diffs = Vec::new();
    for (si, (sc, g)) in scenarios.iter().zip(golden).enumerate() {
        let name = sc["name"].as_str().unwrap();
        assert_eq!(g["name"], sc["name"], "{name}: 골든의 시나리오 순서가 다르다");
        // 시나리오마다 새 디렉터리에 import 한다(저장소를 바꾸는 단계가 다른 시나리오에 번지지 않게)
        let dir = import(&tmp.0.join(format!("s{si}")), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = mode.router(&[(id.as_str(), dir.as_path())]);
        let rev = |expr: &str| rev_parse(&dir, expr);
        let golden_steps = g["steps"].as_array().unwrap();
        let steps = sc["steps"].as_array().unwrap();
        assert_eq!(golden_steps.len(), steps.len(), "{name}: 단계 수가 다르다");
        let mut pages: Vec<Vec<String>> = Vec::new();
        for (i, (step, gs)) in steps.iter().zip(golden_steps).enumerate() {
            if let Some(args) = step.get("git") {
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                let ok = raw_git(&dir).args(&args).status().unwrap().success();
                assert!(ok, "{name}: git {args:?}");
                pages.push(Vec::new());
                continue;
            }
            let query = expand(&step["log"], &rev);
            let env = envelope(router.route("git:log", &json!([id, query, format!("req-{si}-{i}")])).await);
            let hashes = env["data"]["commits"].as_array().map(|c| c.iter().map(|x| x["hash"].as_str().unwrap().to_string()).collect()).unwrap_or_default();
            pages.push(hashes);
            let got = json!({ "log": step["log"], "result": log_result(&env, step.get("full") == Some(&json!(true))) });
            if got != *gs {
                diffs.push(format!("{name} / 단계 {i}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(gs)));
            }
        }
        if let Some(p) = sc.get("pagesEqualFull") {
            let (full, first, last) = (p[0].as_u64().unwrap() as usize, p[1].as_u64().unwrap() as usize, p[2].as_u64().unwrap() as usize);
            let joined: Vec<&String> = pages[first..=last].iter().flatten().collect();
            let all: Vec<&String> = pages[full].iter().collect();
            assert_eq!(joined, all, "{name}: 페이지를 이어 붙인 결과가 전체 한 번과 다르다");
        }
        // 시나리오 안에서 검증이 거부한 요청이 파일을 만들지 않았다
        assert!(!dir.join("pwned.txt").exists() && !Path::new("pwned.txt").exists(), "{name}: pwned.txt");
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    mode.finish();
}

#[test]
fn log_인자가_ts_골든과_같다() {
    let cases = read_json("tests/fixtures/repos/logArgs.cases.json");
    let golden = read_json("tests/golden/channels/logArgs.json");
    let cases = cases["cases"].as_array().expect("cases");
    let golden = golden["cases"].as_array().expect("golden cases");
    assert_eq!(cases.len(), golden.len(), "케이스 수가 골든과 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
    for (c, g) in cases.iter().zip(golden) {
        let name = c["name"].as_str().unwrap();
        let skip = c["skip"].as_u64().unwrap();
        let got = parse_scope(c.get("scope")).and_then(|s| log_args(s.as_ref(), skip));
        let got = match got {
            Ok(args) => json!({ "name": name, "args": args }),
            Err(m) => json!({ "name": name, "error": m }),
        };
        assert_eq!(&got, g, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_범위는_git_을_띄우지도_파일을_쓰지도_않는다() {
    // 저장소 경로가 없다 — 검증이 먼저 걸러야 한다(git 을 띄웠다면 GIT 오류가 난다)
    let nowhere = Path::new("/nonexistent-antler-log");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    let hostile: Vec<Value> = vec![
        json!({ "solo": ["-c"] }),
        json!({ "solo": ["-calias.x=!echo>pwned.txt"] }),
        json!({ "solo": ["--output=pwned.txt"] }),
        json!({ "solo": ["refs/heads/a\n--output=pwned.txt"] }),
        json!({ "solo": ["refs/heads/\u{feff}a"] }),
        json!({ "hide": ["--output=pwned.txt"] }),
        json!({ "hide": ["-c", "refs/heads/a"] }),
        json!({ "hide": [["refs/heads/a"]] }),
        json!({ "solo": "-c" }),
        json!("-c"),
        json!(null),
    ];
    for scope in hostile {
        let env = envelope(router.route("git:log", &json!(["a", { "skip": 0, "count": 10, "scope": scope }, "r"])).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{scope}");
        assert!(env["error"].as_str().unwrap().starts_with("invalid "), "{scope}: {env}");
    }
    // count·skip 는 숫자만(문자열이 git 인자 `--skip=…` 에 들어가지 못한다)
    for q in [json!({ "skip": "0 --output=pwned.txt", "count": 10 }), json!({ "skip": 0, "count": "-c" }), json!({ "skip": [], "count": 10 }), json!(null)] {
        let env = envelope(router.route("git:log", &json!(["a", q, "r"])).await);
        assert_eq!((env["code"].as_str(), env["error"].as_str()), (Some("ERROR"), Some("invalid query")), "{q}");
    }
    assert_eq!(router.log_cursor_count(), 0);
    assert!(!Path::new("pwned.txt").exists());
}

// ── 파일 단위 채널: git:compare · git:fileDiff · git:fileHistory · git:blame ────────────────

const FILE_CHANNELS: [&str; 4] = ["compare", "fileDiff", "fileHistory", "blame"];

/// 숫자를 모두 f64 로 맞춘다(시나리오의 `1.0` 과 TS 골든의 `1` 을 같게 본다)
fn norm_numbers(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64()),
        Value::Array(a) => Value::Array(a.iter().map(norm_numbers).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), norm_numbers(x))).collect()),
        _ => v.clone(),
    }
}

/// diff 의 hunk 줄이 40개를 넘으면 개수·처음·마지막으로 줄인다(`tests/goldenChannels.test.ts` 의 `summarizeDiff` 와 같다)
fn summarize_diff(mut data: Value) -> Value {
    for h in data["hunks"].as_array_mut().expect("hunks") {
        let lines = h["lines"].as_array().expect("lines").clone();
        if lines.len() > 40 {
            h["lines"] = json!({ "count": lines.len(), "first": lines[0], "last": lines[lines.len() - 1] });
        }
    }
    data
}

fn file_result(channel: &str, env: &Value, msg: bool) -> Value {
    if env["ok"] != true {
        return if msg { json!({ "ok": false, "code": env["code"], "error": env["error"] }) } else { json!({ "ok": false, "code": env["code"] }) };
    }
    match channel {
        "compare" => normalize(env),
        "fileDiff" => json!({ "ok": true, "data": summarize_diff(env["data"].clone()) }),
        _ => json!({ "ok": true, "data": env["data"] }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 파일_단위_채널_시나리오가_ts_골든과_같다() {
    file_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 파일_단위_채널_시나리오가_ssh_에서도_ts_골든과_같다() {
    file_scenarios(&Mode::ssh().await).await;
}

async fn file_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/files.scenarios.json");
    let tmp = TempDir::new("files");
    let dirs: Vec<(String, PathBuf)> = ["files", "empty"].iter().map(|n| ((*n).to_string(), import(&tmp.0, n))).collect();
    let entries: Vec<(&str, &Path)> = dirs.iter().map(|(n, d)| (n.as_str(), d.as_path())).collect();
    let router = mode.router(&entries);
    let mut diffs = Vec::new();
    let mut total = 0;
    for channel in FILE_CHANNELS {
        let scenarios = all[channel].as_array().unwrap_or_else(|| panic!("{channel} 시나리오"));
        let golden = read_json(&format!("tests/golden/channels/{channel}.json"));
        let golden = golden["scenarios"].as_array().expect("golden scenarios");
        assert_eq!(scenarios.len(), golden.len(), "{channel}: 시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
        for (i, (s, g)) in scenarios.iter().zip(golden).enumerate() {
            let name = s["name"].as_str().unwrap();
            let repo = s.get("repo").and_then(Value::as_str).unwrap_or("files");
            let dir = &dirs.iter().find(|(n, _)| n == repo).unwrap().1;
            assert_eq!(g["name"], s["name"], "{channel}/{name}: 골든의 시나리오 순서가 다르다");
            let args = expand(&s["args"], &|expr: &str| rev_parse(dir, expr));
            // 보낸 값(해시 포함)이 TS 와 같아야 같은 입력이다 — 같은 .fi 라 해시가 결정적이다
            assert_eq!(norm_numbers(&args), norm_numbers(&g["args"]), "{channel}/{name}: 입력이 다르다(fixture 가 바뀌었는가)");
            let mut call = vec![json!(repo)];
            call.extend(args.as_array().unwrap().iter().cloned());
            // 요청 인자가 없는 시나리오는 requestId 가 그 자리를 채우지 않게 TS 와 같이 undefined 로 비워 둔다
            let wanted = if channel == "compare" { 2 } else { 1 };
            while call.len() < wanted + 1 {
                call.push(json!({ "__antlerUndefinedArg": true }));
            }
            call.push(json!(format!("req-{channel}-{i}")));
            let env = envelope(router.route(&format!("git:{channel}"), &Value::Array(call)).await);
            let got = file_result(channel, &env, s.get("msg") == Some(&json!(true)));
            total += 1;
            if got != g["result"] {
                diffs.push(format!("{channel}/{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&g["result"])));
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    // 검증에서 거부된 요청은 handled 로 세지 않는다(git 까지 간 것만) — 전부 Rust 경로였고(envelope 가 사이드카를 거부한다) 일부는 git 을 돌렸다
    assert!(total > 0 && router.rust_handled() > 0 && router.rust_handled() <= total);
    // 어떤 시나리오도 파일을 만들지 못했다
    for d in dirs.iter().map(|(_, d)| d) {
        assert!(!d.join("pwned.txt").exists() && !Path::new("pwned.txt").exists());
    }
    mode.finish();
}

/// 검증이 git 을 띄우기 전에 거르는 적대적 입력: 저장소 경로가 없어서 git 을 띄웠다면 GIT 오류가 난다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_파일_채널_입력은_git_을_띄우지도_파일을_쓰지도_않는다() {
    let nowhere = Path::new("/nonexistent-antler-files");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    let cwd_before = fs::read_dir(".").unwrap().count();
    let evil_hashes = ["-x", "--output=pwned.txt", "-calias.x=!echo>pwned2.txt", "--output", "-c", "HEAD", "main", "abc\n", "abcd/../..", "abcd\u{0}"];
    let ok_hash = "abcd1234";
    let mut calls: Vec<(&str, Value)> = Vec::new();
    for h in evil_hashes {
        calls.push(("git:compare", json!(["a", h, ok_hash, "r"])));
        calls.push(("git:compare", json!(["a", ok_hash, h, "r"])));
        calls.push(("git:fileDiff", json!(["a", { "hash": h, "path": "a.txt" }, "r"])));
        calls.push(("git:fileDiff", json!(["a", { "hash": ok_hash, "parent": h, "path": "a.txt" }, "r"])));
        calls.push(("git:fileHistory", json!(["a", { "path": "a.txt", "rev": h, "skip": 0, "count": 1 }, "r"])));
        calls.push(("git:blame", json!(["a", { "path": "a.txt", "rev": h }, "r"])));
    }
    for p in [json!(""), json!("a\u{0}b"), json!(null), json!(5), json!(["a"]), json!({ "x": 1 })] {
        calls.push(("git:fileDiff", json!(["a", { "hash": ok_hash, "path": p }, "r"])));
        calls.push(("git:fileDiff", json!(["a", { "hash": ok_hash, "path": "a.txt", "oldPath": p }, "r"])));
        calls.push(("git:fileHistory", json!(["a", { "path": p, "skip": 0, "count": 1 }, "r"])));
        calls.push(("git:blame", json!(["a", { "path": p }, "r"])));
    }
    for ch in ["git:fileDiff", "git:fileHistory", "git:blame"] {
        for req in [json!(null), json!("a.txt"), json!(5), json!([]), json!({})] {
            calls.push((ch, json!(["a", req, "r"])));
        }
    }
    for (ch, args) in calls {
        let env = envelope(router.route(ch, &args).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{ch} {args}");
        assert!(env["error"].as_str().unwrap().starts_with("invalid "), "{ch} {args}: {env}");
    }
    for p in [PathBuf::from("pwned.txt"), PathBuf::from("pwned2.txt")] {
        assert!(!p.exists(), "{p:?}");
    }
    assert_eq!(fs::read_dir(".").unwrap().count(), cwd_before);
}

/// 경로는 옵션 자리에 올 수 없다: 실제 저장소에 적대적 경로를 보내도 마커 파일이 생기지 않고, pathspec 매직·glob 은 리터럴이다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_경로는_옵션이_아니라_리터럴_경로로만_쓰인다() {
    let tmp = TempDir::new("literal");
    let dir = import(&tmp.0, "files");
    let router = Router::new(true, false, mirror_of(&[("f", dir.as_path())]));
    let (head, mid) = (rev_parse(&dir, "main"), rev_parse(&dir, "main~5"));
    let hunks = |env: &Value| env["data"]["hunks"].as_array().map(Vec::len);
    // a.txt 를 바꾸는 커밋에서 glob·매직 경로는 아무 것도 고르지 못한다(매직이 켜져 있었다면 a.txt 의 diff 가 나온다)
    let paths = [
        "*.txt",
        ":(glob)*.txt",
        ":(top)a.txt",
        ":!a.txt",
        ":/a.txt",
        "a.tx[t]",
        "a.tx?",
        "--output=pwned.txt",
        "-calias.x=!echo>pwned2.txt",
        "--",
        "--end-of-options",
        "-c",
        "../a.txt",
        "a.txt\n--output=pwned3.txt",
    ];
    for path in paths {
        let outside = path.starts_with("../");
        let env = envelope(router.route("git:fileDiff", &json!(["f", { "hash": mid, "path": path }, "r"])).await);
        if outside {
            assert_eq!(env["code"], "GIT", "{path}");
        } else {
            assert_eq!(env["ok"], true, "{path}: {env}");
            assert_eq!(hunks(&env), Some(0), "{path}");
        }
        // 같은 경로가 history·blame 에서도 옵션이 아니다
        let env = envelope(router.route("git:fileHistory", &json!(["f", { "path": path, "rev": head, "skip": 0, "count": 5 }, "r"])).await);
        if outside {
            assert_eq!(env["code"], "GIT", "{path}");
        } else {
            assert_eq!(env["data"]["entries"].as_array().map(Vec::len), Some(0), "{path}: {env}");
        }
        let env = envelope(router.route("git:blame", &json!(["f", { "path": path, "rev": head }, "r"])).await);
        if path == "--output=pwned.txt" {
            // 이 이름의 파일은 실제로 있다 — 파일로 읽는다
            assert_eq!(env["data"]["missing"], false, "{path}");
        } else {
            assert_eq!(env["data"]["missing"], true, "{path}: {env}");
        }
    }
    // oldPath(옛 경로)도 같다: 옵션·매직 모양은 `--` 뒤의 리터럴 경로일 뿐이고, path 와 함께 와도 마커가 생기지 않는다
    for old in ["--output=pwned.txt", "-calias.x=!echo>pwned2.txt", ":(glob)*.txt", "*.txt", "--", "-c"] {
        let env = envelope(router.route("git:fileDiff", &json!(["f", { "hash": mid, "path": "a.txt", "oldPath": old }, "r"])).await);
        assert_eq!(env["ok"], true, "{old}: {env}");
        // 옛 경로가 리터럴이라 a.txt 의 변경(두 hunk)만 나온다
        assert_eq!(hunks(&env), Some(2), "{old}");
    }
    // 옵션처럼 생긴 이름의 실제 파일은 그냥 파일이다
    let env = envelope(router.route("git:fileDiff", &json!(["f", { "hash": rev_parse(&dir, "main~1"), "path": "--output=pwned.txt" }, "r"])).await);
    assert_eq!(hunks(&env), Some(1));
    let env = envelope(router.route("git:blame", &json!(["f", { "path": "--output=pwned.txt", "rev": head }, "r"])).await);
    assert_eq!((env["data"]["missing"].as_bool(), env["data"]["lines"].as_array().map(Vec::len)), (Some(false), Some(1)));
    // glob 문자가 든 실제 파일 이름도 그 파일 하나만 고른다
    let env = envelope(router.route("git:fileDiff", &json!(["f", { "hash": rev_parse(&dir, "main~1"), "path": "[ab].txt" }, "r"])).await);
    assert_eq!(hunks(&env), Some(1));
    for name in ["pwned.txt", "pwned2.txt", "pwned3.txt"] {
        assert!(!dir.join(name).exists() && !Path::new(name).exists(), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 파일_채널은_미러에_없거나_스위치가_꺼지면_사이드카() {
    let tmp = TempDir::new("fileside");
    let dir = import(&tmp.0, "files");
    let router = Router::new(true, false, mirror_of(&[("f", dir.as_path())]));
    let off = Router::new(false, false, mirror_of(&[("f", dir.as_path())]));
    for (ch, args) in [
        ("git:compare", json!(["x", "abcd", "abcd"])),
        ("git:fileDiff", json!(["x", { "hash": "abcd", "path": "a" }])),
        ("git:fileHistory", json!(["x", { "path": "a", "skip": 0, "count": 1 }])),
        ("git:blame", json!(["x", { "path": "a" }])),
        ("git:blame", json!([])),
        ("git:fileDiff", json!([5, {}])),
    ] {
        assert!(matches!(router.route(ch, &args).await, Routed::Sidecar), "{ch} {args}");
    }
    for (ch, args) in [
        ("git:compare", json!(["f", "abcd", "abcd"])),
        ("git:fileDiff", json!(["f", { "hash": "abcd", "path": "a" }])),
        ("git:fileHistory", json!(["f", { "path": "a", "skip": 0, "count": 1 }])),
        ("git:blame", json!(["f", { "path": "a" }])),
    ] {
        assert!(matches!(off.route(ch, &args).await, Routed::Sidecar), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 같은_파일_diff_요청은_합쳐지고_결과는_캐시에서_나온다() {
    let tmp = TempDir::new("diffcache");
    let dir = import(&tmp.0, "files");
    let (hash, from) = (rev_parse(&dir, "main~3"), rev_parse(&dir, "main~6"));
    let router = Arc::new(Router::new(true, false, mirror_of(&[("f", dir.as_path())])));
    let mut tasks = Vec::new();
    for i in 0..6 {
        let r = router.clone();
        let (h, f) = if i % 2 == 0 { (hash.clone(), from.clone()) } else { (hash.to_uppercase(), from.to_uppercase()) };
        tasks.push(tokio::spawn(async move {
            let diff = envelope(r.route("git:fileDiff", &json!(["f", { "hash": h, "path": "big.txt" }, format!("d{i}")])).await);
            let cmp = envelope(r.route("git:compare", &json!(["f", f, h, format!("c{i}")])).await);
            (diff, cmp)
        }));
    }
    let mut results = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    assert_eq!(results[0].0["ok"], true);
    assert_eq!(results[0].0["data"]["truncated"], true);
    assert!(results.iter().all(|r| r.0 == results[0].0));
    // 비교 결과의 from 은 보낸 그대로라 대소문자는 합쳐진(또는 캐시된) 첫 요청의 것이다(TS 도 같다) — 어느 쪽이든 같은 해시다
    for r in &results {
        assert_eq!(r.1["data"]["from"].as_str().unwrap().to_lowercase(), from);
    }
    // 저장소를 지워도 캐시된 결과가 나온다(해시 기준 불변 캐시). full 은 다른 항목이라 git 이 필요하다
    drop(tmp);
    if !dir.exists() {
        let again = envelope(router.route("git:fileDiff", &json!(["f", { "hash": hash, "path": "big.txt" }])).await);
        assert_eq!(again, results[0].0);
        let full = envelope(router.route("git:fileDiff", &json!(["f", { "hash": hash, "path": "big.txt", "full": true }])).await);
        assert_eq!(full["ok"], false);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 등록_전에_온_cancel_은_파일_채널_요청을_git_없이_취소한다() {
    let nowhere = Path::new("/nonexistent-antler-files");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    for (i, (ch, args)) in [
        ("git:compare", json!(["a", "abcd1234", "abcd5678"])),
        ("git:fileDiff", json!(["a", { "hash": "abcd1234", "path": "a.txt" }])),
        ("git:fileHistory", json!(["a", { "path": "a.txt", "skip": 0, "count": 1 }])),
        ("git:blame", json!(["a", { "path": "a.txt", "rev": "abcd1234" }])),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("early{i}");
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        // git 을 띄웠다면 경로가 없어 GIT/ERROR 가 난다 — ABORTED 면 띄우지 않은 것이다
        let env = envelope(router.route(ch, &Value::Array(call)).await);
        assert_eq!(env["code"], "ABORTED", "{ch}: {env}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_파일_채널_요청을_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("filecancel");
    let dir = import(&tmp.0, "files");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("f", dir.as_path())])));
    let head = rev_parse(&dir, "main");
    for (i, (ch, args)) in [
        ("git:compare", json!(["f", rev_parse(&dir, "main~6"), head])),
        ("git:fileDiff", json!(["f", { "hash": rev_parse(&dir, "main~2"), "path": "big.txt", "full": true }])),
        ("git:fileHistory", json!(["f", { "path": "a.txt", "skip": 0, "count": 100 }])),
        ("git:blame", json!(["f", { "path": "big.txt" }])),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("slow{i}");
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        let r = router.clone();
        let task = tokio::spawn(async move { envelope(r.route(ch, &Value::Array(call)).await) });
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
        // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
        match env["ok"].as_bool() {
            Some(true) => {}
            _ => assert_eq!(env["code"], "ABORTED", "{ch}: {env}"),
        }
    }
    // 취소한 뒤에도 같은 요청을 다시 받을 수 있다(합쳐진 요청이 오염되지 않는다)
    let again = envelope(router.route("git:fileDiff", &json!(["f", { "hash": rev_parse(&dir, "main~2"), "path": "big.txt", "full": true }])).await);
    assert_eq!(again["ok"], true);
}

/// 줄 수 세기 한계(20만 줄)를 넘는 diff 는 git 을 끊고(스트림을 버린다) `totalIsLowerBound` 로 끝난다 — 골든 fixture 로는 닿지 않는 경로
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 줄_수_세기_한계를_넘는_diff_는_하한값으로_끝난다() {
    let tmp = TempDir::new("hugediff");
    let dir = tmp.0.join("huge");
    fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| assert!(raw_git(&dir).args(args).status().unwrap().success(), "git {args:?}");
    run(&["init", "-q", "-b", "main"]);
    fs::write(dir.join("huge.txt"), "x\n".repeat(210_000)).unwrap();
    run(&["add", "huge.txt"]);
    run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "huge"]);
    let router = Router::new(true, false, mirror_of(&[("h", dir.as_path())]));
    let head = rev_parse(&dir, "HEAD");
    let env = envelope(router.route("git:fileDiff", &json!(["h", { "hash": head, "path": "huge.txt" }, "r"])).await);
    assert_eq!(env["ok"], true, "{env}");
    let d = &env["data"];
    assert_eq!((d["truncated"].clone(), d["totalIsLowerBound"].clone()), (json!(true), json!(true)), "{}", short(d));
    assert!(d["totalLines"].as_u64().unwrap() >= 200_000);
    assert_eq!(d["hunks"][0]["lines"].as_array().unwrap().len(), 3000);
    // full 도 줄 수 상한(DIFF_FULL_MAX_LINES = 20만)에서 자른다 — TS 와의 알려진 차이(줄당 수백 바이트인 Value 로 수십 GB 가 되는 것을 막는다).
    // 잘린 뒤에도 줄 수는 끝까지 센다(21만 < 세기 한계가 아니라 같은 값이라 하한값일 수 있다)
    let env = envelope(router.route("git:fileDiff", &json!(["h", { "hash": head, "path": "huge.txt", "full": true }, "r"])).await);
    let d = &env["data"];
    assert_eq!(d["truncated"], json!(true), "{}", short(d));
    assert_eq!(d["hunks"][0]["lines"].as_array().unwrap().len(), 200_000);
    assert!(d["totalLines"].as_u64().unwrap() >= 200_000);
}

/// 악성 저장소의 `blame.ignoreRevsFile` 이 사용자 파일을 가리켜도 그 내용이 응답(오류 문구 포함)으로 renderer 에 가지 않는다.
/// git 은 목록 파일의 OID 가 아닌 첫 줄을 `invalid object name: <줄>` 로 stderr 에 낸다 — 목록을 끄고 다시 돌려야 한다(보안 리뷰)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blame_ignore_revs_file_의_내용이_응답으로_새지_않는다() {
    let tmp = TempDir::new("blameleak");
    let dir = tmp.0.join("r");
    fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| assert!(raw_git(&dir).args(args).status().unwrap().success(), "git {args:?}");
    run(&["init", "-q", "-b", "main"]);
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "a"]);
    let secret = tmp.0.join("secret.txt");
    fs::write(&secret, "https://user:ghp_SECRETTOKEN@github.com\n").unwrap();
    run(&["config", "blame.ignoreRevsFile", &secret.to_string_lossy().replace('\\', "/")]);
    let router = Router::new(true, false, mirror_of(&[("b", dir.as_path())]));
    let env = envelope(router.route("git:blame", &json!(["b", { "path": "a.txt" }, "r"])).await);
    assert!(!env.to_string().contains("SECRETTOKEN"), "목록 파일 내용이 응답에 섞였다: {env}");
    // 목록을 끄고 다시 돌려 blame 자체는 성공한다
    assert_eq!(env["ok"], true, "{env}");
}

// ── 코드 탐색기: git:resolveRev · git:tree · git:blob · git:onRemote ────────────────

const CODE_CHANNELS: [(&str, &str); 4] = [("resolveRev", "git:resolveRev"), ("tree", "git:tree"), ("blob", "git:blob"), ("onRemote", "git:onRemote")];

/// 큰 파일 커밋(고아)을 만든다 — 작성자·시각이 고정이라 해시가 TS 와 같다(`tests/goldenChannels.test.ts` 의 `makeBigCommit` 과 같은 명령)
fn make_big_commit(dir: &Path, setup: &Value) {
    use std::io::Write;
    let run = |args: &[&str], input: Option<&[u8]>| -> String {
        let mut c = raw_git(dir);
        c.args(args)
            .env("GIT_AUTHOR_NAME", "홍길동")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0900")
            .env("GIT_COMMITTER_NAME", "홍길동")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_DATE", "1700000000 +0900")
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped());
        let mut child = c.spawn().unwrap();
        if let Some(bytes) = input {
            child.stdin.take().unwrap().write_all(bytes).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let mut entries = String::new();
    for f in setup["files"].as_array().unwrap() {
        let (ch, n) = (f["fill"][0].as_str().unwrap(), f["fill"][1].as_u64().unwrap() as usize);
        let sha = run(&["hash-object", "-w", "--stdin"], Some(ch.repeat(n).as_bytes()));
        entries.push_str(&format!("100644 blob {sha}\t{}\n", f["path"].as_str().unwrap()));
    }
    let tree = run(&["mktree"], Some(entries.as_bytes()));
    let commit = run(&["-c", "commit.gpgsign=false", "commit-tree", &tree, "-m", "big files"], None);
    run(&["update-ref", setup["ref"].as_str().unwrap(), &commit], None);
}

fn utf16_slice(s: &str, from: usize, to: usize) -> String {
    let u: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16_lossy(&u[from..to])
}

/// text·dataUrl 이 200 글자(UTF-16 단위)를 넘으면 {len, head, tail}, tree 항목이 20개를 넘으면 {count, first, last}(`summarizeCode` 와 같다)
fn summarize_code(channel: &str, mut data: Value) -> Value {
    if channel == "blob" {
        for k in ["text", "dataUrl"] {
            if let Some(s) = data[k].as_str().map(str::to_string) {
                let len = s.encode_utf16().count();
                if len > 200 {
                    data[k] = json!({ "len": len, "head": utf16_slice(&s, 0, 40), "tail": utf16_slice(&s, len - 40, len) });
                }
            }
        }
    }
    if channel == "tree" {
        if let Some(entries) = data["entries"].as_array().filter(|e| e.len() > 20).cloned() {
            data["entries"] = json!({ "count": entries.len(), "first": entries[0], "last": entries[entries.len() - 1] });
        }
    }
    data
}

fn code_result(channel: &str, env: &Value, msg: bool) -> Value {
    if env["ok"] != true {
        return if msg { json!({ "ok": false, "code": env["code"], "error": env["error"] }) } else { json!({ "ok": false, "code": env["code"] }) };
    }
    json!({ "ok": true, "data": summarize_code(channel, env["data"].clone()) })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 코드_탐색기_시나리오가_ts_골든과_같다() {
    code_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 코드_탐색기_시나리오가_ssh_에서도_ts_골든과_같다() {
    code_scenarios(&Mode::ssh().await).await;
}

async fn code_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/code.scenarios.json");
    let tmp = TempDir::new("code");
    let dirs: Vec<(String, PathBuf)> = ["code", "branchy", "many", "empty"].iter().map(|n| ((*n).to_string(), import(&tmp.0, n))).collect();
    for s in all["setup"].as_array().unwrap() {
        make_big_commit(&dirs.iter().find(|(n, _)| n == s["repo"].as_str().unwrap()).unwrap().1, s);
    }
    let entries: Vec<(&str, &Path)> = dirs.iter().map(|(n, d)| (n.as_str(), d.as_path())).collect();
    let router = mode.router(&entries);
    let mut diffs = Vec::new();
    let mut total = 0;
    for (channel, route) in CODE_CHANNELS {
        let scenarios = all[channel].as_array().unwrap_or_else(|| panic!("{channel} 시나리오"));
        let golden = read_json(&format!("tests/golden/channels/{channel}.json"));
        let golden = golden["scenarios"].as_array().expect("golden scenarios");
        assert_eq!(scenarios.len(), golden.len(), "{channel}: 시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
        for (i, (s, g)) in scenarios.iter().zip(golden).enumerate() {
            let name = s["name"].as_str().unwrap();
            let repo = s.get("repo").and_then(Value::as_str).unwrap_or("code");
            let dir = &dirs.iter().find(|(n, _)| n == repo).unwrap().1;
            assert_eq!(g["name"], s["name"], "{channel}/{name}: 골든의 시나리오 순서가 다르다");
            let args = expand(&s["args"], &|expr: &str| rev_parse(dir, expr));
            // 보낸 값(해시 포함)이 TS 와 같아야 같은 입력이다 — 같은 .fi·같은 큰 파일 커밋이라 해시가 결정적이다
            assert_eq!(args, g["args"], "{channel}/{name}: 입력이 다르다(fixture 가 바뀌었는가)");
            let mut call = vec![json!(repo)];
            call.extend(args.as_array().unwrap().iter().cloned());
            // TS 는 tree·blob 에만 requestId 를 보낸다
            if channel == "tree" || channel == "blob" {
                call.push(json!(format!("req-{channel}-{i}")));
            }
            let env = envelope(router.route(route, &Value::Array(call)).await);
            let got = code_result(channel, &env, s.get("msg") == Some(&json!(true)));
            total += 1;
            if norm_numbers(&got) != norm_numbers(&g["result"]) {
                diffs.push(format!("{channel}/{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&g["result"])));
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert!(total > 0 && router.rust_handled() > 0 && router.rust_handled() <= total);
    for d in dirs.iter().map(|(_, d)| d) {
        assert!(!d.join("x.txt").exists() && !Path::new("x.txt").exists());
    }
    mode.finish();
}

/// 검증이 git 을 띄우기 전에 거르는 적대적 입력: 저장소 경로가 없어서 git 을 띄웠다면 GIT 오류가 난다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_코드_탐색기_입력은_git_을_띄우지도_파일을_쓰지도_않는다() {
    let nowhere = Path::new("/nonexistent-antler-code");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    let cwd_before = fs::read_dir(".").unwrap().count();
    let ok_rev = "a".repeat(40);
    let mut calls: Vec<(&str, Value, &str)> = Vec::new();
    let evil_refs = [json!("--output=pwned.txt"), json!("-calias.x=!echo>pwned2.txt"), json!("--end-of-options"), json!("main"), json!("refs/heads/*"), json!("a\u{0}b"), json!(""), json!(5), json!(null), json!(["HEAD"])];
    for r in evil_refs {
        calls.push(("git:resolveRev", json!(["a", r]), "invalid ref"));
    }
    calls.push(("git:resolveRev", json!(["a"]), "invalid ref"));
    let evil_revs = [
        json!("-x"),
        json!("--output=pwned.txt"),
        json!(format!("-{}", "a".repeat(39))),
        json!(format!("--{}", "a".repeat(38))),
        json!("HEAD"),
        json!("abcd"),
        json!("a".repeat(39)),
        json!("a".repeat(41)),
        json!("a".repeat(63)),
        json!("a".repeat(65)),
        json!(format!("{ok_rev}\n")),
        json!(format!("{}\u{0}", &ok_rev[..39])),
        json!(5),
        json!(null),
    ];
    for r in evil_revs {
        calls.push(("git:tree", json!(["a", { "rev": r, "dir": "" }, "r"]), "invalid commit hash"));
        calls.push(("git:blob", json!(["a", { "rev": r, "path": "a.txt" }, "r"]), "invalid commit hash"));
        calls.push(("git:onRemote", json!(["a", r]), "invalid commit hash"));
    }
    for d in [json!(".."), json!("../x"), json!("a/../b"), json!("a/./b"), json!("."), json!("a//b"), json!("a\u{0}"), json!(5), json!(null), json!(["a"])] {
        calls.push(("git:tree", json!(["a", { "rev": ok_rev, "dir": d }, "r"]), "invalid path"));
    }
    for p in [json!(""), json!("/"), json!(".."), json!("a/../b"), json!("./a"), json!("a\u{0}"), json!(5), json!(null), json!(["a"])] {
        calls.push(("git:blob", json!(["a", { "rev": ok_rev, "path": p }, "r"]), "invalid path"));
    }
    calls.push(("git:tree", json!(["a", { "rev": ok_rev }, "r"]), "invalid path"));
    calls.push(("git:blob", json!(["a", { "rev": ok_rev }, "r"]), "invalid path"));
    for req in [json!("a"), json!(5), json!([]), json!({}), json!(true)] {
        calls.push(("git:tree", json!(["a", req, "r"]), "invalid commit hash"));
        calls.push(("git:blob", json!(["a", req, "r"]), "invalid commit hash"));
    }
    calls.push(("git:onRemote", json!(["a"]), "invalid commit hash"));
    for (ch, args, msg) in calls {
        let env = envelope(router.route(ch, &args).await);
        assert_eq!((env["ok"].as_bool(), env["code"].as_str(), env["error"].as_str()), (Some(false), Some("ERROR"), Some(msg)), "{ch} {args}");
    }
    // 요청이 null 이거나 없으면 TS 는 TypeError(ERROR) — 문구는 다르지만 git 을 띄우지 않고 ERROR 다
    for ch in ["git:tree", "git:blob"] {
        for args in [json!(["a", null, "r"]), json!(["a"])] {
            let env = envelope(router.route(ch, &args).await);
            assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{ch} {args}");
        }
    }
    for p in [PathBuf::from("pwned.txt"), PathBuf::from("pwned2.txt")] {
        assert!(!p.exists(), "{p:?}");
    }
    assert_eq!(fs::read_dir(".").unwrap().count(), cwd_before);
    assert_eq!(router.rust_handled(), 0, "검증에서 거부된 요청은 git 까지 가지 않는다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 코드_탐색기_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    for ch in ["git:resolveRev", "git:tree", "git:blob", "git:onRemote"] {
        assert!(RUST_CHANNELS.contains(&ch), "{ch}");
    }
    let nowhere = Path::new("/nonexistent-antler-code");
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: nowhere.to_string_lossy().into_owned(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
        ],
    );
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    let rev = "a".repeat(40);
    let calls = |id: &str| {
        vec![
            ("git:resolveRev", json!([id, "HEAD"])),
            ("git:tree", json!([id, { "rev": rev, "dir": "" }])),
            ("git:blob", json!([id, { "rev": rev, "path": "a" }])),
            ("git:onRemote", json!([id, rev])),
        ]
    };
    // 미러에 없는 id·github·(풀이 없는) ssh 는 사이드카가 처리한다
    for id in ["nope", "g", "s"] {
        for (ch, args) in calls(id) {
            assert!(matches!(router.route(ch, &args).await, Routed::Sidecar), "{id} {ch}");
        }
    }
    // 저장소 id 가 문자열이 아니거나 없다
    for ch in ["git:resolveRev", "git:tree", "git:blob", "git:onRemote"] {
        for args in [json!([]), json!([null]), json!([5, "HEAD"])] {
            assert!(matches!(router.route(ch, &args).await, Routed::Sidecar), "{ch} {args}");
        }
    }
    // 스위치(ANTLER_RUST_GIT=0)를 끄면 미러에 있어도 사이드카다
    for (ch, args) in calls("a") {
        assert!(matches!(off.route(ch, &args).await, Routed::Sidecar), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 등록_전에_온_cancel_은_코드_탐색기_요청을_git_없이_취소한다() {
    let nowhere = Path::new("/nonexistent-antler-code");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    let rev = "a".repeat(40);
    for (i, (ch, args)) in [("git:tree", json!(["a", { "rev": rev, "dir": "src" }])), ("git:blob", json!(["a", { "rev": rev, "path": "a.txt" }]))].into_iter().enumerate() {
        let id = format!("early{i}");
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        // git 을 띄웠다면 경로가 없어 GIT/ERROR 가 난다 — ABORTED 면 띄우지 않은 것이다
        let env = envelope(router.route(ch, &Value::Array(call)).await);
        assert_eq!(env["code"], "ABORTED", "{ch}: {env}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_코드_탐색기_요청을_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("codecancel");
    let dir = import(&tmp.0, "code");
    make_big_commit(&dir, &read_json("tests/fixtures/repos/code.scenarios.json")["setup"][0]);
    let router = Arc::new(Router::new(true, false, mirror_of(&[("c", dir.as_path())])));
    let big = rev_parse(&dir, "big");
    let main = rev_parse(&dir, "main");
    for (i, (ch, args)) in [
        ("git:blob", json!(["c", { "rev": big, "path": "limit.png" }])),
        ("git:blob", json!(["c", { "rev": big, "path": "limit.txt" }])),
        ("git:tree", json!(["c", { "rev": main, "dir": "src" }])),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("slow{i}");
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        let r = router.clone();
        let task = tokio::spawn(async move { envelope(r.route(ch, &Value::Array(call)).await) });
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
        // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
        match env["ok"].as_bool() {
            Some(true) => {}
            _ => assert_eq!(env["code"], "ABORTED", "{ch}: {env}"),
        }
    }
    // 취소한 뒤에도 같은 요청을 다시 받을 수 있다
    let again = envelope(router.route("git:blob", &json!(["c", { "rev": big, "path": "limit.png" }])).await);
    assert_eq!((again["ok"].clone(), again["data"]["kind"].clone()), (json!(true), json!("image")));
}

/// 트리·파일 내용은 커밋 해시 기준 불변이라 캐시에서 나온다(저장소를 지워도). 캐시 키는 rev 소문자라
/// 대문자 요청이 캐시에 걸리면 먼저 캐시한 요청의 rev 표기가 돌아온다(TS 도 같다)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 트리와_파일_내용은_캐시에서_나온다() {
    let tmp = TempDir::new("codecache");
    let dir = import(&tmp.0, "code");
    let main = rev_parse(&dir, "main");
    let router = Router::new(true, false, mirror_of(&[("c", dir.as_path())]));
    let tree = envelope(router.route("git:tree", &json!(["c", { "rev": main, "dir": "src" }, "t1"])).await);
    let blob = envelope(router.route("git:blob", &json!(["c", { "rev": main, "path": "README.md" }, "b1"])).await);
    assert_eq!((tree["ok"].clone(), blob["data"]["kind"].clone()), (json!(true), json!("text")));
    // 저장소가 사라져도 캐시된 것은 나온다 — 대문자 rev 도 같은 키다
    drop(tmp);
    if !dir.exists() {
        let upper = main.to_uppercase();
        let tree2 = envelope(router.route("git:tree", &json!(["c", { "rev": upper, "dir": "/src/" }])).await);
        let blob2 = envelope(router.route("git:blob", &json!(["c", { "rev": upper, "path": "README.md" }])).await);
        assert_eq!((&tree2, &blob2), (&tree, &blob));
    }
}

/// missing 은 캐시하지 않는다: 없던 커밋이 나중에 들어오면(fetch) 같은 요청이 이제 내용을 돌려준다.
/// 작은 큰-파일 커밋(결정적 해시)을 다른 저장소에서 먼저 만들어 해시를 알아 둔다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 없던_커밋이_들어오면_missing_이_아니게_된다() {
    let tmp = TempDir::new("codemissing");
    let small = json!({ "ref": "refs/heads/big", "files": [{ "path": "s.txt", "fill": ["z", 10] }] });
    let (donor, dir) = (import(&tmp.0, "code"), import(&tmp.0, "empty"));
    make_big_commit(&donor, &small);
    let rev = rev_parse(&donor, "big");
    let router = Router::new(true, false, mirror_of(&[("c", dir.as_path())]));
    let ask = |ch: &'static str, args: Value| {
        let router = &router;
        async move { envelope(router.route(ch, &args).await) }
    };
    let (t, b) = (json!(["c", { "rev": rev, "dir": "" }, "t"]), json!(["c", { "rev": rev, "path": "s.txt" }, "b"]));
    assert_eq!(ask("git:tree", t.clone()).await["data"]["missing"], true);
    assert_eq!(ask("git:blob", b.clone()).await["data"]["kind"], "missing");
    make_big_commit(&dir, &small);
    assert_eq!(rev_parse(&dir, "big"), rev);
    assert_eq!(ask("git:tree", t).await["data"]["missing"], false);
    assert_eq!(ask("git:blob", b).await["data"]["kind"], "text");
}

/// onRemote 는 true 만 기억한다: false 였던 커밋이 fetch 로 원격 ref 에 들어오면 다음 호출이 true 가 된다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 원격_여부는_true_만_기억한다() {
    let tmp = TempDir::new("onremote");
    let dir = import(&tmp.0, "branchy");
    let head = rev_parse(&dir, "main");
    let router = Router::new(true, false, mirror_of(&[("b", dir.as_path())]));
    let ask = |rev: String| {
        let router = &router;
        async move { envelope(router.route("git:onRemote", &json!(["b", rev])).await) }
    };
    assert_eq!(ask(head.clone()).await["data"], false);
    assert!(raw_git(&dir).args(["update-ref", "refs/remotes/origin/main", &head]).status().unwrap().success());
    assert_eq!(ask(head.clone()).await["data"], true);
    // 원격 ref 가 사라져도 true 는 기억한다(경고용이라 허용)
    assert!(raw_git(&dir).args(["update-ref", "-d", "refs/remotes/origin/main"]).status().unwrap().success());
    assert_eq!(ask(head.to_uppercase()).await["data"], true);
}

/// 악성 저장소: textconv·filter 를 저장소 설정에 심어도 `cat-file blob`·`ls-tree` 는 돌리지 않는다(`codeBrowse.test.ts` 의 같은 이름 시험)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 악성_저장소의_textconv_와_filter_는_코드_탐색기가_실행하지_않는다() {
    let tmp = TempDir::new("codeevil");
    let dir = import(&tmp.0, "code");
    let head = rev_parse(&dir, "main");
    let marker = tmp.0.join("cb-evil-ran");
    let m = marker.to_string_lossy().replace('\\', "/");
    fs::write(dir.join(".git").join("info").join("attributes"), "*.txt diff=evil filter=evil\n*.md diff=evil filter=evil\n").unwrap();
    let cmd = format!("sh -c \"echo x > '{m}'; cat\" --");
    for key in ["diff.evil.textconv", "filter.evil.smudge", "filter.evil.clean", "filter.evil.process"] {
        assert!(raw_git(&dir).args(["config", key, &cmd]).status().unwrap().success());
    }
    let router = Router::new(true, false, mirror_of(&[("c", dir.as_path())]));
    for (path, kind) in [("README.md", "text"), ("later.txt", "text"), ("alpha.txt", "text")] {
        let b = envelope(router.route("git:blob", &json!(["c", { "rev": head, "path": path }, "r"])).await);
        assert_eq!(b["data"]["kind"], kind, "{path}: {b}");
    }
    let t = envelope(router.route("git:tree", &json!(["c", { "rev": head, "dir": "" }, "r"])).await);
    assert_eq!(t["ok"], true);
    let r = envelope(router.route("git:resolveRev", &json!(["c", "HEAD"])).await);
    assert_eq!(r["ok"], true);
    assert!(!marker.exists(), "저장소가 심은 명령이 실행됐다");
}

#[test]
fn 코드_탐색기_argv_는_상수_뒤_검증된_값뿐이다() {
    use antler_lib::git::code_browse::{cat_file_args, ls_tree_args, on_remote_args, resolve_rev_args, CommitId};
    let rev = "ABCDEFabcdef0123456789ABCDEFabcdef012345";
    assert_eq!(resolve_rev_args("refs/heads/main"), ["rev-parse", "--verify", "--end-of-options", "refs/heads/main^{commit}"]);
    assert_eq!(cat_file_args("blob", &format!("{rev}:-x")), ["cat-file", "blob", &format!("{rev}:-x")]);
    assert_eq!(ls_tree_args(rev), ["ls-tree", "-z", "--end-of-options", rev]);
    // onRemote 의 해시는 소문자, 범위는 refs/remotes/
    assert_eq!(on_remote_args(&CommitId::parse(rev).unwrap()), ["for-each-ref", "--count=1", "--contains", &rev.to_lowercase(), "--format=%(refname)", "refs/remotes/"]);
    // 파일 내용은 `cat-file blob` 뿐이다 — `git show`·diff 계열·`ls-tree -l` 인자는 어디에도 없다
    let src = fs::read_to_string(repo_root().join("src-tauri/src/git/code_browse.rs")).unwrap();
    let body = src.split("#[cfg(test)]").next().unwrap();
    for banned in ["\"show\"", "\"diff\"", "\"-l\"", "\"--textconv\"", "\"--filters\""] {
        assert!(!body.contains(banned), "코드 탐색기가 {banned} 를 쓴다");
    }
}

// ── 사이드바 ref 채널: git:refs · git:refsDetails ────────────────

/// 시나리오의 git 단계가 쓰는 고정 작성자·시각(`tests/goldenChannels.test.ts` 의 `REFS_STEP_ENV` 와 같다)
fn refs_git(dir: &Path) -> std::process::Command {
    let mut c = raw_git(dir);
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "홍길동"),
        ("GIT_AUTHOR_EMAIL", "dev@example.com"),
        ("GIT_AUTHOR_DATE", "1700000000 +0900"),
        ("GIT_COMMITTER_NAME", "홍길동"),
        ("GIT_COMMITTER_EMAIL", "dev@example.com"),
        ("GIT_COMMITTER_DATE", "1700000000 +0900"),
    ] {
        c.env(k, v);
    }
    c
}

/// 결과의 문자열에 든 시험 루트 경로(`/`·`\` 두 모양)를 `<ROOT>` 로 바꾼다(`scrubRoot` 와 같다)
fn scrub_root(v: &Value, root: &str) -> Value {
    let fwd = root.replace('\\', "/");
    match v {
        Value::String(s) => json!(s.replace(&fwd, "<ROOT>").replace(root, "<ROOT>")),
        Value::Array(a) => Value::Array(a.iter().map(|x| scrub_root(x, root)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), scrub_root(x, root))).collect()),
        _ => v.clone(),
    }
}

/// 응답의 `data.local` 을 이름순으로 다시 놓는다(정렬 근사의 알려진 차이를 빼고 나머지를 비교하려고)
fn sort_local(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(local) = v["data"]["local"].as_array_mut() {
        local.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    }
    v
}

fn local_names(v: &Value) -> Vec<String> {
    v["data"]["local"].as_array().expect("local").iter().map(|b| b["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 사이드바_ref_채널_시나리오가_ts_골든과_같다() {
    refs_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 사이드바_ref_채널_시나리오가_ssh_에서도_ts_골든과_같다() {
    refs_scenarios(&Mode::ssh().await).await;
}

async fn refs_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/refs.scenarios.json");
    let scenarios = all["scenarios"].as_array().expect("scenarios");
    let golden = [read_json("tests/golden/channels/refs.json"), read_json("tests/golden/channels/refsDetails.json")];
    let tmp = TempDir::new("refs");
    let root = tmp.0.to_string_lossy().into_owned();
    // CI(Windows 러너)의 TEMP 는 8.3 짧은 이름(`RUNNER~1`)인데 git(worktree list 등)은 긴 이름으로 보고한다 — 두 꼴을 모두 지운다
    let root_long = fs::canonicalize(&tmp.0).map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| root.clone());
    let mut cursor = [0usize; 2];
    let mut diffs = Vec::new();
    for (si, sc) in scenarios.iter().enumerate() {
        let name = sc["name"].as_str().unwrap();
        // 시나리오마다 새 디렉터리에 import 한다 — 작업 트리(`../wt`)가 이 아래 형제 디렉터리로 생긴다
        let dir = import_at(tmp.0.join(format!("s{si}")).join("repo"), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = mode.router(&[(id.as_str(), dir.as_path())]);
        // '@rev:<경로>::<식>' 은 그 하위 저장소의 rev-parse 다
        let rev = |expr: &str| match expr.split_once("::") {
            Some((sub, e)) => rev_parse(&dir.join(sub), e),
            None => rev_parse(&dir, expr),
        };
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(m) = step.get("mkdir") {
                fs::create_dir_all(dir.join(m.as_str().unwrap())).unwrap();
                continue;
            }
            if let Some(args) = step.get("git") {
                let args = expand(args, &rev);
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                let cwd = step.get("in").and_then(Value::as_str).map_or_else(|| dir.clone(), |p| dir.join(p));
                assert!(refs_git(&cwd).args(&args).status().unwrap().success(), "{name}: git {args:?}");
                continue;
            }
            let (k, route) = if step.get("refs").is_some() { (0, "git:refs") } else { (1, "git:refsDetails") };
            let env = envelope(router.route(route, &json!([id])).await);
            let got = if env["ok"] == true { json!({ "ok": true, "data": scrub_root(&scrub_root(&env["data"], &root), &root_long) }) } else { json!({ "ok": false, "code": env["code"] }) };
            let g = &golden[k]["scenarios"][cursor[k]];
            cursor[k] += 1;
            assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}/{route}: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
            let (got, want) = match sc.get("knownDiff") {
                // 알려진 차이(정렬 근사): local 의 순서만 선언한 값이어야 하고 TS 와 달라야 한다 — 나머지는 같다
                Some(kd) => {
                    let declared: Vec<String> = kd["rustLocal"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
                    assert_eq!(local_names(&got), declared, "{name}: Rust 의 local 순서");
                    assert_ne!(local_names(&got), local_names(&g["result"]), "{name}: TS 와 같은 순서가 됐다 — 정렬을 ICU 와 같게 옮겼다면 knownDiff 를 지운다");
                    (sort_local(&got), sort_local(&g["result"]))
                }
                None => (got, g["result"].clone()),
            };
            if norm_numbers(&got) != norm_numbers(&want) {
                diffs.push(format!("{name} / {route} 단계 {i}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&want)));
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!([golden[0]["scenarios"].as_array().unwrap().len(), golden[1]["scenarios"].as_array().unwrap().len()], cursor, "골든 항목 수가 시나리오와 다르다");
    mode.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ref_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    for ch in ["git:refs", "git:refsDetails"] {
        assert!(RUST_CHANNELS.contains(&ch), "{ch}");
    }
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: "/nonexistent-antler-refs".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    for ch in ["git:refs", "git:refsDetails"] {
        // 미러에 없는 id·github·(풀이 없는) ssh 는 사이드카가 처리한다
        for id in ["nope", "g", "s"] {
            assert!(matches!(router.route(ch, &json!([id])).await, Routed::Sidecar), "{id} {ch}");
        }
        // 저장소 id 가 문자열이 아니거나 없다
        for args in [json!([]), json!([null]), json!([5])] {
            assert!(matches!(router.route(ch, &args).await, Routed::Sidecar), "{ch} {args}");
        }
        // 스위치(ANTLER_RUST_GIT=0)를 끄면 미러에 있어도 사이드카다
        assert!(matches!(off.route(ch, &json!(["a"])).await, Routed::Sidecar), "{ch}");
        // 미러 검사에 걸린 저장소는 사이드카가 아니라 ERROR 다(다른 채널과 같다)
        let env = envelope(router.route(ch, &json!(["unc"])).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

/// `refs` 는 첫 로그 페이지를 기다리지 않고, `refsDetails` 는 문(`repos:watch` 가 닫는다)이 열릴 때까지 기다린다(TS `RepoService.refsDetails` 의 `afterFirstPage`)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_details_는_첫_페이지_문이_열릴_때까지_기다리고_refs_는_기다리지_않는다() {
    let tmp = TempDir::new("refsgate");
    let dir = import(&tmp.0, "basic");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    // 문이 없는 저장소는 바로 간다
    assert_eq!(envelope(router.route("git:refsDetails", &json!(["b"])).await)["ok"], true);
    assert!(matches!(router.route("repos:watch", &json!(["b"])).await, Routed::Sidecar));
    assert!(router.repo_state().is_armed("b"));
    assert_eq!(envelope(router.route("git:refs", &json!(["b"])).await)["ok"], true, "refs 는 문과 무관하다");
    let r = router.clone();
    let pending = tokio::spawn(async move { envelope(r.route("git:refsDetails", &json!(["b"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!pending.is_finished(), "문이 닫혀 있는 동안 refsDetails 가 끝났다");
    // 첫 로그 페이지가 나갔다는 알림이 문을 연다(로그 서비스·fetch 호스트가 쓰는 같은 길)
    router.note_sink()(NoteOp::FirstPage, "b");
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), pending).await.expect("문이 열리면 끝나야 한다").unwrap();
    assert_eq!(env["ok"], true);
    assert!(env["data"]["tracking"].is_object() && env["data"]["submodules"].is_array());
}

/// 서브모듈 시험용: 하위 저장소 `rel` 을 만들고 커밋 하나를 둔 뒤 그 해시를 돌려준다
fn make_sub(dir: &Path, rel: &str) -> String {
    assert!(refs_git(dir).args(["init", "-q", "-b", "main", rel]).status().unwrap().success());
    assert!(refs_git(&dir.join(rel)).args(["commit", "-q", "--allow-empty", "-m", "sub 1"]).status().unwrap().success());
    rev_parse(&dir.join(rel), "HEAD")
}

/// `update-index --index-info` 에 줄을 먹인다(`--cacheinfo` 는 같은 경로를 한 번에 하나씩이라 많은 gitlink 에 쓸모없다)
fn index_info(dir: &Path, lines: &str) {
    use std::io::Write;
    let mut child = raw_git(dir).args(["update-index", "--add", "--index-info"]).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(lines.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "update-index --index-info");
}

async fn details_of(router: &Router, id: &str) -> Value {
    let env = envelope(router.route("git:refsDetails", &json!([id])).await);
    assert_eq!(env["ok"], true, "{env}");
    env["data"]["submodules"].clone()
}

/// tests/submodule.test.ts 의 '악성 서브모듈' — 하위 저장소의 .git/config·훅·속성에 심은 명령이 상태 조회 중 실행되지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 악성_서브모듈의_설정과_훅은_상태_조회가_실행하지_않는다() {
    let tmp = TempDir::new("subevil");
    let dir = import(&tmp.0, "basic");
    let sha = make_sub(&dir, "libs/ok");
    index_info(&dir, &format!("160000 {sha} 0\tlibs/ok\n"));
    let marker = tmp.0.join("sub-evil-ran");
    let m = marker.to_string_lossy().replace('\\', "/");
    let cmd = format!("sh -c \"echo x > '{m}'; cat\" --");
    let sub = dir.join("libs/ok");
    for key in ["core.fsmonitor", "core.sshCommand", "core.pager", "core.editor", "filter.x.clean", "filter.x.smudge", "filter.x.process", "remote.origin.uploadpack"] {
        assert!(raw_git(&sub).args(["config", key, &cmd]).status().unwrap().success(), "{key}");
    }
    let hooks = sub.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for h in ["post-checkout", "post-commit", "reference-transaction", "pre-command"] {
        fs::write(hooks.join(h), format!("#!/bin/sh\necho x > '{m}'\n")).unwrap();
    }
    fs::write(sub.join(".gitattributes"), "* filter=x\n").unwrap();
    // 대조군: 하드닝 없이 인덱스를 읽는 명령을 돌리면 fsmonitor 페이로드가 실제로 실행된다(시험이 의미 있음을 확인)
    let _ = raw_git(&sub).args(["status", "--porcelain"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    assert!(marker.exists(), "대조군이 실행되지 않았다 — 이 시험이 아무것도 증명하지 못한다");
    fs::remove_file(&marker).unwrap();

    let router = Router::new(true, false, mirror_of(&[("p", dir.as_path())]));
    let subs = details_of(&router, "p").await;
    assert_eq!(subs[0]["path"], "libs/ok");
    assert_eq!(subs[0]["status"], "clean");
    assert_eq!(envelope(router.route("git:refs", &json!(["p"])).await)["ok"], true);
    assert!(!marker.exists(), "하위 저장소가 심은 명령이 실행됐다");
}

/// tests/submodule.test.ts 의 'git 명령은 `..` 경로를 인덱스에 넣지 못하게 하므로 인덱스 파일(v2)을 직접 쓴다'
fn write_gitlink_index(git_dir: &Path, entries: &[(&str, &str)]) {
    use sha1::{Digest, Sha1};
    let mut all = Vec::new();
    all.extend_from_slice(b"DIRC");
    all.extend_from_slice(&2u32.to_be_bytes());
    all.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (path, sha) in entries {
        let mut fixed = [0u8; 62];
        fixed[24..28].copy_from_slice(&0o160000u32.to_be_bytes());
        for (i, b) in fixed[40..60].iter_mut().enumerate() {
            *b = u8::from_str_radix(&sha[i * 2..i * 2 + 2], 16).unwrap();
        }
        fixed[60..62].copy_from_slice(&(path.len().min(0xfff) as u16).to_be_bytes());
        let body_len = 62 + path.len();
        all.extend_from_slice(&fixed);
        all.extend_from_slice(path.as_bytes());
        all.extend(std::iter::repeat_n(0u8, 8 - (body_len % 8)));
    }
    let digest = Sha1::digest(&all);
    all.extend_from_slice(&digest);
    fs::write(git_dir.join("index"), all).unwrap();
}

/// 저장소 밖을 가리키는 gitlink(`../outside` 등)는 git 을 돌리지 않고 unknown 이다(`isSafeSubmodulePath`)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 저장소_밖을_가리키는_서브모듈_경로는_git_을_돌리지_않고_unknown_이다() {
    let tmp = TempDir::new("subpath");
    // evil/inner 에서 `../outside` 가 실제로 가리키는 곳(= evil/outside)에 저장소를 둔다 — 문자열 검사가 없으면 열리는 곳이다
    let outside = tmp.0.join("evil").join("outside");
    fs::create_dir_all(&outside).unwrap();
    assert!(raw_git(&outside).args(["init", "-q", "-b", "main"]).status().unwrap().success());
    assert!(refs_git(&outside).args(["commit", "-q", "--allow-empty", "-m", "outside"]).status().unwrap().success());
    let out_head = rev_parse(&outside, "HEAD");
    let evil = tmp.0.join("evil").join("inner");
    fs::create_dir_all(&evil).unwrap();
    assert!(raw_git(&evil).args(["init", "-q", "-b", "main"]).status().unwrap().success());
    let mut paths = ["../outside", "a/../../outside", "a//b", "./x", "a/./b", "/abs", "C:/abs"];
    paths.sort_unstable();
    write_gitlink_index(&evil.join(".git"), &paths.map(|p| (p, out_head.as_str())));
    // 대조군: 문자열 검사 없이 `../outside` 를 열면 outside 저장소의 HEAD 가 보인다 — 아래에서 head 가 없다는 것이 의미를 가진다
    assert_eq!(rev_parse(&evil.join("../outside"), "HEAD"), out_head);

    let router = Router::new(true, false, mirror_of(&[("e", evil.as_path())]));
    let subs = details_of(&router, "e").await;
    let got: Vec<&str> = subs.as_array().unwrap().iter().map(|s| s["path"].as_str().unwrap()).collect();
    assert_eq!(got, paths, "git ls-files 가 돌려준 경로가 모두 목록에 있다(TS 와 같다)");
    for s in subs.as_array().unwrap() {
        assert_eq!(s["status"], "unknown", "{}", s["path"]);
        assert!(s.get("head").is_none(), "{}", s["path"]);
    }
}

/// 정션이 저장소 밖·`.git` 을 가리키는 서브모듈은 git 을 돌리지 않고 unknown 이다(`assert_inside_root`). Windows 에서만(정션은 `mklink /J`)
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 정션인_서브모듈_경로는_git_을_돌리지_않고_unknown_이다() {
    /// 정션은 대상을 따라가지 않고 링크만 지워야 한다 — `TempDir` 보다 먼저 떨어지도록 뒤에 선언한다
    struct Links(Vec<PathBuf>);
    impl Drop for Links {
        fn drop(&mut self) {
            for l in &self.0 {
                let _ = fs::remove_dir(l);
            }
        }
    }
    #[allow(clippy::disallowed_methods)] // 시험 준비용: std 에 정션 API 가 없다
    fn mklink_j(link: &Path, target: &Path) {
        let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().expect("cmd");
        assert!(out.status.success(), "mklink /J: {}", String::from_utf8_lossy(&out.stdout));
    }
    let tmp = TempDir::new("subjunction");
    let dir = import(&tmp.0, "basic");
    let outside = tmp.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    assert!(raw_git(&outside).args(["init", "-q", "-b", "main"]).status().unwrap().success());
    assert!(refs_git(&outside).args(["commit", "-q", "--allow-empty", "-m", "outside"]).status().unwrap().success());
    let out_head = rev_parse(&outside, "HEAD");
    let links = Links(vec![dir.join("outj"), dir.join("gitj")]);
    mklink_j(&links.0[0], &outside);
    mklink_j(&links.0[1], &dir.join(".git"));
    index_info(&dir, &format!("160000 {out_head} 0\tgitj\n160000 {out_head} 0\toutj\n"));
    // 대조군: 링크 안은 실제로 저장소 밖 저장소의 HEAD 를 보여 준다
    assert_eq!(rev_parse(&links.0[0], "HEAD"), out_head);

    let router = Router::new(true, false, mirror_of(&[("j", dir.as_path())]));
    let subs = details_of(&router, "j").await;
    let got: Vec<&str> = subs.as_array().unwrap().iter().map(|s| s["path"].as_str().unwrap()).collect();
    assert_eq!(got, ["gitj", "outj"]);
    for s in subs.as_array().unwrap() {
        assert_eq!(s["status"], "unknown", "{}", s["path"]);
        assert!(s.get("head").is_none(), "{}", s["path"]);
    }
    drop(links);
}

/// 채널(`git:refs`)은 lite 만 부르지만 TS 의 `refs()` 는 서브모듈을 담는 non-lite 도 있다 — `refs_snapshot` 의 `lite` 인자로 옮겼다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_lite_스냅샷은_서브모듈을_담고_lite_는_비운다() {
    use antler_lib::git::exec::RunOpts;
    use antler_lib::git::refs_service::refs_snapshot;
    let tmp = TempDir::new("sublite");
    let dir = import(&tmp.0, "basic");
    let plain = import(&tmp.0, "empty");
    let sha = make_sub(&dir, "libs/ok");
    index_info(&dir, &format!("160000 {sha} 0\tlibs/ok\n160000 {sha} 0\tlibs/gone\n"));
    let exec = GitExecutor::Local(LocalExecutor::default());
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let full = refs_snapshot(&exec, &path(&dir), &RunOpts::default(), false).await.unwrap();
    assert_eq!(full.submodules.len(), 2);
    assert!(refs_snapshot(&exec, &path(&dir), &RunOpts::default(), true).await.unwrap().submodules.is_empty(), "lite 는 서브모듈을 비워 둔다");
    assert!(refs_snapshot(&exec, &path(&plain), &RunOpts::default(), false).await.unwrap().submodules.is_empty(), "서브모듈이 없으면 빈 배열");
}

/// 서브모듈은 200개까지만 목록에 넣고(`SUBMODULE_MAX`), 앞 50개만 확인한다(`SUBMODULE_PROBE_MAX`) — 나머지는 unknown
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 서브모듈은_200개까지_담고_앞_50개만_확인한다() {
    let tmp = TempDir::new("subcap");
    let dir = import(&tmp.0, "basic");
    let lines: String = (0..205).map(|i| format!("160000 {} 0\tm{i}\n", "a".repeat(40))).collect();
    index_info(&dir, &lines);
    let router = Router::new(true, false, mirror_of(&[("c", dir.as_path())]));
    let subs = details_of(&router, "c").await;
    let subs = subs.as_array().unwrap();
    assert_eq!(subs.len(), 200);
    let count = |status: &str| subs.iter().filter(|s| s["status"] == status).count();
    // 디렉터리가 없는 gitlink 는 확인한 것만 uninitialized 이고(`cannot change to`) 확인하지 않은 것은 unknown 이다
    assert_eq!((count("uninitialized"), count("unknown")), (50, 150));
    assert!(subs[..50].iter().all(|s| s["status"] == "uninitialized") && subs[50..].iter().all(|s| s["status"] == "unknown"));
}

// ── 검색·활동: git:search · git:activity ────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 검색_활동_시나리오가_ts_골든과_같다() {
    search_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 검색_활동_시나리오가_ssh_에서도_ts_골든과_같다() {
    search_scenarios(&Mode::ssh().await).await;
}

async fn search_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/search.scenarios.json");
    let scenarios = all["scenarios"].as_array().expect("scenarios");
    let golden = [read_json("tests/golden/channels/search.json"), read_json("tests/golden/channels/activity.json")];
    let tmp = TempDir::new("search");
    let mut cursor = [0usize; 2];
    let mut diffs = Vec::new();
    for (si, sc) in scenarios.iter().enumerate() {
        let name = sc["name"].as_str().unwrap();
        // 시나리오마다 새 디렉터리에 import 한다(저장소를 바꾸는 단계가 다른 시나리오에 번지지 않게)
        let dir = import_at(tmp.0.join(format!("s{si}")), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = mode.router(&[(id.as_str(), dir.as_path())]);
        let rev = |expr: &str| rev_parse(&dir, expr);
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(args) = step.get("git") {
                let args = expand(args, &rev);
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                assert!(refs_git(&dir).args(&args).status().unwrap().success(), "{name}: git {args:?}");
                continue;
            }
            // 인자 자리: search 는 (질의, 범위, requestId), activity 는 (범위, requestId). 비어 있는 자리는 bridge 가 보내는 표식이다
            let (k, route, slots) = if step.get("search").is_some() { (0, "git:search", 2) } else { (1, "git:activity", 1) };
            let list = expand(&step[if k == 0 { "search" } else { "activity" }], &rev);
            let mut args = vec![json!(id)];
            args.extend(list.as_array().unwrap().iter().cloned());
            while args.len() < 1 + slots {
                args.push(json!({ UNDEFINED_ARG: true }));
            }
            args.push(json!(format!("req-{si}-{i}")));
            let env = envelope(router.route(route, &Value::Array(args)).await);
            let got = if env["ok"] == true {
                json!({ "ok": true, "data": env["data"] })
            } else if step.get("msg") == Some(&json!(true)) {
                json!({ "ok": false, "code": env["code"], "error": env["error"] })
            } else {
                json!({ "ok": false, "code": env["code"] })
            };
            let g = &golden[k]["scenarios"][cursor[k]];
            cursor[k] += 1;
            assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}/{route}: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
            assert_eq!(norm_numbers(&g["args"]), norm_numbers(&list), "{name}/{route} 단계 {i}: 인자가 골든과 다르다");
            if norm_numbers(&got) != norm_numbers(&g["result"]) {
                diffs.push(format!("{name} / {route} 단계 {i}: 골든과 다르다\n  인자: {}\n  Rust: {}\n  TS  : {}", short(&list), short(&got), short(&g["result"])));
            }
        }
        // 옵션처럼 보이는 질의(`--output=x`)가 옵션으로 읽혔다면 git 이 저장소에 파일을 만들었을 것이다
        assert!(!dir.join("x").exists() && !dir.join("pwned.txt").exists(), "{name}: 질의가 옵션으로 해석됐다");
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!([golden[0]["scenarios"].as_array().unwrap().len(), golden[1]["scenarios"].as_array().unwrap().len()], cursor, "골든 항목 수가 시나리오와 다르다");
    mode.finish();
}

/// 검증에 걸린 요청은 git 을 띄우지도(저장소 경로가 없는 곳 — 띄웠다면 다른 오류가 난다) 취소 레지스트리를 쓰지도 `rust_handled` 에 세지도 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_검색_활동_입력은_git_을_띄우지_않는다() {
    let nowhere = Path::new("/nonexistent-antler-search");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    let big_hash = "a".repeat(40);
    for (args, want) in [
        (json!(["a", "x\u{0}y"]), "invalid query"),
        (json!(["a", "\u{0}"]), "invalid query"),
        (json!(["a", "  \u{0}  "]), "invalid query"),
        (json!(["a", 5]), "invalid query"),
        (json!(["a", null]), "invalid query"),
        (json!(["a", { UNDEFINED_ARG: true }]), "invalid query"),
        (json!(["a"]), "invalid query"),
        (json!(["a", "q", null]), "invalid scope"),
        (json!(["a", "q", "x"]), "invalid scope"),
        (json!(["a", "q", { "solo": ["-x"] }]), "invalid solo"),
        (json!(["a", "q", { "solo": ["--output=pwned.txt"] }]), "invalid solo"),
        (json!(["a", "q", { "hide": [big_hash] }]), "invalid hide"),
        (json!(["a", "q", { "includeHead": "no" }]), "invalid scope"),
        // 범위 오류가 질의 오류보다 먼저다
        (json!(["a", 5, null]), "invalid scope"),
    ] {
        let env = envelope(router.route("git:search", &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone(), env["error"].clone()), (json!(false), json!("ERROR"), json!(want)), "{args}");
    }
    for (args, want) in [
        (json!(["a", null]), "invalid scope"),
        (json!(["a", "x"]), "invalid scope"),
        (json!(["a", { "solo": ["--output=pwned.txt"] }]), "invalid solo"),
        (json!(["a", { "hide": [big_hash] }]), "invalid hide"),
        (json!(["a", { "includeHead": 1 }]), "invalid scope"),
    ] {
        let env = envelope(router.route("git:activity", &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone(), env["error"].clone()), (json!(false), json!("ERROR"), json!(want)), "{args}");
    }
    assert_eq!(router.rust_handled(), 0, "거부된 요청은 처리한 것으로 세지 않는다");
    assert!(!Path::new("pwned.txt").exists());
}

/// 검증을 통과한 요청이 git 을 돌려 실패하면 TS 처럼 GIT 이다(저장소 경로가 없다) — 검색의 갈래·활동의 시그니처 계산 모두. 검색이 빈 결과로 삼키지도 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 검증을_통과한_요청이_git_실패로_끝나면_git_오류다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-search"))]));
    for (ch, args) in [("git:search", json!(["a", "abc"])), ("git:search", json!(["a", "로그인"])), ("git:activity", json!(["a"]))] {
        let env = envelope(router.route(ch, &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{ch} {args}: {env}");
    }
    assert_eq!(router.rust_handled(), 3);
}

/// `activity` 는 첫 로그 페이지 문(`repos:watch` 가 닫는다)이 열릴 때까지 기다린다(TS `RepoService.activity` 의 `afterFirstPage`). `search` 는 기다리지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활동은_첫_페이지_문이_열릴_때까지_기다리고_검색은_기다리지_않는다() {
    let tmp = TempDir::new("actgate");
    let dir = import(&tmp.0, "basic");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    // 문이 없는 저장소는 바로 간다
    assert_eq!(envelope(router.route("git:activity", &json!(["b"])).await)["ok"], true);
    assert!(matches!(router.route("repos:watch", &json!(["b"])).await, Routed::Sidecar));
    assert!(router.repo_state().is_armed("b"));
    assert_eq!(envelope(router.route("git:search", &json!(["b", "init"])).await)["ok"], true, "검색은 문과 무관하다");
    let r = router.clone();
    let pending = tokio::spawn(async move { envelope(r.route("git:activity", &json!(["b"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!pending.is_finished(), "문이 닫혀 있는 동안 활동이 끝났다");
    // 첫 로그 페이지가 나갔다는 알림이 문을 연다(로그 서비스·fetch 호스트가 쓰는 같은 길)
    router.note_sink()(NoteOp::FirstPage, "b");
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), pending).await.expect("문이 열리면 끝나야 한다").unwrap();
    assert_eq!((env["ok"].clone(), env["data"]["total"].as_u64().is_some()), (json!(true), true));
}

/// 질의는 셸이 해석하지 않는다 — local 은 셸 없이, ssh 는 인용된 한 인자로 간다. 표식 파일이 생기면 명령이 실행된 것이다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 셸_메타문자_질의는_명령을_실행하지_않는다() {
    for mode in [Mode::Local, Mode::ssh().await] {
        let tmp = TempDir::new("searchshell");
        let dir = import(&tmp.0, "search");
        let marker = tmp.0.join("search-evil-ran");
        let m = marker.to_string_lossy().replace('\\', "/");
        let router = mode.router(&[("s", dir.as_path())]);
        for q in [
            format!("$(touch '{m}')"),
            format!("`touch '{m}'`"),
            format!(";touch '{m}';"),
            format!("' ; touch '{m}' ; '"),
            format!("\" ; touch '{m}' ; \""),
            format!("a\ntouch '{m}'"),
            format!("| touch '{m}'"),
            format!("&& touch '{m}'"),
            format!("> '{m}'"),
            format!("--output={m}"),
            format!("-c core.pager=touch '{m}'"),
        ] {
            let env = envelope(router.route("git:search", &json!(["s", q, null])).await);
            // 범위가 null 이라 거부된다 — 같은 질의를 범위 없이도 보낸다
            assert_eq!(env["code"], "ERROR", "{q}");
            let env = envelope(router.route("git:search", &json!(["s", q])).await);
            assert_eq!(env["ok"], true, "{q}: {env}");
            assert!(!marker.exists(), "{q}: 질의가 실행됐다");
        }
        mode.finish();
    }
}

/// `n` 개의 커밋을 `git fast-import` 로 만든다(작성자·메시지에 `bulk` 가 들어 있다 — 메시지·작성자 두 갈래가 모두 찬다)
fn make_bulk(dir: &Path, n: usize) {
    use std::io::Write;
    assert!(raw_git(dir).args(["init", "-q", "-b", "main"]).status().unwrap().success());
    let mut stream = String::new();
    for i in 1..=n {
        let msg = format!("bulk {i}\n");
        stream.push_str(&format!("commit refs/heads/main\nmark :{i}\nauthor bulk <b@example.com> 1700000000 +0000\ncommitter bulk <b@example.com> 1700000000 +0000\ndata {}\n{msg}", msg.len()));
        if i > 1 {
            stream.push_str(&format!("from :{}\n", i - 1));
        }
        stream.push('\n');
    }
    let mut child = raw_git(dir).args(["fast-import", "--quiet"]).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "fast-import");
}

/// 결과는 1만 개에서 자르고 `truncated` 를 알린다(TS `SEARCH_LIMIT`). 두 갈래가 모두 1만 1개씩 차서 병합 스트림을 끝까지 훑는 경로다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 검색_결과는_1만개에서_자르고_truncated_를_알린다() {
    let tmp = TempDir::new("searchbulk");
    let dir = tmp.0.join("bulk");
    fs::create_dir_all(&dir).unwrap();
    make_bulk(&dir, 10_010);
    let router = Router::new(true, false, mirror_of(&[("b", dir.as_path())]));
    let env = envelope(router.route("git:search", &json!(["b", "bulk"])).await);
    assert_eq!(env["ok"], true, "{}", short(&env));
    let hashes = env["data"]["hashes"].as_array().unwrap();
    assert_eq!((hashes.len(), &env["data"]["truncated"]), (10_000, &json!(true)));
    // 그래프 순서(자식 먼저)의 앞 1만 개다
    assert_eq!(hashes[0], json!(rev_parse(&dir, "main")));
    assert_eq!(hashes[9_999], json!(rev_parse(&dir, "main~9999")));
    // 1만 개 이하면 자르지 않는다
    let env = envelope(router.route("git:search", &json!(["b", "bulk 10009"])).await);
    assert_eq!((env["data"]["hashes"].as_array().unwrap().len(), env["data"]["truncated"].clone()), (1, json!(false)));
    // 활동은 상한 없이 모든 커밋을 센다
    let env = envelope(router.route("git:activity", &json!(["b"])).await);
    assert_eq!(env["data"]["total"], 10_010);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 검색_활동_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    for ch in ["git:search", "git:activity"] {
        assert!(RUST_CHANNELS.contains(&ch), "{ch}");
    }
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: "/nonexistent-antler-search".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    for (ch, tail) in [("git:search", json!(["q"])), ("git:activity", json!([]))] {
        let call = |id: &Value| {
            let mut v = vec![id.clone()];
            v.extend(tail.as_array().unwrap().iter().cloned());
            Value::Array(v)
        };
        // 미러에 없는 id·github·(풀이 없는) ssh 는 사이드카가 처리한다
        for id in ["nope", "g", "s"] {
            assert!(matches!(router.route(ch, &call(&json!(id))).await, Routed::Sidecar), "{id} {ch}");
        }
        // 저장소 id 가 문자열이 아니거나 없다
        for id in [json!(null), json!(5)] {
            assert!(matches!(router.route(ch, &call(&id)).await, Routed::Sidecar), "{ch} {id}");
        }
        assert!(matches!(router.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
        // 스위치(ANTLER_RUST_GIT=0)를 끄면 미러에 있어도 사이드카다
        assert!(matches!(off.route(ch, &call(&json!("a"))).await, Routed::Sidecar), "{ch}");
        // 미러 검사에 걸린 저장소는 사이드카가 아니라 ERROR 다(다른 채널과 같다)
        let env = envelope(router.route(ch, &call(&json!("unc"))).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 등록_전에_온_cancel_은_검색과_활동을_git_없이_취소한다() {
    let nowhere = Path::new("/nonexistent-antler-search");
    let router = Router::new(true, false, mirror_of(&[("a", nowhere)]));
    for (i, (ch, args)) in [("git:search", json!(["a", "q", { "solo": ["refs/heads/main"] }])), ("git:activity", json!(["a", { "solo": ["refs/heads/main"] }]))].into_iter().enumerate() {
        let id = format!("early{i}");
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        // git 을 띄웠다면 경로가 없어 GIT/ERROR 가 난다 — ABORTED 면 띄우지 않은 것이다
        let env = envelope(router.route(ch, &Value::Array(call)).await);
        assert_eq!(env["code"], "ABORTED", "{ch}: {env}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_검색_활동을_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("searchcancel");
    let dir = tmp.0.join("bulk");
    fs::create_dir_all(&dir).unwrap();
    make_bulk(&dir, 3_000);
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    for (i, (ch, args)) in [("git:search", json!(["b", "bulk"])), ("git:search", json!(["b", "bulk 1"])), ("git:activity", json!(["b"]))].into_iter().enumerate() {
        let id = format!("slow{i}");
        // search 는 (질의, 범위, requestId), activity 는 (범위, requestId) — 범위 자리는 표식으로 비운다
        let mut call = args.as_array().unwrap().clone();
        call.push(json!({ UNDEFINED_ARG: true }));
        call.push(json!(id));
        let r = router.clone();
        let task = tokio::spawn(async move { envelope(r.route(ch, &Value::Array(call)).await) });
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
        // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
        match env["ok"].as_bool() {
            Some(true) => {}
            _ => assert_eq!(env["code"], "ABORTED", "{ch}: {env}"),
        }
    }
    // 취소한 뒤에도 같은 요청을 다시 받을 수 있다(슬롯·합치기 표가 풀렸다)
    // `bulk 7`·`bulk 70`~`bulk 79`·`bulk 700`~`bulk 799`
    assert_eq!(envelope(router.route("git:search", &json!(["b", "bulk 7"])).await)["data"]["hashes"].as_array().unwrap().len(), 1 + 10 + 100);
    assert_eq!(envelope(router.route("git:activity", &json!(["b"])).await)["data"]["total"], 3_000);
}

async fn activity_total(router: &Router, id: &str) -> u64 {
    let env = envelope(router.route("git:activity", &json!([id])).await);
    assert_eq!(env["ok"], true, "{env}");
    env["data"]["total"].as_u64().unwrap()
}

/// 기본 보기(모든 브랜치·리모트·태그 + HEAD)에서 닿는 커밋 수 — 활동의 `total` 과 같아야 한다
fn reachable_count(dir: &Path) -> u64 {
    let out = raw_git(dir).args(["rev-list", "--count", "--ignore-missing", "--branches", "--remotes", "--tags", "HEAD"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().parse().unwrap()
}

fn commit_empty(dir: &Path, msg: &str) {
    assert!(refs_git(dir).args(["commit", "-q", "--allow-empty", "-m", msg]).status().unwrap().success());
}

// 아래 네 시험은 TS `repoService.test.ts` 의 첫 페이지 문·시그니처 `it` 4개를 activity 수준으로 옮긴 것이다(test-map.json).
// Phase 6a PR 4 의 Rust `activity` 는 감시가 기억한 시그니처(`known_signature`)를 **쓰지 않고** 매번 새로 계산하므로(감시는 PR 7b 에서 온다)
// 알림(`logStale`·`fetchChanged`)이 없어도 같은 결과가 나온다 — 알림 시험은 알림 길이 활동 결과를 깨지 않음을 보증하고,
// 재사용 시험은 TS 와 반대로 "재사용하지 않음"을 단정한다. 7b 에서 재사용을 켜면 알림 시험이 의미를 갖는다(알림 없이는 낡은 값이 나오므로)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활동은_logstale_알림_뒤에도_바뀐_저장소를_새로_센다() {
    let tmp = TempDir::new("actstale");
    let dir = import(&tmp.0, "search");
    let router = Router::new(true, false, mirror_of(&[("a", dir.as_path())]));
    let before = activity_total(&router, "a").await;
    assert_eq!(before, reachable_count(&dir));
    commit_empty(&dir, "note-stale");
    // Rust 가 `StaleLogError` 에 해당하는 일을 알려 오면 기억한 시그니처를 버린다(로그 서비스가 같은 길로 알린다)
    router.note_sink()(NoteOp::Stale, "a");
    assert_eq!(router.repo_state().known_signature("a"), None, "알림이 기억을 버렸다");
    assert_eq!(activity_total(&router, "a").await, before + 1);
    // 모르는 저장소 id 는 아무 일도 하지 않는다
    router.note_sink()(NoteOp::Stale, "nope");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활동은_fetch_변경_알림_뒤에도_바뀐_저장소를_새로_센다() {
    let tmp = TempDir::new("actfetch");
    let dir = import(&tmp.0, "search");
    let router = Router::new(true, false, mirror_of(&[("a", dir.as_path())]));
    let before = activity_total(&router, "a").await;
    commit_empty(&dir, "note-fetch-changed");
    router.note_sink()(NoteOp::FetchChanged, "a");
    assert_eq!(router.repo_state().known_signature("a"), None);
    assert_eq!(activity_total(&router, "a").await, before + 1);
    router.note_sink()(NoteOp::FetchChanged, "nope");
}

/// TS 의 "감시가 계산한 ref 시그니처를 활동 조회가 다시 계산하지 않고 쓴다"의 PR 4 판: **재사용하지 않는다**.
/// 첫 활동이 시그니처를 기억해도(공유 계산이 기록한다) 알림 없이 저장소가 바뀐 뒤의 활동은 낡은 기억 대신 새로 계산해 바뀐 값을 낸다 — 기억값을 읽는 구현이면 캐시 적중으로 낡은 값이 나온다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활동은_기억한_시그니처를_재사용하지_않고_알림_없이도_매번_새로_계산한다() {
    let tmp = TempDir::new("actfresh");
    let dir = import(&tmp.0, "search");
    let router = Router::new(true, false, mirror_of(&[("a", dir.as_path())]));
    let first = activity_total(&router, "a").await;
    let remembered = router.repo_state().known_signature("a");
    assert!(remembered.is_some(), "공유 계산은 시그니처를 기억한다(소비하지 않을 뿐이다)");
    // 알림 없이 저장소가 바뀐다 — 감시가 없는 상태다
    commit_empty(&dir, "silent");
    assert_eq!(activity_total(&router, "a").await, first + 1, "기억한 낡은 시그니처로 캐시에 적중했다");
    assert_ne!(router.repo_state().known_signature("a"), remembered, "새 계산이 기억을 갱신했다");
    // 바뀐 것이 없으면 같은 값(캐시 적중)이다
    assert_eq!(activity_total(&router, "a").await, first + 1);
    // ref 를 지워도 같다
    assert!(refs_git(&dir).args(["update-ref", "-d", "refs/heads/side"]).status().unwrap().success());
    assert_eq!(activity_total(&router, "a").await, reachable_count(&dir));
}

/// TS 의 "StaleLogError 로 이력이 바뀐 것을 알면 … 새로 계산해 활동을 돌려준다": 로그 이어 읽기가 이력 변경(STALE)을 알린 뒤에도, 알린 적이 없을 때도 활동은 바뀐 이력을 센다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 로그가_stale_을_알린_뒤의_활동은_바뀐_이력을_센다() {
    let tmp = TempDir::new("actlog");
    let dir = import(&tmp.0, "branchy");
    let router = Router::new(true, false, mirror_of(&[("a", dir.as_path())]));
    let before = activity_total(&router, "a").await;
    assert_eq!(before, reachable_count(&dir));
    let first = envelope(router.route("git:log", &json!(["a", { "skip": 0, "count": 5 }])).await);
    assert_eq!(first["data"]["hasMore"], true);
    // 커서를 다른 범위로 물러나게 한 뒤 앞쪽 이력을 바꾼다(log.scenarios.json 의 STALE 시나리오와 같다)
    envelope(router.route("git:log", &json!(["a", { "skip": 0, "count": 1, "scope": { "solo": ["refs/tags/v1.0"] } }])).await);
    assert!(refs_git(&dir).args(["update-ref", "-d", "refs/heads/topic"]).status().unwrap().success());
    let stale = envelope(router.route("git:log", &json!(["a", { "skip": 5, "count": 5 }])).await);
    assert_eq!((stale["ok"].clone(), stale["code"].clone()), (json!(false), json!("STALE")), "{stale}");
    let after = activity_total(&router, "a").await;
    assert_eq!(after, reachable_count(&dir));
    assert!(after < before, "topic 에만 있던 커밋이 빠졌다: {before} → {after}");
}

/// 같은 키의 활동 요청은 하나로 합쳐지고, 한 호출자의 취소가 다른 호출자의 결과를 죽이지 않는다. 키가 서로 다른 요청이 소스의 큐 한도(local 6·ssh 4)보다 많아도
/// 서로 막지 않고 끝난다 — 합치기는 슬롯을 쥐지 않고, 슬롯 안에서는 시그니처가 다시 슬롯을 기다리지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 활동_요청은_합쳐지고_큐_한도보다_많아도_서로_막지_않는다() {
    for mode in [Mode::Local, Mode::ssh().await] {
        let tmp = TempDir::new("actjoin");
        let dir = import(&tmp.0, "search");
        let router = Arc::new(mode.router(&[("a", dir.as_path())]));
        let want = reachable_count(&dir);
        // 같은 키 셋 중 첫 요청만 취소한다
        let mut tasks = Vec::new();
        for i in 0..3 {
            let r = router.clone();
            tasks.push(tokio::spawn(async move { envelope(r.route("git:activity", &json!(["a", { "hide": ["refs/heads/side"] }, format!("same{i}")])).await) }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(matches!(router.route("req:cancel", &json!(["same0"])).await, Routed::Sidecar));
        for (i, t) in tasks.into_iter().enumerate() {
            let env = tokio::time::timeout(std::time::Duration::from_secs(60), t).await.expect("끝나야 한다").unwrap();
            match (i, env["ok"].as_bool()) {
                (0, Some(false)) => assert_eq!(env["code"], "ABORTED"),
                _ => assert_eq!(env["data"]["total"], want - 1, "{i}: {env}"),
            }
        }
        // 키가 모두 다른 요청 16개(존재하지 않는 solo ref)
        let mut tasks = Vec::new();
        for i in 0..16 {
            let r = router.clone();
            tasks.push(tokio::spawn(async move { envelope(r.route("git:activity", &json!(["a", { "solo": [format!("refs/heads/none{i}")] }])).await) }));
        }
        for t in tasks {
            let env = tokio::time::timeout(std::time::Duration::from_secs(120), t).await.expect("큐 한도보다 많아도 끝나야 한다").unwrap();
            assert_eq!((env["ok"].clone(), env["data"]["total"].clone()), (json!(true), json!(0)), "{env}");
        }
        mode.finish();
    }
}

// ── LFS·gitflow: git:repoExtras · git:lfsPaths ────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lfs_gitflow_시나리오가_ts_골든과_같다() {
    extras_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lfs_gitflow_시나리오가_ssh_에서도_ts_골든과_같다() {
    extras_scenarios(&Mode::ssh().await).await;
}

/// 골든에 남기는 인자: 요청의 paths 가 20개를 넘으면 `{count, first, last}`(`tests/goldenChannels.test.ts` 의 `summarizeLfsArgs` 와 같다)
fn summarize_lfs_args(list: &Value) -> Value {
    let mut list = list.as_array().expect("args").clone();
    if let Some(paths) = list.first().and_then(|r| r["paths"].as_array()).filter(|p| p.len() > 20).cloned() {
        list[0]["paths"] = json!({ "count": paths.len(), "first": paths[0], "last": paths[paths.len() - 1] });
    }
    Value::Array(list)
}

async fn extras_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/extras.scenarios.json");
    let scenarios = all["scenarios"].as_array().expect("scenarios");
    let golden = [read_json("tests/golden/channels/repoExtras.json"), read_json("tests/golden/channels/lfsPaths.json")];
    let tmp = TempDir::new("extras");
    let mut cursor = [0usize; 2];
    let mut diffs = Vec::new();
    for (si, sc) in scenarios.iter().enumerate() {
        let name = sc["name"].as_str().unwrap();
        // 시나리오마다 새 디렉터리에 import 한다(저장소·작업 트리를 바꾸는 단계가 다른 시나리오에 번지지 않게)
        let dir = import_at(tmp.0.join(format!("s{si}")), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = mode.router(&[(id.as_str(), dir.as_path())]);
        let rev = |expr: &str| rev_parse(&dir, expr);
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(args) = step.get("git") {
                let args = expand(args, &rev);
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                assert!(refs_git(&dir).args(&args).status().unwrap().success(), "{name}: git {args:?}");
                continue;
            }
            if let Some(w) = step.get("write") {
                let w = expand(w, &rev);
                let target = dir.join(w[0].as_str().unwrap());
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, w[1].as_str().unwrap()).unwrap();
                continue;
            }
            if step.get("extras").is_some() {
                let env = envelope(router.route("git:repoExtras", &json!([id])).await);
                let got = if env["ok"] == true { json!({ "ok": true, "data": env["data"] }) } else { json!({ "ok": false, "code": env["code"] }) };
                let g = &golden[0]["scenarios"][cursor[0]];
                cursor[0] += 1;
                assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}/repoExtras: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
                if norm_numbers(&got) != norm_numbers(&g["result"]) {
                    diffs.push(format!("{name} / repoExtras 단계 {i}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&g["result"])));
                }
                continue;
            }
            // 인자 자리: (요청, requestId). 비어 있는 자리는 bridge 가 보내는 표식이다
            let list = expand(&step["lfsPaths"], &rev);
            let mut args = vec![json!(id)];
            args.extend(list.as_array().unwrap().iter().cloned());
            while args.len() < 2 {
                args.push(json!({ UNDEFINED_ARG: true }));
            }
            args.push(json!(format!("req-{si}-{i}")));
            let env = envelope(router.route("git:lfsPaths", &Value::Array(args)).await);
            let got = if env["ok"] == true {
                let data = env["data"].as_array().expect("paths").clone();
                let data = if data.len() > 20 { json!({ "count": data.len(), "first": data[0], "last": data[data.len() - 1] }) } else { Value::Array(data) };
                json!({ "ok": true, "data": data })
            } else if step.get("msg") == Some(&json!(true)) {
                json!({ "ok": false, "code": env["code"], "error": env["error"] })
            } else {
                json!({ "ok": false, "code": env["code"] })
            };
            let g = &golden[1]["scenarios"][cursor[1]];
            cursor[1] += 1;
            assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}/lfsPaths: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
            assert_eq!(norm_numbers(&g["args"]), norm_numbers(&summarize_lfs_args(&list)), "{name}/lfsPaths 단계 {i}: 인자가 골든과 다르다");
            if norm_numbers(&got) != norm_numbers(&g["result"]) {
                diffs.push(format!("{name} / lfsPaths 단계 {i}: 골든과 다르다\n  인자: {}\n  Rust: {}\n  TS  : {}", short(&summarize_lfs_args(&list)), short(&got), short(&g["result"])));
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!([golden[0]["scenarios"].as_array().unwrap().len(), golden[1]["scenarios"].as_array().unwrap().len()], cursor, "골든 항목 수가 시나리오와 다르다");
    mode.finish();
}

/// 검증에 걸린 요청은 git 을 띄우지도(저장소 경로가 없는 곳 — 띄웠다면 GIT 오류가 난다) 취소 레지스트리를 쓰지도 `rust_handled` 에 세지도 않는다.
/// 요청의 앞 1000개만 검증한다(TS 는 `slice` 한 뒤 검사한다) — 1001번째 항목은 보지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_lfs_요청은_git_을_띄우지_않는다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-lfs"))]));
    for (args, want) in [
        (json!(["a"]), "invalid request"),
        (json!(["a", null]), "invalid request"),
        (json!(["a", { UNDEFINED_ARG: true }]), "invalid request"),
        (json!(["a", "x"]), "invalid paths"),
        (json!(["a", { "paths": "a" }]), "invalid paths"),
        (json!(["a", { "paths": [""] }]), "invalid path"),
        (json!(["a", { "paths": ["a\u{0}b"] }]), "invalid path"),
        (json!(["a", { "paths": [5] }]), "invalid path"),
        (json!(["a", { "rev": "--help", "paths": ["a"] }]), "invalid commit hash"),
        (json!(["a", { "rev": "--output=pwned.txt", "paths": ["a"] }]), "invalid commit hash"),
        (json!(["a", { "rev": null, "paths": ["a"] }]), "invalid commit hash"),
        (json!(["a", { "rev": "abc", "paths": 5 }]), "invalid commit hash"),
    ] {
        let env = envelope(router.route("git:lfsPaths", &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone(), env["error"].clone()), (json!(false), json!("ERROR"), json!(want)), "{args}");
    }
    assert_eq!(router.rust_handled(), 0, "거부된 요청은 처리한 것으로 세지 않는다");
    assert!(!Path::new("pwned.txt").exists());
    // 1001번째 항목은 검증하지 않는다 → 검증은 통과하고 git 이 실패한다(경로가 없다)
    let mut paths: Vec<Value> = (0..1000).map(|i| json!(format!("f{i}"))).collect();
    paths.push(json!(""));
    let env = envelope(router.route("git:lfsPaths", &json!(["a", { "paths": paths }])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    assert_eq!(router.rust_handled(), 1);
}

/// `repoExtras` 는 조회가 모두 실패해도(저장소 경로가 없다) 오류가 아니라 `{lfs:false, gitflow:null}` 이다(TS 의 `.catch(() => false)`). `lfsPaths` 의 git 실패는 GIT 이다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repo_extras_는_git_실패를_삼키고_lfs_paths_는_git_오류를_낸다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-lfs"))]));
    let env = envelope(router.route("git:repoExtras", &json!(["a"])).await);
    assert_eq!((env["ok"].clone(), env["data"].clone()), (json!(true), json!({ "lfs": false, "gitflow": null })));
    let env = envelope(router.route("git:lfsPaths", &json!(["a", { "paths": ["a.psd"] }])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    assert_eq!(router.rust_handled(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lfs_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: "/nonexistent-antler-lfs".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    for (ch, tail) in [("git:repoExtras", json!([])), ("git:lfsPaths", json!([{ "paths": ["a"] }]))] {
        let call = |id: &Value| {
            let mut v = vec![id.clone()];
            v.extend(tail.as_array().unwrap().iter().cloned());
            Value::Array(v)
        };
        for id in ["nope", "g", "s"] {
            assert!(matches!(router.route(ch, &call(&json!(id))).await, Routed::Sidecar), "{id} {ch}");
        }
        for id in [json!(null), json!(5)] {
            assert!(matches!(router.route(ch, &call(&id)).await, Routed::Sidecar), "{ch} {id}");
        }
        assert!(matches!(router.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
        assert!(matches!(off.route(ch, &call(&json!("a"))).await, Routed::Sidecar), "{ch}");
        let env = envelope(router.route(ch, &call(&json!("unc"))).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 등록_전에_온_cancel_은_lfs_paths_를_git_없이_취소한다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-lfs"))]));
    assert!(matches!(router.route("req:cancel", &json!(["early"])).await, Routed::Sidecar));
    // git 을 띄웠다면 경로가 없어 GIT 이 난다 — ABORTED 면 띄우지 않은 것이다
    let env = envelope(router.route("git:lfsPaths", &json!(["a", { "paths": ["a.psd"] }, "early"])).await);
    assert_eq!(env["code"], "ABORTED", "{env}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_lfs_paths_를_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("lfscancel");
    let dir = import(&tmp.0, "basic");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    // 묶음이 여러 개(1000개 × 긴 경로)라 취소가 묶음 사이에도 닿아야 한다
    let paths: Vec<Value> = (0..1000).map(|i| json!(format!("dir/{}{i}", "y".repeat(60)))).collect();
    let r = router.clone();
    let task = tokio::spawn(async move { envelope(r.route("git:lfsPaths", &json!(["b", { "paths": paths }, "slow"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    assert!(matches!(router.route("req:cancel", &json!(["slow"])).await, Routed::Sidecar));
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
    // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
    match env["ok"].as_bool() {
        Some(true) => {}
        _ => assert_eq!(env["code"], "ABORTED", "{env}"),
    }
    // 취소한 뒤에도 같은 요청을 다시 받을 수 있다(슬롯이 풀렸다)
    let again = envelope(router.route("git:lfsPaths", &json!(["b", { "paths": ["a.psd"] }])).await);
    assert_eq!((again["ok"].clone(), again["data"].clone()), (json!(true), json!([])));
}

/// 악성 저장소: `check-attr`·`ls-tree`·`cat-file blob` 길(lfsPaths 와 repoExtras 의 커밋된 속성 읽기)은 저장소 설정의 filter·textconv 를 실행하지 않는다.
/// 작업 트리·info/attributes 를 읽는 `diff --no-index` 는 이 시험이 아니라 `no_index_*` 시험들이 덮는다(규칙이 `.gitattributes` 자체에 맞아야 실행되므로 `*` 규칙을 쓴다)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 악성_저장소의_필터는_lfs_paths_가_실행하지_않는다() {
    let tmp = TempDir::new("lfsevil");
    let dir = import(&tmp.0, "basic");
    let marker = tmp.0.join("lfs-evil-ran");
    let m = marker.to_string_lossy().replace('\\', "/");
    assert!(raw_git(&dir).args(["checkout", "-q", "-f", "main"]).status().unwrap().success());
    fs::write(dir.join(".gitattributes"), "* filter=evil diff=evil\n*.psd filter=lfs\n").unwrap();
    fs::write(dir.join("a.psd"), "x").unwrap();
    assert!(raw_git(&dir).args(["add", "-A"]).status().unwrap().success());
    assert!(refs_git(&dir).args(["commit", "-q", "-m", "evil"]).status().unwrap().success());
    let cmd = format!("sh -c \"echo x > '{m}'; cat\" --");
    for key in ["diff.evil.textconv", "filter.evil.smudge", "filter.evil.clean", "filter.evil.process"] {
        assert!(raw_git(&dir).args(["config", key, &cmd]).status().unwrap().success());
    }
    let head = rev_parse(&dir, "HEAD");
    let router = Router::new(true, false, mirror_of(&[("e", dir.as_path())]));
    for req in [json!({ "rev": head, "paths": ["a.psd", ".gitattributes"] }), json!({ "paths": ["a.psd", ".gitattributes"] })] {
        let env = envelope(router.route("git:lfsPaths", &json!(["e", req])).await);
        assert_eq!(env["data"], json!(["a.psd"]), "{env}");
    }
    assert!(!marker.exists(), "저장소가 심은 명령이 실행됐다");
}

// ── `git diff --no-index` 의 filter 실행 차단(`tests/noIndexFilter.test.ts` 의 repoExtras 쪽 대응) ──

/// 표지 filter 를 심는 저장소: `driver` 드라이버의 clean 이 표지 파일을 만드는 명령(임시 디렉터리 안에만 쓴다). 표지 파일 경로를 돌려준다
fn plant_filter(tmp: &Path, dir: &Path, driver: &str, tag: &str) -> PathBuf {
    let marker = tmp.join(format!("MARK-{tag}"));
    let m = marker.to_string_lossy().replace('\\', "/");
    let cmd = format!("sh -c \"echo x > '{m}'; cat\" --");
    assert!(raw_git(dir).args(["config", &format!("filter.{driver}.clean"), &cmd]).status().unwrap().success());
    marker
}

async fn extras_lfs(router: &Router, id: &str) -> Value {
    let env = envelope(router.route("git:repoExtras", &json!([id])).await);
    assert_eq!(env["ok"], json!(true), "{env}");
    env["data"]["lfs"].clone()
}

async fn no_index_scenarios(mode: &Mode) {
    let tmp = TempDir::new("noindex");
    // 대조군: 가드 없이 --no-index 를 돌리면 심어 둔 명령이 실행된다(시험이 의미 있음을 확인)
    let dir = import_at(tmp.0.join("control"), "basic");
    let marker = plant_filter(&tmp.0, &dir, "evil", "control");
    fs::write(dir.join(".gitattributes"), ".gitattributes filter=evil\n").unwrap();
    let st = raw_git(&dir).args(["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--", "/dev/null", ".gitattributes"]).output().unwrap();
    assert_eq!(st.status.code(), Some(1));
    assert!(marker.exists(), "대조군: 가드 없는 --no-index 가 표지 명령을 실행하지 않았다 — 시험이 의미가 없다");

    // 작업 트리 .gitattributes 규칙이 filter 에 맞아도 실행하지 않고 LFS 판정은 그대로다
    let dir = import_at(tmp.0.join("worktree"), "basic");
    let marker = plant_filter(&tmp.0, &dir, "evil", "worktree");
    fs::write(dir.join(".gitattributes"), ".gitattributes filter=evil\n*.bin filter=lfs diff=lfs merge=lfs -text\n").unwrap();
    let router = mode.router(&[("w", dir.as_path())]);
    assert_eq!(extras_lfs(&router, "w").await, json!(true));
    assert!(!marker.exists(), "작업 트리 .gitattributes 읽기가 저장소가 심은 명령을 실행했다");

    // .git/info/attributes 도 마찬가지다
    let dir = import_at(tmp.0.join("info"), "basic");
    let marker = plant_filter(&tmp.0, &dir, "evil", "info");
    let info = raw_git(&dir).args(["rev-parse", "--path-format=absolute", "--git-path", "info/attributes"]).output().unwrap();
    let info = PathBuf::from(String::from_utf8(info.stdout).unwrap().trim());
    fs::create_dir_all(info.parent().unwrap()).unwrap();
    fs::write(&info, "attributes filter=evil\n*.bin filter=lfs\n").unwrap();
    let router = mode.router(&[("i", dir.as_path())]);
    assert_eq!(extras_lfs(&router, "i").await, json!(true));
    assert!(!marker.exists(), "info/attributes 읽기가 저장소가 심은 명령을 실행했다");

    // 무력화할 수 없는 저장소(드라이버 이름에 `=`)는 파일을 읽지 않고 lfs:false 다
    let dir = import_at(tmp.0.join("eq"), "basic");
    let marker = plant_filter(&tmp.0, &dir, "a=b", "eq");
    fs::write(dir.join(".gitattributes"), ".gitattributes filter=a\n*.bin filter=lfs\n").unwrap();
    let router = mode.router(&[("e", dir.as_path())]);
    assert_eq!(extras_lfs(&router, "e").await, json!(false));
    assert!(!marker.exists(), "무력화할 수 없는 저장소인데 표지 명령이 실행됐다");
    mode.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_index_읽기는_저장소가_심은_filter_를_실행하지_않는다() {
    no_index_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_index_읽기는_ssh_에서도_저장소가_심은_filter_를_실행하지_않는다() {
    no_index_scenarios(&Mode::ssh().await).await;
}

#[test]
fn lfs_argv_는_상수_뒤_검증된_값뿐이고_작업_트리_읽기는_한_곳이다() {
    // 파일 내용은 `cat-file blob`·`diff --no-index` 로만 읽는다 — `git show`·`--textconv`·`--filters` 인자는 없다.
    // `no_index_args(` 의 호출은 `read_worktree_attributes` 안 한 곳뿐이고, 그 함수는 가드(`&FilterGuard`)를 필수 인자로 받는다(가드 없이 `--no-index` argv 를 만들 수 없다)
    let src = fs::read_to_string(repo_root().join("src-tauri/src/git/lfs.rs")).unwrap().replace("\r\n", "\n");
    let body = src.split("#[cfg(test)]").next().unwrap();
    for banned in ["\"show\"", "\"--textconv\"", "\"--filters\"", "\"hash-object\"", "\"submodule\""] {
        assert!(!body.contains(banned), "lfs.rs 가 {banned} 를 쓴다");
    }
    assert_eq!(body.matches("no_index_args(").count(), 2, "정의 하나와 호출 하나");
    assert_eq!(body.matches("\"--no-index\"").count(), 1, "--no-index 인자는 no_index_args 한 곳에만 있다");
    assert!(body.contains("pub fn no_index_args(guard: &FilterGuard, "), "no_index_args 는 가드를 필수 인자로 받아야 한다");
    assert!(body.contains("fn read_worktree_attributes(exec: &GitExecutor, repo_path: &str, path: &str, guard: &FilterGuard, "), "read_worktree_attributes 는 가드를 필수 인자로 받아야 한다");
    let call = body.split("async fn read_worktree_attributes").nth(1).expect("read_worktree_attributes");
    assert!(call.split("\n}\n").next().unwrap().contains("no_index_args(guard, "), "호출이 read_worktree_attributes 밖에 있거나 가드를 넘기지 않는다");
}

// ── remote 조회: gh:repo · RoutedRepos(Launchpad 의 remote 목록·아바타의 push 된 커밋) ────────────────

/// 사이드카 질의의 가짜. 받은 질의를 적고 정해진 값을 돌려준다 — **Rust 가 git 을 맡는 저장소의 질의가 여기 오면 안 된다**(오면 사이드카가 그 저장소에 git 을 돌린다)
#[derive(Default)]
struct FakeSidecar {
    asked: Mutex<Vec<String>>,
    invalidated: AtomicUsize,
}

impl FakeSidecar {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

fn side_ref() -> GhRepoRef {
    GhRepoRef { owner: "side".into(), repo: "car".into(), remote: "origin".into() }
}

impl RepoQuery for FakeSidecar {
    fn gh_repo<'a>(&'a self, repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        self.asked.lock().unwrap().push(format!("ghRepo {repo_id}"));
        async { Ok(Some(side_ref())) }.boxed()
    }

    fn gh_remotes<'a>(&'a self, repo_id: &'a str, _cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        self.asked.lock().unwrap().push(format!("ghRemotes {repo_id}"));
        async { Ok(vec![side_ref()]) }.boxed()
    }

    fn pushed_shas_by_author<'a>(&'a self, repo_id: &'a str, email: &'a str, _limit: usize, _cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        self.asked.lock().unwrap().push(format!("pushedShas {repo_id} {email}"));
        async { Ok(vec!["a".repeat(40)]) }.boxed()
    }

    fn invalidate_gh_remotes(&self) {
        self.invalidated.fetch_add(1, Ordering::SeqCst);
    }

    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        self.asked.lock().unwrap().push("clientId".into());
        async { Some("Iv1.side".to_string()) }.boxed()
    }

    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        self.asked.lock().unwrap().push("repoIds".into());
        async { vec!["side-id".to_string()] }.boxed()
    }
}

fn ref_value(r: &GhRepoRef) -> Value {
    json!({ "owner": r.owner, "repo": r.repo, "remote": r.remote })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_조회_시나리오가_ts_골든과_같다() {
    remote_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_조회_시나리오가_ssh_에서도_ts_골든과_같다() {
    remote_scenarios(&Mode::ssh().await).await;
}

/// `gh:repo` 는 라우터 채널로, remote 목록·push 된 커밋은 GitHub 서비스가 부르는 `RoutedRepos` 로 돈다(앱과 같은 길). 어느 단계도 사이드카에 묻지 않는다
async fn remote_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/remotes.scenarios.json");
    let scenarios = all["scenarios"].as_array().expect("scenarios");
    let golden = ["ghRepo", "ghRemotes", "pushedShas"].map(|n| read_json(&format!("tests/golden/channels/{n}.json")));
    let tmp = TempDir::new("remotes");
    let mut cursor = [0usize; 3];
    let mut diffs = Vec::new();
    for (si, sc) in scenarios.iter().enumerate() {
        let name = sc["name"].as_str().unwrap();
        let dir = import_at(tmp.0.join(format!("s{si}")), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = Arc::new(mode.router(&[(id.as_str(), dir.as_path())]));
        let side = Arc::new(FakeSidecar::default());
        let repos = RoutedRepos::new(router.clone(), side.clone());
        let rev = |expr: &str| rev_parse(&dir, expr);
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(args) = step.get("git") {
                let args = expand(args, &rev);
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                assert!(refs_git(&dir).args(&args).status().unwrap().success(), "{name}: git {args:?}");
                continue;
            }
            if step.get("invalidate").is_some() {
                repos.invalidate_gh_remotes();
                continue;
            }
            let (slot, args, got) = if step.get("ghRepo").is_some() {
                let env = envelope(router.route("gh:repo", &json!([id])).await);
                (0, None, if env["ok"] == true { json!({ "ok": true, "data": env["data"] }) } else { json!({ "ok": false, "code": env["code"] }) })
            } else if step.get("ghRemotes").is_some() {
                let got = match repos.gh_remotes(&id, CancellationToken::new()).await {
                    Ok(list) => json!({ "ok": true, "data": list.iter().map(ref_value).collect::<Vec<_>>() }),
                    Err(_) => json!({ "ok": false, "code": "ERROR" }),
                };
                (1, None, got)
            } else {
                let p = &step["pushed"];
                let got = match repos.pushed_shas_by_author(&id, p["email"].as_str().unwrap(), p["limit"].as_u64().unwrap() as usize, CancellationToken::new()).await {
                    Ok(list) => json!({ "ok": true, "data": list }),
                    Err(_) => json!({ "ok": false, "code": "ERROR" }),
                };
                (2, Some(p.clone()), got)
            };
            let g = &golden[slot]["scenarios"][cursor[slot]];
            cursor[slot] += 1;
            assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
            if let Some(a) = args {
                assert_eq!(g["args"], a, "{name} 단계 {i}: 인자가 골든과 다르다");
            }
            if got != g["result"] {
                diffs.push(format!("{name} 단계 {i}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&g["result"])));
            }
        }
        assert_eq!(side.asked(), Vec::<String>::new(), "{name}: Rust 가 맡는 저장소의 질의가 사이드카로 갔다");
        assert!(!dir.join("pwned.txt").exists(), "{name}: 옵션처럼 보이는 이메일이 파일을 만들었다");
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!(golden.each_ref().map(|g| g["scenarios"].as_array().unwrap().len()), cursor, "골든 항목 수가 시나리오와 다르다");
    mode.finish();
}

/// 소유 판정은 라우터의 다른 git 채널과 같은 한 술어다: Rust 가 맡는 저장소는 Rust, github·미러에 없는 id·풀 없는 ssh·스위치 꺼짐은 사이드카에 위임하고,
/// 미러가 거부한 저장소는 **위임하지 않고** 오류다(위임하면 미러가 막은 경로에 사이드카가 git 을 돌린다). 설정 읽기(`github_client_id`·`repo_ids`)는 항상 사이드카다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_질의는_rust_가_맡는_저장소만_답하고_나머지는_사이드카에_위임한다() {
    let tmp = TempDir::new("remoteown");
    let dir = import(&tmp.0, "basic");
    assert!(refs_git(&dir).args(["remote", "add", "origin", "https://github.com/octo/hello.git"]).status().unwrap().success());
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: dir.to_string_lossy().into_owned(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    let router = Arc::new(Router::new(true, false, m.clone()));
    let side = Arc::new(FakeSidecar::default());
    let repos = RoutedRepos::new(router.clone(), side.clone());
    let none = || CancellationToken::new();

    // Rust 가 맡는 저장소: 사이드카에 묻지 않는다
    assert_eq!(repos.gh_repo("a").await, Ok(Some(GhRepoRef { owner: "octo".into(), repo: "hello".into(), remote: "origin".into() })));
    assert_eq!(repos.gh_remotes("a", none()).await.unwrap().len(), 1);
    assert_eq!(repos.pushed_shas_by_author("a", "dev@example.com", 3, none()).await, Ok(vec![]));
    assert_eq!(side.asked(), Vec::<String>::new());

    // 미러에 없는 id·github·풀 없는 ssh: 사이드카가 답한다
    for id in ["nope", "g", "s"] {
        assert_eq!(repos.gh_repo(id).await, Ok(Some(side_ref())), "{id}");
        assert_eq!(repos.gh_remotes(id, none()).await, Ok(vec![side_ref()]), "{id}");
        assert_eq!(repos.pushed_shas_by_author(id, "x@y.z", 3, none()).await, Ok(vec!["a".repeat(40)]), "{id}");
    }
    assert_eq!(side.asked().len(), 9);

    // 미러가 거부한 저장소: 오류이고 사이드카에 가지 않는다
    let before = side.asked().len();
    assert!(repos.gh_repo("unc").await.is_err());
    assert!(repos.gh_remotes("unc", none()).await.is_err());
    assert!(repos.pushed_shas_by_author("unc", "x@y.z", 3, none()).await.is_err());
    assert_eq!(side.asked().len(), before, "거부된 저장소의 질의가 사이드카로 갔다");

    // 스위치가 꺼진 라우터(ANTLER_RUST_GIT=0)는 전부 사이드카다
    let off = RoutedRepos::new(Arc::new(Router::new(false, false, m)), side.clone());
    assert_eq!(off.gh_repo("a").await, Ok(Some(side_ref())));
    assert_eq!(side.asked().len(), before + 1);

    // 설정 읽기는 항상 사이드카
    assert_eq!(repos.github_client_id().await.as_deref(), Some("Iv1.side"));
    assert_eq!(repos.repo_ids().await, vec!["side-id".to_string()]);
    // 무효화는 Rust 캐시를 비우고 사이드카에도 알린다
    repos.invalidate_gh_remotes();
    assert_eq!(side.invalidated.load(Ordering::SeqCst), 1);
    assert_eq!(router.rust_handled(), 0, "서비스 길의 질의는 채널 처리 수에 세지 않는다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_repo_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: "/nonexistent-antler-remote".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    assert!(RUST_CHANNELS.contains(&"gh:repo"));
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    for id in ["nope", "g", "s"] {
        assert!(matches!(router.route("gh:repo", &json!([id])).await, Routed::Sidecar), "{id}");
    }
    // 문자열이 아니거나 없는 id: 사이드카가 TS 와 같은 오류를 만든다
    for args in [json!([]), json!([null]), json!([5]), json!([{ UNDEFINED_ARG: true }]), json!({ "a": 1 }), json!("a")] {
        assert!(matches!(router.route("gh:repo", &args).await, Routed::Sidecar), "{args}");
    }
    assert!(matches!(off.route("gh:repo", &json!(["a"])).await, Routed::Sidecar));
    let env = envelope(router.route("gh:repo", &json!(["unc"])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{env}");
    // 저장소 경로가 없어 git 이 실패하면 GIT 오류다(없는 것으로 삼키지 않는다 — TS 도 던진다)
    let env = envelope(router.route("gh:repo", &json!(["a"])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    assert_eq!(router.rust_handled(), 1);
    assert_eq!(off.rust_handled(), 0);
}

/// TS: `repos:update`·`repos:remove` 핸들러가 `invalidateGhRemotes(repoId)` 를 부른다 — 모든 GitHub remote 목록(10분)만 비우고 `gh:repo` 추론(60초)은 그대로다.
/// 응답은 사이드카가 하므로 라우터는 Sidecar 를 돌려준다. 다른 저장소의 캐시는 건드리지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 저장소를_바꾸거나_지우면_remote_목록_캐시만_비워진다() {
    let tmp = TempDir::new("remotecache");
    let a = import(&tmp.0, "basic");
    let b = import_at(tmp.0.join("b"), "basic");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("a", a.as_path()), ("b", b.as_path())])));
    let repos = RoutedRepos::new(router.clone(), Arc::new(FakeSidecar::default()));
    let add = |dir: &Path, name: &str, url: &str| assert!(refs_git(dir).args(["remote", "add", name, url]).status().unwrap().success());
    for id in ["a", "b"] {
        assert_eq!(repos.gh_remotes(id, CancellationToken::new()).await, Ok(vec![]));
    }
    assert_eq!(repos.gh_repo("a").await, Ok(None));
    add(&a, "origin", "https://github.com/octo/hello.git");
    add(&b, "origin", "https://github.com/octo/hello.git");
    // 캐시 안: 바뀐 remote 가 보이지 않는다
    assert_eq!(repos.gh_remotes("a", CancellationToken::new()).await, Ok(vec![]));
    assert_eq!(repos.gh_repo("a").await, Ok(None));
    assert!(matches!(router.route("repos:update", &json!(["a", { "name": "x" }])).await, Routed::Sidecar));
    assert_eq!(repos.gh_remotes("a", CancellationToken::new()).await.unwrap().len(), 1, "repos:update 가 a 의 목록 캐시를 비운다");
    assert_eq!(repos.gh_remotes("b", CancellationToken::new()).await, Ok(vec![]), "다른 저장소는 그대로다");
    assert_eq!(repos.gh_repo("a").await, Ok(None), "gh:repo 의 60초 추론은 그대로다(TS 와 같다)");
    assert!(matches!(router.route("repos:remove", &json!(["b"])).await, Routed::Sidecar));
    assert_eq!(repos.gh_remotes("b", CancellationToken::new()).await.unwrap().len(), 1, "repos:remove 도 같다");
    // 형식이 틀린 인자는 아무것도 비우지 않고 사이드카가 거부한다
    for args in [json!([]), json!([5]), json!({ "a": 1 })] {
        assert!(matches!(router.route("repos:update", &args).await, Routed::Sidecar), "{args}");
    }
    // 무효화(`gh:launchpad` 의 fresh)는 전부 비운다
    add(&a, "upstream", "https://github.com/octo/other.git");
    repos.invalidate_gh_remotes();
    assert_eq!(repos.gh_remotes("a", CancellationToken::new()).await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_repo_는_첫_페이지_문이_열릴_때까지_기다린다() {
    let tmp = TempDir::new("remotegate");
    let dir = import(&tmp.0, "basic");
    assert!(refs_git(&dir).args(["remote", "add", "origin", "https://github.com/octo/hello.git"]).status().unwrap().success());
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    assert!(matches!(router.route("repos:watch", &json!(["b"])).await, Routed::Sidecar));
    assert!(router.repo_state().is_armed("b"));
    let r = router.clone();
    let pending = tokio::spawn(async move { envelope(r.route("gh:repo", &json!(["b"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!pending.is_finished(), "문이 닫혀 있는 동안 gh:repo 가 끝났다");
    router.note_sink()(NoteOp::FirstPage, "b");
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), pending).await.expect("문이 열리면 끝나야 한다").unwrap();
    assert_eq!(env["data"], json!({ "owner": "octo", "repo": "hello", "remote": "origin" }));
    // 목록 조회(Launchpad)는 문을 기다리지 않는다(TS `ghRemotes` 에는 문이 없다)
    router.repo_state().arm("b");
    let repos = RoutedRepos::new(router.clone(), Arc::new(FakeSidecar::default()));
    assert_eq!(repos.gh_remotes("b", CancellationToken::new()).await.unwrap().len(), 1);
}

/// 서비스 길의 입력은 renderer 값(아바타 작성자 이메일)에서 온다. 이메일·개수가 틀리면 git 을 부르지 않고 오류다 — git 이 실패해도 `[]` 가 되므로 오류인지로 안 띄운 것을 알 수 있다.
/// 옵션처럼 보이는 이메일은 `--author=<…>` 한 인자 안에만 있어 옵션이 되지 못한다(`pushed_shas_args` 단위 시험과 골든 시나리오가 파일이 안 생김을 본다)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_된_커밋_조회는_적대적_입력을_git_없이_거절한다() {
    let repos = RoutedRepos::new(Arc::new(Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-remote"))]))), Arc::new(FakeSidecar::default()));
    for (email, limit, want) in [
        ("", 3, "잘못된 이메일입니다"),
        ("a\0b", 3, "잘못된 이메일입니다"),
        ("a\nb", 3, "잘못된 이메일입니다"),
        ("a\rb", 3, "잘못된 이메일입니다"),
        (&"a".repeat(321), 3, "잘못된 이메일입니다"),
        ("a@b.c", 0, "잘못된 개수입니다"),
        ("a@b.c", 101, "잘못된 개수입니다"),
    ] {
        assert_eq!(repos.pushed_shas_by_author("a", email, limit, CancellationToken::new()).await, Err(want.to_string()), "{email:?} {limit}");
    }
    // 검증을 통과하면 git 이 돌고(경로가 없어 실패 → TS 처럼 빈 목록)
    assert_eq!(repos.pushed_shas_by_author("a", &"a".repeat(320), 100, CancellationToken::new()).await, Ok(vec![]));
}

/// 이미 취소된 호출은 git 을 띄우지 않고 `aborted` 다(저장소 경로가 없다 — 띄웠다면 GIT 오류다). 제한 시간이 지난 호출은 오류이고 이후 호출은 계속 된다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_질의는_취소와_제한_시간을_따른다() {
    let tmp = TempDir::new("remotecancel");
    let dir = import(&tmp.0, "basic");
    let router = Arc::new(Router::new(true, false, mirror_of(&[("gone", Path::new("/nonexistent-antler-remote")), ("a", dir.as_path())])));
    let repos = RoutedRepos::new(router.clone(), Arc::new(FakeSidecar::default()));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(repos.gh_remotes("gone", cancelled.clone()).await, Err("aborted".to_string()));
    assert_eq!(repos.pushed_shas_by_author("gone", "a@b.c", 3, cancelled).await, Err("aborted".to_string()));
    // 취소된 조회는 캐시에 남지 않는다
    assert!(repos.gh_remotes("gone", CancellationToken::new()).await.is_err(), "경로가 없으면 git 이 실패한다");
    // 제한 시간
    let slow = RoutedRepos::new(router.clone(), Arc::new(FakeSidecar::default())).with_timeout(std::time::Duration::ZERO);
    assert_eq!(slow.gh_repo("a").await, Err("git 이 제때 응답하지 않았습니다".to_string()));
    assert_eq!(repos.gh_repo("a").await, Ok(None), "시간 초과 뒤에도 큐가 막히지 않는다");
}

// ── 작업 트리: git:wipStatus · git:wipDiff ──────────────

/// wipStatus 결과의 untracked 가 20개를 넘으면 `{count, first, last}` 로 줄인다(`tests/goldenChannels.test.ts` 의 `summarizeWipStatus` 와 같다)
fn summarize_wip_status(mut data: Value) -> Value {
    let u = data["untracked"].as_array().expect("untracked").clone();
    if u.len() > 20 {
        data["untracked"] = json!({ "count": u.len(), "first": u[0], "last": u[u.len() - 1] });
    }
    data
}

async fn wip_scenarios(mode: &Mode) {
    let all = read_json("tests/fixtures/repos/wip.scenarios.json");
    let scenarios = all["scenarios"].as_array().expect("scenarios");
    let golden = read_json("tests/golden/channels/wip.json");
    let golden = golden["scenarios"].as_array().expect("golden");
    let tmp = TempDir::new("wip");
    let mut cursor = 0usize;
    let mut diffs = Vec::new();
    for (si, sc) in scenarios.iter().enumerate() {
        let name = sc["name"].as_str().unwrap();
        // 시나리오마다 새 디렉터리에 import 한다(작업 트리·인덱스가 다른 시나리오에 번지지 않게)
        let dir = import_at(tmp.0.join(format!("s{si}")), sc["repo"].as_str().unwrap());
        let id = format!("s{si}");
        let router = mode.router(&[(id.as_str(), dir.as_path())]);
        for (i, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(args) = step.get("git") {
                let args: Vec<&str> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
                assert!(refs_git(&dir).args(&args).status().unwrap().success(), "{name}: git {args:?}");
                continue;
            }
            if let Some(w) = step.get("write") {
                let target = dir.join(w[0].as_str().unwrap());
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(target, w[1].as_str().unwrap()).unwrap();
                continue;
            }
            if let Some(p) = step.get("rm") {
                fs::remove_file(dir.join(p.as_str().unwrap())).unwrap();
                continue;
            }
            if let Some(b) = step.get("bulk") {
                let (prefix, n) = (b[0].as_str().unwrap(), b[1].as_u64().unwrap());
                fs::create_dir_all(dir.join(format!("{prefix}0")).parent().unwrap()).unwrap();
                for k in 0..n {
                    fs::write(dir.join(format!("{prefix}{k}")), format!("{k}\n")).unwrap();
                }
                continue;
            }
            let rid = json!(format!("req-{si}-{i}"));
            let (env, args) = match step.get("wipDiff") {
                Some(req) => (envelope(router.route("git:wipDiff", &json!([id, req, rid])).await), Some(req.clone())),
                None => (envelope(router.route("git:wipStatus", &json!([id, rid])).await), None),
            };
            let msg = step.get("msg") == Some(&json!(true));
            let got = if env["ok"] == true {
                let data = env["data"].clone();
                json!({ "ok": true, "data": if args.is_some() { summarize_diff(data) } else { summarize_wip_status(data) } })
            } else if msg {
                json!({ "ok": false, "code": env["code"], "error": env["error"] })
            } else {
                json!({ "ok": false, "code": env["code"] })
            };
            let g = golden.get(cursor).unwrap_or_else(|| panic!("{name}: 골든이 모자란다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다"));
            cursor += 1;
            assert_eq!((&g["name"], &g["step"]), (&json!(name), &json!(i)), "{name}: 골든의 순서가 다르다 — UPDATE_GOLDEN=1 로 골든을 다시 만든다");
            if let Some(a) = &args {
                assert_eq!(norm_numbers(&g["args"]), norm_numbers(a), "{name} 단계 {i}: 인자가 골든과 다르다");
            }
            if norm_numbers(&got) != norm_numbers(&g["result"]) {
                diffs.push(format!("{name} 단계 {i} ({}): 골든과 다르다\n  Rust: {}\n  TS  : {}", args.as_ref().map_or("wipStatus".to_string(), short), short(&got), short(&g["result"])));
            }
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    assert_eq!(golden.len(), cursor, "골든 항목 수가 시나리오와 다르다");
    mode.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 작업_트리_시나리오가_ts_골든과_같다() {
    wip_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 작업_트리_시나리오가_ssh_에서도_ts_골든과_같다() {
    wip_scenarios(&Mode::ssh().await).await;
}

/// 같은 크기로 내용을 바꾸고 mtime 을 앞으로 옮긴다 — stat 이 달라야 git 이 내용을 해시하려고 clean 필터를 돈다(크기가 다르면 읽지 않고 "수정됨"으로 본다)
fn dirty(path: &Path, body: &str, ahead_secs: u64) {
    fs::write(path, body).unwrap();
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(ahead_secs)).unwrap();
}

async fn wip_status_of(router: &Router, id: &str) -> Value {
    envelope(router.route("git:wipStatus", &json!([id])).await)
}

async fn wip_diff_of(router: &Router, id: &str, req: Value) -> Value {
    envelope(router.route("git:wipDiff", &json!([id, req])).await)
}

/// 저장소가 심은 filter 가 `wipStatus`·`wipDiff`(staged·unstaged·untracked 모두)에서 실행되지 않는다. 표지 파일이 생기면 실행된 것이다.
/// 시험마다 **대조군**(날 git 이 표지를 만든다)을 먼저 확인한다 — 대조군이 통과하지 못하면 본 시험은 의미가 없다.
/// (시험 스레드의 스택이 작아 한 async 함수에 다 담으면 넘친다 — 경우마다 함수를 나누고 Box 로 부른다)
async fn wip_filter_scenarios(mode: &Mode) {
    let tmp = TempDir::new("wipfilter");
    Box::pin(wip_filter_plain(mode, &tmp.0)).await;
    Box::pin(wip_filter_lfs_like(mode, &tmp.0)).await;
    Box::pin(wip_filter_equals(mode, &tmp.0)).await;
    Box::pin(wip_filter_non_utf8(mode, &tmp.0)).await;
    Box::pin(wip_filter_weird_name(mode, &tmp.0)).await;
    mode.finish();
}

/// 1) required 인 일반 드라이버: 실행하지 않고 실패하지도 않는다
async fn wip_filter_plain(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("evil"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    fs::write(dir.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    let marker = plant_filter(tmp, &dir, "evil", "evil");
    assert!(raw_git(&dir).args(["config", "filter.evil.required", "true"]).status().unwrap().success());
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let _ = raw_git(&dir).args(["status", "--porcelain"]).output().unwrap();
    assert!(marker.exists(), "대조군: 날 git status 가 표지 명령을 실행하지 않았다 — 시험이 의미가 없다");
    fs::remove_file(&marker).unwrap();
    let router = mode.router(&[("evil", dir.as_path())]);
    let st = wip_status_of(&router, "evil").await;
    assert_eq!(st["ok"], true, "{st}");
    assert!(st["data"]["unstaged"].as_array().unwrap().iter().any(|f| f["path"] == "a.txt"), "{st}");
    let d = wip_diff_of(&router, "evil", json!({ "area": "unstaged", "path": "a.txt" })).await;
    assert_eq!(d["ok"], true, "{d}");
    assert!(!d["data"]["hunks"].as_array().unwrap().is_empty(), "{d}");
    assert_eq!(wip_diff_of(&router, "evil", json!({ "area": "staged", "path": "a.txt" })).await["ok"], true);
    // untracked(`--no-index`)도 저장소의 설정·속성을 읽어 clean 을 돈다 — 내용은 그대로 나온다
    fs::write(dir.join("u.txt"), "hello\n").unwrap();
    assert!(wip_status_of(&router, "evil").await["data"]["untracked"].as_array().unwrap().iter().any(|p| p == "u.txt"));
    let d = wip_diff_of(&router, "evil", json!({ "area": "untracked", "path": "u.txt" })).await;
    assert_eq!(d["ok"], true, "{d}");
    assert!(d["data"].to_string().contains("hello"), "{d}");
    assert!(!marker.exists(), "wipStatus·wipDiff 가 저장소가 심은 filter 명령을 실행했다");
}

/// 2) git-lfs 처럼 보이게 뒤에 명령을 붙인 기본 값도 덮어 쓴다(값을 보지 않는다)
async fn wip_filter_lfs_like(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("lfslike"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    fs::write(dir.join(".gitattributes"), "*.txt filter=lfs\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    let m = tmp.join("MARK-lfslike");
    // `process` 가 있으면 git 은 clean 대신 그것을 쓴다 — 표지가 나오려면 clean 만 둔다(가드는 process 도 같이 비운다 — `wip.rs` 단위 시험)
    let cmd = format!("git-lfs clean -- %f; echo x > '{}'", m.to_string_lossy().replace('\\', "/"));
    for (k, v) in [("filter.lfs.clean", cmd.as_str()), ("filter.lfs.smudge", "git-lfs smudge -- %f")] {
        assert!(raw_git(&dir).args(["config", k, v]).status().unwrap().success());
    }
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let _ = raw_git(&dir).args(["status", "--porcelain"]).output().unwrap();
    assert!(m.exists(), "대조군: 날 git status 가 표지 명령을 실행하지 않았다");
    fs::remove_file(&m).unwrap();
    let router = mode.router(&[("lfslike", dir.as_path())]);
    assert_eq!(wip_status_of(&router, "lfslike").await["ok"], true);
    assert_eq!(wip_diff_of(&router, "lfslike", json!({ "area": "unstaged", "path": "a.txt" })).await["ok"], true);
    assert!(!m.exists(), "git-lfs 이름의 드라이버가 예외로 실행됐다");
}

/// 3) 이름에 `=` 가 든 드라이버는 `-c filter.a=b.clean=` 로 덮이지 않는다 — 작업 트리를 읽지 않고 거부한다
async fn wip_filter_equals(mode: &Mode, tmp: &Path) {
    let msg_of = |env: &Value| env["error"].as_str().unwrap_or_default().to_string();
    let dir = import_at(tmp.join("eq"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    fs::write(dir.join(".gitattributes"), "*.txt filter=a=b\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    let marker = plant_filter(tmp, &dir, "a=b", "eq");
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let _ = raw_git(&dir).args(["-c", "filter.a=b.clean=", "status", "--porcelain"]).output().unwrap();
    assert!(marker.exists(), "대조군: `-c filter.a=b.clean=` 가 덮어 쓰지 못해 명령이 실행돼야 한다");
    fs::remove_file(&marker).unwrap();
    fs::write(dir.join("u.txt"), "x\n").unwrap();
    dirty(&dir.join("a.txt"), "hellp\n", 10);
    let router = mode.router(&[("eq", dir.as_path())]);
    let mut unsafe_msg = String::new();
    for (ch, args) in [
        ("git:wipStatus", json!(["eq"])),
        ("git:wipDiff", json!(["eq", { "area": "unstaged", "path": "a.txt" }])),
        ("git:wipDiff", json!(["eq", { "area": "staged", "path": "a.txt" }])),
    ] {
        let env = envelope(router.route(ch, &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{ch}: {env}");
        assert!(msg_of(&env).contains('='), "{env}");
        unsafe_msg = msg_of(&env);
    }
    assert_eq!(unsafe_msg, antler_lib::git::wip::UNSAFE_FILTER_MSG);
    assert!(!marker.exists(), "무력화할 수 없는 저장소인데 표지 명령이 실행됐다");

    // untracked diff(`--no-index`)도 같다: 허용 목록은 성공한 wipStatus 가 채우므로, 드라이버를 심기 전에 보고를 받아 둔 뒤 심는다
    let dir = import_at(tmp.join("eq2"), "empty");
    fs::write(dir.join("u.txt"), "hello\n").unwrap();
    let router = mode.router(&[("eq2", dir.as_path())]);
    assert_eq!(wip_status_of(&router, "eq2").await["data"]["untracked"], json!(["u.txt"]));
    fs::write(dir.join(".gitattributes"), "*.txt filter=a=b\n").unwrap();
    let marker = plant_filter(tmp, &dir, "a=b", "eq2");
    let env = wip_diff_of(&router, "eq2", json!({ "area": "untracked", "path": "u.txt" })).await;
    assert_eq!((env["ok"].clone(), env["code"].clone(), msg_of(&env)), (json!(false), json!("GIT"), antler_lib::git::wip::UNSAFE_FILTER_MSG.to_string()), "{env}");
    assert!(!marker.exists(), "무력화할 수 없는 저장소의 untracked diff 가 표지 명령을 실행했다");
}

/// 4) 잘못된 UTF-8 이름의 드라이버: 문자열로 바꾸면 U+FFFD 가 돼 `-c` 가 실제 드라이버를 덮지 못한다 — 작업 트리를 읽지 않고 거부한다
async fn wip_filter_non_utf8(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("ff"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    let mut attrs = b"* filter=".to_vec();
    attrs.extend_from_slice(&[0xff, b'\n']);
    fs::write(dir.join(".gitattributes"), attrs).unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    let m = tmp.join("MARK-ff");
    // 설정 값은 따옴표로 감싸야 `;` 가 주석이 되지 않는다(안의 따옴표는 `\"`)
    let mut cfg = b"[filter \"".to_vec();
    cfg.extend_from_slice(&[0xff]);
    cfg.extend_from_slice(format!("\"]\n\tclean = \"sh -c \\\"echo x > '{}'; cat\\\" --\"\n", m.to_string_lossy().replace('\\', "/")).as_bytes());
    use std::io::Write as _;
    fs::OpenOptions::new().append(true).open(dir.join(".git").join("config")).unwrap().write_all(&cfg).unwrap();
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let _ = raw_git(&dir).args(["status", "--porcelain"]).output().unwrap();
    assert!(m.exists(), "대조군: 날 git status 가 표지 명령을 실행하지 않았다");
    fs::remove_file(&m).unwrap();
    dirty(&dir.join("a.txt"), "hellp\n", 10);
    let router = mode.router(&[("ff", dir.as_path())]);
    for (ch, args) in [("git:wipStatus", json!(["ff"])), ("git:wipDiff", json!(["ff", { "area": "unstaged", "path": "a.txt" }]))] {
        let env = envelope(router.route(ch, &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone(), env["error"].clone()), (json!(false), json!("GIT"), json!(antler_lib::git::wip::UNSAFE_FILTER_MSG)), "{ch}");
    }
    assert!(!m.exists(), "잘못된 UTF-8 이름의 드라이버 저장소에서 표지 명령이 실행됐다");
}

/// 5) 드라이버 이름에 셸 메타 문자·따옴표가 있어도 `-c` 값이 한 인자로 도착한다(ssh 는 원격 셸을 한 번 거치므로 인용이 어긋나면 주입된다).
///    덮어쓰기가 이름 그대로 이뤄져 매칭되는 드라이버도 함께 막힌다
async fn wip_filter_weird_name(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("quote"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    fs::write(dir.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    let marker = plant_filter(tmp, &dir, "evil", "quote");
    let inj = tmp.join("INJECTED");
    let weird = format!("x;sh -c \"echo y > '{}'\";$(echo z)`id`&|#", inj.to_string_lossy().replace('\\', "/"));
    assert!(raw_git(&dir).args(["config", &format!("filter.{weird}.clean"), "cat"]).status().unwrap().success(), "이름이 이상한 드라이버를 심지 못했다");
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let router = mode.router(&[("quote", dir.as_path())]);
    let st = wip_status_of(&router, "quote").await;
    assert_eq!(st["ok"], true, "{st}");
    assert_eq!(wip_diff_of(&router, "quote", json!({ "area": "unstaged", "path": "a.txt" })).await["ok"], true);
    assert!(!marker.exists(), "이름이 이상한 드라이버가 있는 저장소에서 evil 드라이버가 실행됐다");
    assert!(!inj.exists(), "드라이버 이름이 셸에서 명령으로 실행됐다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 악성_저장소의_filter_는_작업_트리_채널이_실행하지_않는다() {
    wip_filter_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 악성_저장소의_filter_는_ssh_에서도_작업_트리_채널이_실행하지_않는다() {
    wip_filter_scenarios(&Mode::ssh().await).await;
}

// ── 작업 트리 diff 는 plumbing 이다(`tests/wipPlumbing.test.ts` 와 같은 세 건 + 출력) ──────────────

/// 표지를 남기는 셸 명령 본문(`echo x >> '<표지>'`). 표지는 임시 디렉터리 안에만 쓴다
fn marker_cmd(marker: &Path) -> String {
    format!("echo x >> '{}'", marker.to_string_lossy().replace('\\', "/"))
}

fn commit_all(dir: &Path) {
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(dir).args(&args).status().unwrap().success());
    }
}

/// A. 빈 이름 filter 드라이버(`[filter ""]` + `* filter=`)는 가드가 덮지 못해 거부한다
async fn wip_plumbing_empty_filter(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("a"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    commit_all(&dir);
    let marker = tmp.join("MARK-a");
    // `git config` 는 빈 서브섹션을 못 쓰는 경우가 있어 파일에 직접 쓴다
    {
        use std::io::Write as _;
        let mut cfg = fs::OpenOptions::new().append(true).open(dir.join(".git").join("config")).unwrap();
        write!(cfg, "[filter \"\"]\n\tclean = \"sh -c \\\"{}; cat\\\" --\"\n", marker_cmd(&marker)).unwrap();
        fs::create_dir_all(dir.join(".git").join("info")).unwrap();
        let mut attrs = fs::OpenOptions::new().create(true).append(true).open(dir.join(".git").join("info").join("attributes")).unwrap();
        attrs.write_all(b"* filter=\n").unwrap();
    }
    dirty(&dir.join("a.txt"), "jello\n", 5);
    let _ = raw_git(&dir).args(["status", "--porcelain"]).output().unwrap();
    assert!(marker.exists(), "대조군: 날 git status 가 빈 이름 드라이버의 명령을 실행하지 않았다 — 시험이 의미가 없다");
    fs::remove_file(&marker).unwrap();
    dirty(&dir.join("a.txt"), "hellp\n", 10);
    let router = mode.router(&[("a", dir.as_path())]);
    let st = wip_status_of(&router, "a").await;
    assert_eq!((st["ok"].clone(), st["code"].clone(), st["error"].clone()), (json!(false), json!("GIT"), json!(antler_lib::git::wip::UNSAFE_FILTER_MSG)), "{st}");
    for area in ["unstaged", "staged"] {
        let d = wip_diff_of(&router, "a", json!({ "area": area, "path": "a.txt" })).await;
        assert_eq!((d["ok"].clone(), d["error"].clone()), (json!(false), json!(antler_lib::git::wip::UNSAFE_FILTER_MSG)), "{area}: {d}");
    }
    assert!(!marker.exists(), "빈 이름 드라이버의 명령이 실행됐다");
}

/// B. 상위 저장소 `diff.submodule=diff` + 인덱스에는 파일·작업 트리에는 중첩 저장소 → porcelain diff 는 자식 git diff 를 띄워 그 저장소의 `diff.external` 을 실행한다
async fn wip_plumbing_submodule_external(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("b"), "empty");
    fs::write(dir.join("sub"), "file\n").unwrap();
    commit_all(&dir);
    // 인덱스에는 파일 `sub`, 작업 트리에는 중첩 저장소(타입 변경)
    fs::remove_file(dir.join("sub")).unwrap();
    let sub = import_at(dir.join("sub"), "empty");
    fs::write(sub.join("f.txt"), "a\n").unwrap();
    commit_all(&sub);
    fs::write(sub.join("f.txt"), "b\n").unwrap();
    let marker = tmp.join("MARK-b");
    assert!(raw_git(&sub).args(["config", "diff.external", &format!("sh -c \"{}\" --", marker_cmd(&marker))]).status().unwrap().success());
    assert!(raw_git(&dir).args(["config", "diff.submodule", "diff"]).status().unwrap().success());
    // 대조군: 예전 porcelain 호출은 중첩 저장소의 diff.external 을 실행한다
    let _ = raw_git(&dir).args(["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3", "--no-renames", "--", "sub"]).output().unwrap();
    assert!(marker.exists(), "대조군: porcelain git diff 가 중첩 저장소의 diff.external 을 실행하지 않았다 — 시험이 의미가 없다");
    fs::remove_file(&marker).unwrap();
    let router = mode.router(&[("b", dir.as_path())]);
    // status 는 이 경로를 `.T` 로 보고하므로 UI 에서 diff 를 요청할 수 있다
    let st = wip_status_of(&router, "b").await;
    assert!(st["data"]["unstaged"].as_array().unwrap().iter().any(|f| f["path"] == "sub"), "{st}");
    let d = wip_diff_of(&router, "b", json!({ "area": "unstaged", "path": "sub" })).await;
    assert_eq!(d["ok"], true, "{d}");
    assert!(!marker.exists(), "wipDiff(unstaged) 가 중첩 저장소의 diff.external 을 실행했다");
}

/// C. porcelain `git diff` 는 stat 만 바뀐 파일이 있으면 index 를 써서 갱신하고 post-index-change 훅을 실행한다(`GIT_OPTIONAL_LOCKS=0` 이어도)
async fn wip_plumbing_index_write(mode: &Mode, tmp: &Path) {
    let dir = import_at(tmp.join("c"), "empty");
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    fs::write(dir.join("b.txt"), "one\n").unwrap();
    commit_all(&dir);
    let marker = tmp.join("MARK-c");
    let hook = dir.join(".git").join("hooks").join("post-index-change");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, format!("#!/bin/sh\n{}\n", marker_cmd(&marker))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // 리눅스는 실행 비트가 없으면 훅을 건너뛴다(대조군이 재현되지 않는다)
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let index = dir.join(".git").join("index");
    // 대조군: stat 만 바뀐 파일(내용·크기 그대로, mtime 만 이동)이 있으면 porcelain diff 가 index 를 쓰고 훅을 실행한다
    dirty(&dir.join("a.txt"), "hello\n", 5);
    let before = fs::read(&index).unwrap();
    let _ = raw_git(&dir).env("GIT_OPTIONAL_LOCKS", "0").args(["diff", "--no-color", "--", "a.txt"]).output().unwrap();
    assert!(marker.exists(), "대조군: porcelain git diff 가 post-index-change 훅을 실행하지 않았다 — 시험이 의미가 없다");
    assert_ne!(fs::read(&index).unwrap(), before, "대조군: porcelain git diff 가 index 를 갱신하지 않았다");
    fs::remove_file(&marker).unwrap();
    // 대조군이 index 를 갱신했으니 다시 stat 만 바뀐 상태로 만든다
    dirty(&dir.join("a.txt"), "hello\n", 9);
    let before = fs::read(&index).unwrap();
    let router = mode.router(&[("c", dir.as_path())]);
    let d = wip_diff_of(&router, "c", json!({ "area": "unstaged", "path": "a.txt" })).await;
    assert_eq!(d["ok"], true, "{d}");
    // 내용이 같은 파일은 UI 에서 빈 diff 로 보인다(hunk 없음)
    assert_eq!((d["data"]["hunks"].clone(), d["data"]["binary"].clone()), (json!([]), json!(false)), "{d}");
    assert_eq!(wip_diff_of(&router, "c", json!({ "area": "staged", "path": "a.txt" })).await["ok"], true);
    assert!(!marker.exists(), "wipDiff 가 post-index-change 훅을 실행했다");
    assert_eq!(fs::read(&index).unwrap(), before, "wipDiff 가 index 를 갱신했다");
    // wipStatus 도 같다
    dirty(&dir.join("b.txt"), "one\n", 12);
    let before = fs::read(&index).unwrap();
    assert_eq!(wip_status_of(&router, "c").await["ok"], true);
    assert!(!marker.exists(), "wipStatus 가 post-index-change 훅을 실행했다");
    assert_eq!(fs::read(&index).unwrap(), before, "wipStatus 가 index 를 갱신했다");
}

/// 출력: 커밋 없는 저장소(SHA-1·SHA-256)의 staged 는 빈 트리와 비교하고, 이름 변경(-M)이 따라간다
async fn wip_plumbing_output(mode: &Mode, tmp: &Path) {
    for (name, extra) in [("unborn", None), ("unborn256", Some("--object-format=sha256"))] {
        let dir = tmp.join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut init = refs_git(&dir);
        init.args(["init", "-q", "-b", "main"]);
        init.args(extra);
        // 이 git 이 sha256 을 지원하지 않으면 건너뛴다
        if !init.status().unwrap().success() {
            assert!(extra.is_some(), "git init");
            continue;
        }
        fs::write(dir.join("n.txt"), "a\nb\n").unwrap();
        assert!(refs_git(&dir).args(["add", "n.txt"]).status().unwrap().success());
        let router = mode.router(&[(name, dir.as_path())]);
        let d = wip_diff_of(&router, name, json!({ "area": "staged", "path": "n.txt" })).await;
        assert_eq!(d["ok"], true, "{name}: {d}");
        assert!(d["data"]["meta"].as_array().unwrap().iter().any(|m| m == "new file mode 100644"), "{name}: {d}");
        let kinds: Vec<&Value> = d["data"]["hunks"][0]["lines"].as_array().unwrap().iter().map(|l| &l["t"]).collect();
        assert_eq!(kinds, [&json!("+"), &json!("+")], "{name}: {d}");
    }
    let dir = import_at(tmp.join("out"), "empty");
    fs::write(dir.join("old name.txt"), "x\ny\nz\nw\n").unwrap();
    commit_all(&dir);
    assert!(refs_git(&dir).args(["mv", "old name.txt", "new name.txt"]).status().unwrap().success());
    fs::write(dir.join("new name.txt"), "x\ny\nZ\nw\n").unwrap();
    assert!(refs_git(&dir).args(["add", "-A"]).status().unwrap().success());
    let router = mode.router(&[("out", dir.as_path())]);
    let d = wip_diff_of(&router, "out", json!({ "area": "staged", "path": "new name.txt", "oldPath": "old name.txt" })).await;
    assert_eq!(d["ok"], true, "{d}");
    assert!(d["data"]["meta"].as_array().unwrap().iter().any(|m| m.as_str().is_some_and(|s| s.starts_with("rename from "))), "{d}");
    assert!(d["data"]["hunks"][0]["lines"].as_array().unwrap().iter().any(|l| l["t"] == "+" && l["text"] == "Z"), "{d}");
}

async fn wip_plumbing_scenarios(mode: &Mode) {
    let tmp = TempDir::new("wipplumbing");
    Box::pin(wip_plumbing_empty_filter(mode, &tmp.0)).await;
    Box::pin(wip_plumbing_submodule_external(mode, &tmp.0)).await;
    Box::pin(wip_plumbing_index_write(mode, &tmp.0)).await;
    Box::pin(wip_plumbing_output(mode, &tmp.0)).await;
    mode.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 작업_트리_diff_는_plumbing_이라_index_를_쓰지_않고_서브모듈_diff_명령을_실행하지_않는다() {
    wip_plumbing_scenarios(&Mode::Local).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 작업_트리_diff_는_ssh_에서도_plumbing_이다() {
    wip_plumbing_scenarios(&Mode::ssh().await).await;
}

/// 드라이버가 하나도 없으면(`git config` 종료 코드 1) 가드는 비어 있다(TS `filterOverrides` 의 "키가 없으면 빈 목록")
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 필터_드라이버가_없으면_가드는_비어_있다() {
    use antler_lib::git::exec::RunOpts;
    use antler_lib::git::wip::filter_overrides;
    let tmp = TempDir::new("wipnofilter");
    let dir = import(&tmp.0, "basic");
    let exec = GitExecutor::Local(LocalExecutor::default());
    let g = filter_overrides(&exec, dir.to_str().unwrap(), &RunOpts::default()).await.expect("가드");
    assert!(g.is_empty());
    // 드라이버가 있으면 4쌍이 서고, `git config` 가 실패하는 저장소(경로 없음)는 오류다
    assert!(raw_git(&dir).args(["config", "filter.x.clean", "cat"]).status().unwrap().success());
    let g = filter_overrides(&exec, dir.to_str().unwrap(), &RunOpts::default()).await.expect("가드");
    assert_eq!(g.args().len(), 8);
    assert!(filter_overrides(&exec, "/nonexistent-antler-wip", &RunOpts::default()).await.is_err());
}

/// git-lfs 의 clean 은 기본 명령 그대로여도 저장소 설정의 `lfs.extension.<이름>.clean` 명령을 실행하고 `.git/lfs/objects` 에 쓴다(`tests/lfsFilterExemption.test.ts`).
/// git-lfs 가 없으면 건너뛴다(표지 명령이 git-lfs 안에서 돌아야 의미가 있다)
fn has_git_lfs() -> bool {
    raw_git(Path::new(".")).args(["lfs", "version"]).output().is_ok_and(|o| o.status.success())
}

/// git-lfs 를 저장소 설정에 직접 연결한 저장소(`git lfs install --local` 과 같은 값). 표지 명령은 표지 파일을 만들고 stdin 을 그대로 내보내는 `sh` 스크립트다
fn lfs_repo(tmp: &Path, name: &str) -> (PathBuf, PathBuf) {
    let dir = import_at(tmp.join(name), "empty");
    for (k, v) in [
        ("core.autocrlf", "false"),
        ("filter.lfs.clean", "git-lfs clean -- %f"),
        ("filter.lfs.smudge", "git-lfs smudge -- %f"),
        ("filter.lfs.process", "git-lfs filter-process"),
        ("filter.lfs.required", "true"),
    ] {
        assert!(raw_git(&dir).args(["config", k, v]).status().unwrap().success());
    }
    (dir, tmp.join(format!("MARK-{name}")))
}

/// 커밋 뒤 `lfs.extension.x.clean` 을 심는다 — 공격은 커밋된 저장소의 설정에만 있다. git-lfs 는 명령을 직접 쪼개 실행하므로 따옴표를 쓰지 않는다(임시 경로에는 공백이 없다)
fn plant_lfs_extension(tmp: &Path, dir: &Path, marker: &Path) {
    let script = tmp.join(format!("mark-{}.sh", marker.file_name().unwrap().to_string_lossy()));
    fs::write(&script, "echo x > \"$1\"\ncat\n").unwrap();
    let fwd = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let _ = fs::remove_dir_all(dir.join(".git").join("lfs"));
    for (k, v) in [
        ("lfs.extension.x.clean", format!("sh {} {}", fwd(&script), fwd(marker))),
        ("lfs.extension.x.smudge", "cat".to_string()),
        ("lfs.extension.x.priority", "0".to_string()),
    ] {
        assert!(raw_git(dir).args(["config", k, &v]).status().unwrap().success());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_lfs_기본_filter_도_작업_트리_채널과_repo_extras_가_실행하지_않는다() {
    if !has_git_lfs() {
        eprintln!("git-lfs 가 없어 건너뜀");
        return;
    }
    let tmp = TempDir::new("wiplfs");
    let attrs = ".gitattributes filter=lfs\n*.bin filter=lfs diff=lfs merge=lfs -text\n";
    let (dir, marker) = lfs_repo(&tmp.0, "attack");
    fs::write(dir.join(".gitattributes"), attrs).unwrap();
    fs::write(dir.join("a.bin"), "payload-1\n").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
        assert!(refs_git(&dir).args(&args).status().unwrap().success());
    }
    plant_lfs_extension(&tmp.0, &dir, &marker);
    // 대조군: 가드 없이 --no-index 를 돌리면 lfs.extension 명령이 실행되고 .git/lfs 가 생긴다
    let _ = raw_git(&dir).args(["diff", "--no-index", "--no-ext-diff", "--no-textconv", "--no-color", "--", "/dev/null", ".gitattributes"]).output().unwrap();
    assert!(marker.exists(), "대조군: 날 --no-index 가 lfs.extension 명령을 실행하지 않았다 — 시험이 의미가 없다");
    assert!(dir.join(".git/lfs/objects").exists(), "대조군: .git/lfs 가 생기지 않았다");
    fs::remove_file(&marker).unwrap();
    plant_lfs_extension(&tmp.0, &dir, &marker);

    let router = Router::new(true, false, mirror_of(&[("a", dir.as_path())]));
    // repoExtras: 표지도 .git/lfs 도 만들지 않고 LFS 판정은 그대로다
    let ex = envelope(router.route("git:repoExtras", &json!(["a"])).await);
    assert_eq!(ex["data"]["lfs"], true, "{ex}");
    assert!(!marker.exists() && !dir.join(".git/lfs").exists(), "repoExtras 가 lfs.extension 명령을 실행했다");
    // wipStatus·wipDiff(unstaged·untracked): 같은 크기로 내용을 바꾸고 mtime 을 옮긴다(해시하려면 clean 이 돈다)
    dirty(&dir.join("a.bin"), "payload-2\n", 5);
    fs::write(dir.join("u.bin"), "new\n").unwrap();
    fs::write(dir.join(".gitattributes"), format!("{attrs}# edit\n")).unwrap();
    let st = wip_status_of(&router, "a").await;
    assert!(st["data"]["unstaged"].as_array().unwrap().iter().any(|f| f["path"] == "a.bin"), "{st}");
    for p in ["a.bin", ".gitattributes"] {
        assert_eq!(wip_diff_of(&router, "a", json!({ "area": "unstaged", "path": p })).await["ok"], true, "{p}");
    }
    assert_eq!(wip_diff_of(&router, "a", json!({ "area": "untracked", "path": "u.bin" })).await["ok"], true);
    assert!(!marker.exists(), "wipStatus·wipDiff 가 lfs.extension 명령을 실행했다");
    assert!(!dir.join(".git/lfs").exists(), "wipStatus·wipDiff 가 .git/lfs 에 썼다");
}

/// 화면 영향(`tests/lfsFilterExemption.test.ts`): clean 이 돌지 않으므로 stat 만 바뀐 LFS 파일은 원본과 비교돼 수정됨으로 보인다 — 알려진 한계
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lfs_저장소의_작업_중_변경은_포인터가_아닌_원본과_비교된다() {
    if !has_git_lfs() {
        eprintln!("git-lfs 가 없어 건너뜀");
        return;
    }
    let tmp = TempDir::new("wiplfsview");
    // LFS 로 커밋한 뒤 파일을 지우고 다시 체크아웃한다 — 체크아웃 직후 수정 없는 LFS 파일
    let checked_out = |name: &str| {
        let (dir, _) = lfs_repo(&tmp.0, name);
        fs::write(dir.join(".gitattributes"), "*.bin filter=lfs diff=lfs merge=lfs -text\n").unwrap();
        fs::write(dir.join("big.bin"), "binary-ish payload\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            assert!(refs_git(&dir).args(&args).status().unwrap().success());
        }
        fs::remove_file(dir.join("big.bin")).unwrap();
        assert!(raw_git(&dir).args(["checkout", "--", "big.bin"]).status().unwrap().success());
        dir
    };
    // 체크아웃 직후 수정 없는 LFS 파일은 stat 캐시 덕분에 수정됨으로 보이지 않는다(파일이 인덱스보다 먼저 쓰인 상태로 만든다 — racy-git 회피)
    let dir = checked_out("clean");
    let f = fs::OpenOptions::new().write(true).open(dir.join("big.bin")).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(10)).unwrap();
    drop(f);
    let _ = raw_git(&dir).args(["status", "--porcelain"]).output().unwrap();
    let router = Router::new(true, false, mirror_of(&[("c", dir.as_path())]));
    let st = wip_status_of(&router, "c").await;
    assert_eq!((st["data"]["staged"].clone(), st["data"]["unstaged"].clone(), st["data"]["untracked"].clone()), (json!([]), json!([]), json!([])), "{st}");
    // stat 만 바뀐 LFS 파일은 수정됨으로 보이고, diff 는 포인터가 아니라 실제 내용이다
    let dir = checked_out("stat");
    let f = fs::OpenOptions::new().write(true).open(dir.join("big.bin")).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(7)).unwrap();
    drop(f);
    let router = Router::new(true, false, mirror_of(&[("s", dir.as_path())]));
    let st = wip_status_of(&router, "s").await;
    let paths: Vec<&str> = st["data"]["unstaged"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["big.bin"], "{st}");
    let d = wip_diff_of(&router, "s", json!({ "area": "unstaged", "path": "big.bin" })).await;
    assert!(d["data"].to_string().contains("binary-ish payload"), "{d}");
}

/// 작업 트리 채널의 링크·짧은 이름 방어(`tests/wip.test.ts` 의 정션·8.3). 링크 확인은 local 실행기만 하므로 골든이 아니라 전용 시험이다. Windows 에서만(정션은 `mklink /J`)
#[cfg(windows)]
struct WipLinks(Vec<PathBuf>);

/// 정션은 대상을 따라가지 않고 링크만 지워야 한다(`rmdir` 은 링크만 지운다) — `TempDir` 보다 먼저 떨어지도록 뒤에 선언한다
#[cfg(windows)]
impl Drop for WipLinks {
    fn drop(&mut self) {
        for l in &self.0 {
            let _ = fs::remove_dir(l);
        }
    }
}

#[cfg(windows)]
#[allow(clippy::disallowed_methods)] // 시험 준비용: std 에 정션 API 가 없다
fn mklink_junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().expect("cmd");
    assert!(out.status.success(), "mklink /J: {}", String::from_utf8_lossy(&out.stdout));
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 정션_아래_경로는_untracked_unstaged_diff_가_모두_거부하고_정상_파일은_열린다() {
    let tmp = TempDir::new("wipjunction");
    let dir = tmp.0.join("jrepo");
    let outside = tmp.0.join("jout");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret\n").unwrap();
    let dir = import_at(dir, "empty");
    fs::write(dir.join("plain.txt"), "p\n").unwrap();
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub").join("ok.txt"), "ok\n").unwrap();
    let links = WipLinks(vec![dir.join("gitj"), dir.join("outj")]);
    mklink_junction(&links.0[0], &dir.join(".git"));
    mklink_junction(&links.0[1], &outside);
    let router = Router::new(true, false, mirror_of(&[("j", dir.as_path())]));
    let st = wip_status_of(&router, "j").await;
    let reported: Vec<&str> = st["data"]["untracked"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    // 대조군: Git for Windows 의 status -uall 은 정션을 따라 들어가 `.git/config`·저장소 밖 파일을 untracked 로 보고한다 — 허용 목록만으로는 막지 못한다
    assert!(reported.contains(&"gitj/config") && reported.contains(&"outj/secret.txt"), "대조군: 정션 아래 경로가 보고되지 않았다 — 시험이 의미가 없다: {reported:?}");
    for p in ["gitj/config", "outj/secret.txt", "gitj\\config"] {
        for area in ["untracked", "unstaged"] {
            let env = wip_diff_of(&router, "j", json!({ "area": area, "path": p })).await;
            assert_eq!((env["ok"].clone(), env["error"].clone()), (json!(false), json!("invalid path")), "{area} {p}: {env}");
        }
    }
    // 정션 옆의 정상 파일·하위 디렉터리 파일은 계속 열린다
    for p in ["plain.txt", "sub/ok.txt"] {
        let env = wip_diff_of(&router, "j", json!({ "area": "untracked", "path": p })).await;
        assert_eq!(env["data"]["hunks"].as_array().map(Vec::len), Some(1), "{p}: {env}");
    }
    drop(links);
}

/// NTFS 8.3 짧은 이름이 실제로 생기는 볼륨에서만 의미가 있다(`dir /x` 로 `.git` 의 짧은 이름을 찾는다 — 볼륨이 끄면 건너뛴다).
/// 허용 목록 때문에 `GIT~1/config` 는 untracked 로 열리지 않는다
#[cfg(windows)]
#[allow(clippy::disallowed_methods)] // 시험 준비용: 8.3 이름을 `dir /x` 로 읽는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 짧은_이름으로_git_config_를_untracked_로_읽을_수_없다() {
    let tmp = TempDir::new("wip83");
    let dir = import_at(tmp.0.join("r83"), "empty");
    let out = std::process::Command::new("cmd").args(["/C", "dir", "/x", "/a:h"]).arg(&dir).output().expect("cmd");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let short = text.lines().find_map(|l| {
        let mut it = l.split_whitespace().rev();
        (it.next()?.eq_ignore_ascii_case(".git")).then(|| it.next().filter(|s| s.contains('~')).map(str::to_string)).flatten()
    });
    let Some(short) = short else {
        eprintln!("이 볼륨은 8.3 짧은 이름을 만들지 않아 건너뜀");
        return;
    };
    fs::write(dir.join("plain.txt"), "p\n").unwrap();
    let router = Router::new(true, false, mirror_of(&[("s", dir.as_path())]));
    let st = wip_status_of(&router, "s").await;
    assert_eq!(st["data"]["untracked"], json!(["plain.txt"]));
    for p in [format!("{short}/config"), format!("{}/config", short.to_lowercase()), format!("{short}\\config")] {
        let env = wip_diff_of(&router, "s", json!({ "area": "untracked", "path": p })).await;
        assert_eq!((env["ok"].clone(), env["error"].clone()), (json!(false), json!("invalid path")), "{p}: {env}");
    }
}

/// 라우팅: 소유 판정은 다른 git 채널과 같은 한 술어(`mirror.resolve`)다 — 미러에 없는 id·github·풀 없는 ssh 와 스위치가 꺼진 라우터는 사이드카, 미러 검사에 걸린 저장소는 ERROR
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 작업_트리_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카() {
    let m = RepoMirror::default();
    m.apply(
        1,
        vec![
            MirrorRepo { id: "a".into(), kind: "local".into(), path: "/nonexistent-antler-wip".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "g".into(), kind: "github".into(), path: "/cache".into(), distro: None, ssh_host: None },
            MirrorRepo { id: "s".into(), kind: "ssh".into(), path: "/srv".into(), distro: None, ssh_host: Some("devbox".into()) },
            MirrorRepo { id: "unc".into(), kind: "local".into(), path: "\\\\evil\\share\\repo".into(), distro: None, ssh_host: None },
        ],
    );
    let router = Router::new(true, false, m.clone());
    let off = Router::new(false, false, m);
    for (ch, tail) in [("git:wipStatus", json!([])), ("git:wipDiff", json!([{ "area": "unstaged", "path": "a.txt" }]))] {
        let call = |id: &Value| {
            let mut v = vec![id.clone()];
            v.extend(tail.as_array().unwrap().iter().cloned());
            Value::Array(v)
        };
        for id in ["nope", "g", "s"] {
            assert!(matches!(router.route(ch, &call(&json!(id))).await, Routed::Sidecar), "{id} {ch}");
        }
        for id in [json!(null), json!(5)] {
            assert!(matches!(router.route(ch, &call(&id)).await, Routed::Sidecar), "{ch} {id}");
        }
        assert!(matches!(router.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
        assert!(matches!(off.route(ch, &call(&json!("a"))).await, Routed::Sidecar), "{ch}");
        let env = envelope(router.route(ch, &call(&json!("unc"))).await);
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}");
    }
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

/// 검증에 걸린 `wipDiff` 는 git 도(저장소 경로가 없는 곳 — 띄웠다면 GIT 오류가 난다) 취소 레지스트리도 건드리지 않고 `rust_handled` 에도 세지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 적대적_wip_diff_요청은_git_을_띄우지_않는다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-wip"))]));
    for (args, want) in [
        (json!(["a"]), "invalid area"),
        (json!(["a", null]), "invalid area"),
        (json!(["a", { UNDEFINED_ARG: true }]), "invalid area"),
        (json!(["a", { "area": "untracked", "path": "x" }]), "invalid path"),
        (json!(["a", { "area": "untracked", "path": ".git/config" }]), "invalid path"),
        (json!(["a", { "area": "unstaged", "path": "GIT~1/config" }]), "invalid path"),
        (json!(["a", { "area": "unstaged", "path": "../x" }]), "invalid path"),
        (json!(["a", { "area": "staged", "path": "a", "oldPath": null }]), "invalid path"),
    ] {
        let env = envelope(router.route("git:wipDiff", &args).await);
        assert_eq!((env["ok"].clone(), env["code"].clone(), env["error"].clone()), (json!(false), json!("ERROR"), json!(want)), "{args}");
    }
    assert_eq!(router.rust_handled(), 0, "거부된 요청은 처리한 것으로 세지 않는다");
    // 검증을 통과한 요청은 git 이 실패한다(경로가 없다)
    let env = envelope(router.route("git:wipDiff", &json!(["a", { "area": "unstaged", "path": "a.txt" }])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    let env = envelope(router.route("git:wipStatus", &json!(["a"])).await);
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    assert_eq!(router.rust_handled(), 2);
}

/// untracked diff 허용 목록의 수명: `wipStatus` 가 채우고, 저장소가 미러에서 빠지거나(저장소 제거) 대상이 바뀌면 지운다(TS `forgetWip`). 앱 종료(`shutdown`)에서도 비운다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn untracked_허용_목록은_저장소가_미러에서_빠지거나_대상이_바뀌면_지워진다() {
    let tmp = TempDir::new("wipallow");
    let dir = import_at(tmp.0.join("r"), "empty");
    let other = import_at(tmp.0.join("other"), "empty");
    fs::write(dir.join("u.txt"), "u\n").unwrap();
    fs::write(other.join("u.txt"), "other\n").unwrap();
    let m = mirror_of(&[("r", dir.as_path())]);
    let router = Router::new(true, false, m.clone());
    let untracked = json!({ "area": "untracked", "path": "u.txt" });
    let denied = |env: &Value| (env["ok"].clone(), env["error"].clone()) == (json!(false), json!("invalid path"));
    // wipStatus 전에는 거부하고, 이후엔 보고된 경로만 연다
    assert!(denied(&wip_diff_of(&router, "r", untracked.clone()).await));
    assert_eq!(wip_status_of(&router, "r").await["data"]["untracked"], json!(["u.txt"]));
    assert_eq!(wip_diff_of(&router, "r", untracked.clone()).await["ok"], true);
    assert!(denied(&wip_diff_of(&router, "r", json!({ "area": "untracked", "path": "GIT~1/config" })).await));
    // 저장소를 지우면(미러에서 빠지면) 목록도 지워진다 — 같은 id 로 다시 추가돼도 wipStatus 전에는 거부다
    assert!(m.apply(2, vec![]));
    router.on_mirror_changed();
    assert!(m.apply(3, vec![MirrorRepo { id: "r".into(), kind: "local".into(), path: dir.to_string_lossy().into_owned(), distro: None, ssh_host: None }]));
    router.on_mirror_changed();
    assert!(denied(&wip_diff_of(&router, "r", untracked.clone()).await), "미러에서 빠졌다 돌아온 저장소에 낡은 허용 목록이 남았다");
    assert_eq!(wip_status_of(&router, "r").await["ok"], true);
    assert_eq!(wip_diff_of(&router, "r", untracked.clone()).await["ok"], true);
    // 같은 id 의 대상(경로)이 바뀌면 `on_mirror_changed` 가 불리기 전에도 낡은 목록을 쓰지 않는다(다른 저장소의 보고를 새 저장소에 적용하지 않는다)
    assert!(m.apply(4, vec![MirrorRepo { id: "r".into(), kind: "local".into(), path: other.to_string_lossy().into_owned(), distro: None, ssh_host: None }]));
    assert!(denied(&wip_diff_of(&router, "r", untracked.clone()).await), "대상이 바뀐 저장소에 낡은 허용 목록을 썼다");
    assert_eq!(wip_status_of(&router, "r").await["ok"], true);
    assert!(wip_diff_of(&router, "r", untracked.clone()).await["data"].to_string().contains("other"));
    // 종료하면 비운다
    router.shutdown();
    assert!(denied(&wip_diff_of(&router, "r", untracked).await));
}

/// 실패한 `wipStatus` 는 허용 목록을 바꾸지 않는다(TS 는 성공했을 때만 `untrackedAllow.set`) — 이전 보고가 그대로다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 실패한_wip_status_는_허용_목록을_바꾸지_않는다() {
    let tmp = TempDir::new("wipallowfail");
    let dir = import_at(tmp.0.join("r"), "empty");
    fs::write(dir.join("u.txt"), "u\n").unwrap();
    let router = Router::new(true, false, mirror_of(&[("r", dir.as_path())]));
    assert_eq!(wip_status_of(&router, "r").await["ok"], true);
    // 이름에 `=` 가 든 드라이버를 심으면 다음 wipStatus 는 거부된다(GIT) — 이전 목록은 남지만 wipDiff 자체도 같은 가드에 막힌다
    assert!(raw_git(&dir).args(["config", "filter.a=b.clean", "cat"]).status().unwrap().success());
    fs::write(dir.join("v.txt"), "v\n").unwrap();
    let env = wip_status_of(&router, "r").await;
    assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("GIT")), "{env}");
    // 가드를 풀면 이전(성공한) 보고 u.txt 만 열리고 v.txt 는 새 wipStatus 전까지 거부다
    assert!(raw_git(&dir).args(["config", "--unset", "filter.a=b.clean"]).status().unwrap().success());
    assert_eq!(wip_diff_of(&router, "r", json!({ "area": "untracked", "path": "u.txt" })).await["ok"], true);
    let env = wip_diff_of(&router, "r", json!({ "area": "untracked", "path": "v.txt" })).await;
    assert_eq!((env["ok"].clone(), env["error"].clone()), (json!(false), json!("invalid path")), "{env}");
}

/// `wipStatus` 는 첫 로그 페이지 문(`repos:watch` 가 닫는다)이 열릴 때까지 기다린다(TS `afterFirstPage`). `wipDiff` 는 기다리지 않는다
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wip_status_는_첫_페이지_문이_열릴_때까지_기다리고_wip_diff_는_기다리지_않는다() {
    let tmp = TempDir::new("wipgate");
    let dir = import_at(tmp.0.join("r"), "empty");
    fs::write(dir.join("u.txt"), "u\n").unwrap();
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    assert_eq!(wip_status_of(&router, "b").await["ok"], true, "문이 없는 저장소는 바로 간다");
    assert!(matches!(router.route("repos:watch", &json!(["b"])).await, Routed::Sidecar));
    assert!(router.repo_state().is_armed("b"));
    assert_eq!(wip_diff_of(&router, "b", json!({ "area": "untracked", "path": "u.txt" })).await["ok"], true, "diff 는 문과 무관하다(이미 보고된 경로)");
    let r = router.clone();
    let pending = tokio::spawn(async move { envelope(r.route("git:wipStatus", &json!(["b"])).await) });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!pending.is_finished(), "문이 닫혀 있는 동안 wipStatus 가 끝났다");
    router.note_sink()(NoteOp::FirstPage, "b");
    let env = tokio::time::timeout(std::time::Duration::from_secs(60), pending).await.expect("문이 열리면 끝나야 한다").unwrap();
    assert_eq!(env["ok"], true, "{env}");
}

/// 같은 저장소의 진행 중 `wipStatus` 는 하나로 합쳐진다 — 동시에 보내도 결과가 같고 서로 막지 않는다. 한쪽 취소가 다른 쪽을 죽이지 않는다(모두 떠났을 때만 프로세스에 닿는다)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 같은_저장소의_wip_status_는_합쳐지고_한쪽_취소가_다른_쪽을_죽이지_않는다() {
    let tmp = TempDir::new("wipjoin");
    let dir = import_at(tmp.0.join("r"), "empty");
    fs::write(dir.join("u.txt"), "u\n").unwrap();
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    let calls: Vec<_> = (0..4)
        .map(|i| {
            let r = router.clone();
            tokio::spawn(async move { envelope(r.route("git:wipStatus", &json!(["b", format!("join{i}")])).await) })
        })
        .collect();
    // 첫 호출이 등록된 뒤 하나를 취소한다
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    assert!(matches!(router.route("req:cancel", &json!(["join1"])).await, Routed::Sidecar));
    for (i, c) in calls.into_iter().enumerate() {
        let env = tokio::time::timeout(std::time::Duration::from_secs(60), c).await.expect("끝나야 한다").unwrap();
        if i == 1 {
            // 타이밍에 따라 이미 끝났을 수도 있다
            assert!(env["ok"] == true || env["code"] == "ABORTED", "{env}");
        } else {
            assert_eq!((env["ok"].clone(), env["data"]["untracked"].clone()), (json!(true), json!(["u.txt"])), "호출 {i}: {env}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 등록_전에_온_cancel_은_작업_트리_요청을_git_없이_취소한다() {
    let router = Router::new(true, false, mirror_of(&[("a", Path::new("/nonexistent-antler-wip"))]));
    for (i, (ch, args)) in [("git:wipStatus", json!(["a"])), ("git:wipDiff", json!(["a", { "area": "unstaged", "path": "a.txt" }]))].into_iter().enumerate() {
        let id = format!("early{i}");
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        // git 을 띄웠다면 경로가 없어 GIT 이 난다 — ABORTED 면 띄우지 않은 것이다
        let env = envelope(router.route(ch, &Value::Array(call)).await);
        assert_eq!(env["code"], "ABORTED", "{ch}: {env}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 진행_중_작업_트리_요청을_취소해도_멈추지_않는다() {
    let tmp = TempDir::new("wipcancel");
    let dir = import_at(tmp.0.join("r"), "empty");
    fs::create_dir_all(dir.join("bulk")).unwrap();
    for k in 0..3_000 {
        fs::write(dir.join("bulk").join(format!("f{k}")), format!("{k}\n")).unwrap();
    }
    // 8MB 를 넘는 untracked 파일(diff 가 오래 걸리도록)
    fs::write(dir.join("big.txt"), "line of text\n".repeat(900_000)).unwrap();
    let router = Arc::new(Router::new(true, false, mirror_of(&[("b", dir.as_path())])));
    assert_eq!(wip_status_of(&router, "b").await["ok"], true);
    for (i, (ch, args)) in [("git:wipStatus", json!(["b"])), ("git:wipDiff", json!(["b", { "area": "untracked", "path": "big.txt", "full": true }]))].into_iter().enumerate() {
        let id = format!("slow{i}");
        let mut call = args.as_array().unwrap().clone();
        call.push(json!(id));
        let r = router.clone();
        let task = tokio::spawn(async move { envelope(r.route(ch, &Value::Array(call)).await) });
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
        let env = tokio::time::timeout(std::time::Duration::from_secs(60), task).await.expect("취소 뒤에도 끝나야 한다").unwrap();
        // 타이밍에 따라 이미 끝났을 수도 있다 — 어느 쪽이든 형식이 맞아야 한다
        match env["ok"].as_bool() {
            Some(true) => {}
            _ => assert_eq!(env["code"], "ABORTED", "{ch}: {env}"),
        }
    }
    // 취소한 뒤에도 같은 요청을 다시 받을 수 있다(슬롯·합치기 표가 풀렸다)
    assert_eq!(wip_status_of(&router, "b").await["data"]["counts"]["untracked"], 3_001);
    assert_eq!(wip_diff_of(&router, "b", json!({ "area": "untracked", "path": "bulk/f7" })).await["data"]["hunks"].as_array().map(Vec::len), Some(1));
}

/// 소스 검사: ① git `-c` 인자(`"-c…` 리터럴)를 만드는 제품 코드는 `wip.rs`(저장소가 정한 값이 들어가는 유일한 곳)와 상수만 쓰는 `exec/common.rs`·`fetch/guard.rs` 뿐이다.
/// ② `FilterGuard::none()` 의 제품 코드 사용은 `wip.rs` 뿐이다(가드를 건너뛰는 길이 없다). ③ wip 의 argv 빌더는 `&FilterGuard` 를 필수 인자로 받고, `--no-index` 는 한 곳에만 있으며,
/// 파일·status 를 읽는 호출은 `filter_overrides` 로 만든 가드를 쓴다. 가드를 읽는 곳(`FilterGuard::args`)은 `wip.rs` 의 `guarded` 와 `lfs.rs` 의 `no_index_args` 둘뿐이다
#[test]
fn c_인자는_wip_밖에서_만들지_않고_작업_트리_읽기는_가드를_필수로_받는다() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    let mut dash_c = Vec::new();
    let mut none_users = Vec::new();
    let mut args_readers = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(f).unwrap().replace("\r\n", "\n");
        // 파일 끝의 시험 모듈은 제외한다(적대적 입력 `"-c"` 등이 든다)
        let body = text.split("\n#[cfg(test)]\nmod ").next().unwrap();
        // jira 의 `orderBy=-created` 같은 git 과 무관한 문자열은 git 모듈이 아니다
        if !rel.starts_with("jira/") && !rel.starts_with("github/") && body.contains("\"-c") {
            dash_c.push(rel.clone());
        }
        if body.contains("FilterGuard::none()") {
            none_users.push(rel.clone());
        }
        if body.contains("guard.args()") {
            args_readers.push(rel.clone());
        }
    }
    dash_c.sort();
    assert_eq!(dash_c, ["git/exec/common.rs", "git/fetch/guard.rs", "git/wip.rs"], "`-c` 인자를 만드는 제품 코드가 늘었다 — 저장소가 정한 값이 전역 옵션 자리에 들어가는 곳은 wip.rs 뿐이어야 한다");
    assert_eq!(none_users, ["git/wip.rs"], "FilterGuard::none() 을 wip.rs 밖에서 쓰면 가드를 건너뛴다");
    args_readers.sort();
    assert_eq!(args_readers, ["git/lfs.rs", "git/wip.rs"]);

    let wip = fs::read_to_string(src.join("git/wip.rs")).unwrap().replace("\r\n", "\n");
    let body = wip.split("\n#[cfg(test)]\nmod ").next().unwrap();
    assert!(body.contains("pub fn wip_status_args(guard: &FilterGuard)"), "wip_status_args 는 가드를 필수 인자로 받아야 한다");
    assert!(body.contains("pub fn wip_diff_args(guard: &FilterGuard, req: &WipDiffRequest, staged_base: &str)"), "wip_diff_args 는 가드를 필수 인자로 받아야 한다");
    assert_eq!(body.matches("WIP_STATUS_ARGS").count(), 2, "status 인자 상수는 import 하나와 wip_status_args 한 곳에서만 쓴다");
    assert_eq!(body.matches("\"--no-index\"").count(), 1, "--no-index 인자는 wip_diff_args 한 곳에만 있다");
    assert_eq!(body.matches("\"--cached\"").count(), 1);
    assert_eq!(body.matches("guard.args().to_vec()").count(), 1, "가드의 -c 를 argv 에 붙이는 곳은 guarded 하나다");
    // git 을 부르는 읽기(pump)는 두 곳이고, 각각 filter_overrides 로 만든 가드를 만든 argv 로만 부른다
    assert_eq!(body.matches("pump(exec,").count(), 2);
    assert_eq!(body.matches("&wip_status_args(&guard)").count(), 1);
    assert_eq!(body.matches("&wip_diff_args(&guard, req, &base)").count(), 1);
    assert_eq!(body.matches("filter_overrides(exec, repo_path, opts).await?").count(), 2, "status·diff 모두 가드를 조회한다");
    // 가드 조회가 diff 의 git 호출보다 앞선다
    let diff_fn = body.split("pub async fn wip_file_diff").nth(1).expect("wip_file_diff");
    assert!(diff_fn.find("filter_overrides(").unwrap() < diff_fn.find("pump(exec,").unwrap());
    assert!(diff_fn.find("assert_no_link_escape(").unwrap() < diff_fn.find("filter_overrides(").unwrap(), "링크 확인이 열기 전(가드 조회 앞)이다");
    // 라우터는 인자를 만들지 않는다
    let router = fs::read_to_string(src.join("router.rs")).unwrap().replace("\r\n", "\n");
    let rbody = router.split("\n#[cfg(test)]\nmod ").next().unwrap();
    assert!(!rbody.contains("wip_status_args(") && !rbody.contains("wip_diff_args("));
}

// ── git:fetch ──────────────────────────────────────────

/// 확인 창 응답(`reply`)·확인 중 실행할 git(`during`)을 정하고, 확인 횟수·변경 알림·승인 요청을 세는 호스트
#[derive(Default)]
struct RigState {
    confirms: AtomicUsize,
    changed: AtomicUsize,
    approvals: Mutex<Vec<String>>,
    reply: Mutex<Option<i64>>,
    during: Mutex<Option<(PathBuf, Vec<String>)>>,
}

#[derive(Clone, Default)]
struct RigHost(Arc<RigState>);

impl FetchHost for RigHost {
    fn emit(&self, event: HostEvent) {
        if matches!(event, HostEvent::RepoChanged(_)) {
            self.0.changed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn confirm(&self, _repo_id: &str, _confirm: &ConfirmBox) -> impl Future<Output = Option<i64>> + Send {
        let state = self.0.clone();
        async move {
            state.confirms.fetch_add(1, Ordering::SeqCst);
            // 확인 창이 떠 있는 동안 저장소 설정이 바뀌는 경우(검사와 실행 사이)
            if let Some((dir, args)) = state.during.lock().unwrap().clone() {
                assert!(raw_git(&dir).args(&args).status().unwrap().success(), "during: {args:?}");
            }
            *state.reply.lock().unwrap()
        }
    }

    fn approve(&self, kind: ApprovalKind, _repo_id: &str, _fingerprint: &str) {
        self.0.approvals.lock().unwrap().push(kind.as_str().to_string());
    }

    fn note(&self, _op: NoteOp, _repo_id: &str) {}
}

/// 401 로만 답하는 루프백 서버(연결마다 스레드 — git 이 미리 연결만 열어 두는 경우에 막히지 않게). 끝나면 `stop` 으로 멈춘다
fn auth_server() -> (u16, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    const RESPONSE: &[u8] = b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
                let mut buf = [0u8; 4096];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(RESPONSE);
            });
        }
    });
    (port, stop, handle)
}

/// TS 골든(`fetchState`)과 같은 규칙: 시각은 유무만, 오류 문구는 유무와 file 전송 안내 여부만
fn fetch_state(data: &Value) -> Value {
    json!({
        "fetching": data["fetching"],
        "lastOk": !data["lastOkAt"].is_null(),
        "error": !data["error"].is_null(),
        "fileHint": data["error"].as_str().is_some_and(|e| e.starts_with("자동 fetch 는 로컬 경로(file) remote")),
        "blocked": data["blocked"],
        "authPaused": data["authPaused"],
        "needsManual": data["needsManual"],
        "noRemote": data["noRemote"],
    })
}

/// 시나리오의 git 단계(`{in, git}`)를 실행한다. `{port}` 는 401 서버 포트로 바꾼다
fn run_step_git(step: &Value, origin: &Path, repo: &Path, port: u16) {
    let dir = if step["in"] == "origin" { origin } else { repo };
    let args: Vec<String> = step["git"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().replace("{port}", &port.to_string())).collect();
    assert!(raw_git(dir).args(&args).status().unwrap().success(), "git {args:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_fetch_시나리오가_ts_골든과_같다() {
    fetch_scenarios(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_fetch_ssh_시나리오가_ts_골든과_같다() {
    fetch_scenarios(true).await;
}

/// target:'ssh' 시나리오의 환경: 루프백 SSH 서버·임시 홈·풀. `anomaly` 는 `refused`(닫힌 포트) 또는 `hostkey`(known_hosts 와 다른 호스트 키)다.
/// 이 서버 너머의 저장소가 원격 저장소다(서버가 같은 PC 의 git 으로 돌린다). 임시 홈·`AgentSetting::Disabled` 만 쓴다
struct FetchSshRig {
    server: Option<ssh_server::TestServer>,
    pool: Arc<SshPool>,
    _home: Arc<ssh_common::Home>,
}

async fn fetch_ssh_rig(anomaly: Option<&str>) -> FetchSshRig {
    let client = ssh_server::new_key();
    let home = Arc::new(ssh_common::Home::new(&client));
    let (server, port, known) = match anomaly {
        // 닫힌 포트: 잠깐 열었다 닫아 빈 포트를 얻는다(서버는 없다)
        Some("refused") => {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = l.local_addr().unwrap().port();
            drop(l);
            (None, port, ssh_server::new_key().public_key().clone())
        }
        other => {
            let s = ssh_server::TestServer::start(ssh_server::Opts::new(vec![client.public_key().clone()])).await;
            let known = if other == Some("hostkey") { ssh_server::new_key().public_key().clone() } else { s.host_key.clone() };
            let port = s.port;
            (Some(s), port, known)
        }
    };
    home.write_config(&format!("Host test
  HostName 127.0.0.1
  Port {port}
  User tester
  IdentityFile {}
  IdentitiesOnly yes
", home.key_for_config()));
    home.write_known_hosts(&ssh_common::known_hosts_line(port, &known));
    let h = home.clone();
    FetchSshRig { server, pool: SshPool::new(move |host| h.executor(host)), _home: home }
}

/// `ssh` 가 거짓이면 target 이 없는(local) 시나리오를, 참이면 target:'ssh' 시나리오를 TS 골든과 비교한다. 둘 다 비어 있으면 안 된다(조용히 건너뛰지 않는다)
async fn fetch_scenarios(ssh: bool) {
    // local·ssh 시나리오는 한 번에 하나만 돈다 — 둘이 겹치면 git 프로세스가 몰려 fetch 의 60초 제한(요청 시점부터)에 걸릴 수 있다(다른 무거운 시험과도 겹치는 러너에서 한 번 관측)
    static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _gate = GATE.lock().await;
    let scenarios = read_json("tests/fixtures/repos/fetch.scenarios.json");
    let golden = read_json("tests/golden/channels/fetch.json");
    let scenarios = scenarios["scenarios"].as_array().expect("scenarios");
    let golden = golden["scenarios"].as_array().expect("golden scenarios");
    assert_eq!(scenarios.len(), golden.len(), "시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts 로 골든을 다시 만든다");
    let picked: Vec<(usize, &Value, &Value)> = scenarios.iter().zip(golden).enumerate().filter(|(_, (sc, _))| (sc.get("target") == Some(&json!("ssh"))) == ssh).map(|(n, (sc, g))| (n, sc, g)).collect();
    assert!(!picked.is_empty(), "{} 시나리오가 하나도 없다", if ssh { "ssh" } else { "local" });

    let tmp = TempDir::new("fetchgold");
    let mut diffs = Vec::new();
    for (n, sc, g) in picked {
        let name = sc["name"].as_str().unwrap();
        assert_eq!(g["name"], sc["name"], "{name}: 골든의 시나리오 순서가 다르다");
        let dir = tmp.0.join(format!("s{n}"));
        fs::create_dir_all(&dir).unwrap();
        // 원격(basic.fi) 과 그 복제본(경로 URL — file 전송)
        let origin = import(&dir, "basic");
        let repo = dir.join("repo");
        assert!(raw_git(&dir).args(["clone", "-q"]).arg(&origin).arg(&repo).status().unwrap().success(), "clone");
        let server = (sc.get("server") == Some(&json!("auth401"))).then(auth_server);
        let port = server.as_ref().map_or(0, |s| s.0);

        let ssh_rig = if ssh { Some(fetch_ssh_rig(sc.get("ssh").and_then(Value::as_str)).await) } else { None };
        let mirror = match &ssh_rig {
            None => mirror_of(&[("r", repo.as_path())]),
            Some(_) => {
                let m = RepoMirror::default();
                m.apply(1, vec![MirrorRepo { id: "r".into(), kind: "ssh".into(), path: repo.to_string_lossy().into_owned(), distro: None, ssh_host: Some("test".into()) }]);
                m
            }
        };
        let router = Router::new(true, false, mirror.clone());
        let host = RigHost::default();
        let pool = ssh_rig.as_ref().map(|r| r.pool.clone());
        if let Some(pool) = &pool {
            router.set_ssh_pool(pool.clone());
            assert!(router.ssh_active());
        }
        let make_runner = move |t: &antler_lib::mirror::Target| match (t, &pool) {
            (antler_lib::mirror::Target::Ssh { host, .. }, Some(pool)) => Arc::new(GitExecutor::Ssh(pool.get(host))),
            _ => Arc::new(GitExecutor::Local(LocalExecutor::default())),
        };
        router.set_fetch(Arc::new(FetchService::new(host.clone(), mirror, make_runner, Some(dir.join("no-hooks").to_string_lossy().into_owned()))));

        let mut got = Vec::new();
        for step in sc["steps"].as_array().unwrap() {
            if step.get("git").is_some() {
                run_step_git(step, &origin, &repo, port);
                continue;
            }
            let f = &step["fetch"];
            *host.0.reply.lock().unwrap() = match f.get("confirm") {
                None => Some(2),
                Some(Value::Null) => None,
                Some(v) => v.as_i64(),
            };
            *host.0.during.lock().unwrap() = f.get("during").map(|d| {
                let args = d["git"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
                (if d["in"] == "origin" { origin.clone() } else { repo.clone() }, args)
            });
            host.0.confirms.store(0, Ordering::SeqCst);
            host.0.changed.store(0, Ordering::SeqCst);
            host.0.approvals.lock().unwrap().clear();
            // manual 은 renderer 가 보낸 값 그대로(없으면 빈 자리 표식)다 — 라우터가 정확히 true 만 참으로 읽는다
            let manual = f.get("manual").cloned().unwrap_or_else(|| json!({ antler_lib::router::UNDEFINED_ARG: true }));
            let env = envelope(router.route("git:fetch", &json!(["r", manual])).await);
            assert_eq!(env["ok"], true, "{name}: {env}");
            got.push(json!({
                "manual": f.get("manual").cloned().unwrap_or(Value::Null),
                "confirm": f.get("confirm").cloned().unwrap_or(Value::Null),
                "state": fetch_state(&env["data"]),
                "confirms": host.0.confirms.load(Ordering::SeqCst),
                "changed": host.0.changed.load(Ordering::SeqCst),
                "approvals": *host.0.approvals.lock().unwrap(),
            }));
        }
        router.shutdown();
        if let Some(server) = ssh_rig.as_ref().and_then(|r| r.server.as_ref()) {
            assert!(server.connections() <= 1, "{name}: 풀이 호스트당 연결 하나를 나눠 쓴다(연결 {})", server.connections());
        }
        if let Some((port, stop, handle)) = server {
            stop.store(true, Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(("127.0.0.1", port));
            handle.join().unwrap();
        }
        if Value::Array(got.clone()) != g["steps"] {
            diffs.push(format!("{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&Value::Array(got)), short(&g["steps"])));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}
