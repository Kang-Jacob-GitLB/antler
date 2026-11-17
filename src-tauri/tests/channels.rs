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

use antler_lib::git::exec::{GitExecutor, LocalExecutor};
use antler_lib::git::fetch::confirm_text::ConfirmBox;
use antler_lib::git::fetch::service::{ApprovalKind, FetchHost, FetchService, HostEvent};
use antler_lib::git::log_service::NoteOp;
use antler_lib::git::scope::{log_args, parse_scope};
use antler_lib::mirror::{MirrorRepo, RepoMirror};
use antler_lib::router::{Routed, Router, RUST_CHANNELS};
use serde_json::{json, Value};

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
    let dir = root.join(repo);
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

fn mirror_of(entries: &[(&str, &Path)]) -> RepoMirror {
    let m = RepoMirror::default();
    let repos = entries
        .iter()
        .map(|(id, p)| MirrorRepo { id: (*id).to_string(), kind: "local".into(), path: p.to_string_lossy().into_owned(), distro: None })
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
    let router = Router::new(true, false, mirror_of(&entries));

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
    assert!(matches!(router.route("git:refs", &json!(["basic"])).await, Routed::Sidecar));
    let off = Router::new(false, false, mirror_of(&[("basic", dir.as_path())]));
    assert!(matches!(off.route("git:commit", &json!(["basic", "abcd1234"])).await, Routed::Sidecar));
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}

// ── git:log ────────────────────────────────────────────

/// 시나리오 값의 '@rev:<식>'·$gen·$str·$upper·$cat 을 푼다(`tests/goldenChannels.test.ts` 의 `expand` 와 같은 규칙)
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
        let router = Router::new(true, false, mirror_of(&[(id.as_str(), dir.as_path())]));
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
    let all = read_json("tests/fixtures/repos/files.scenarios.json");
    let tmp = TempDir::new("files");
    let dirs: Vec<(String, PathBuf)> = ["files", "empty"].iter().map(|n| ((*n).to_string(), import(&tmp.0, n))).collect();
    let entries: Vec<(&str, &Path)> = dirs.iter().map(|(n, d)| (n.as_str(), d.as_path())).collect();
    let router = Router::new(true, false, mirror_of(&entries));
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
    let scenarios = read_json("tests/fixtures/repos/fetch.scenarios.json");
    let golden = read_json("tests/golden/channels/fetch.json");
    let scenarios = scenarios["scenarios"].as_array().expect("scenarios");
    let golden = golden["scenarios"].as_array().expect("golden scenarios");
    assert_eq!(scenarios.len(), golden.len(), "시나리오 수가 골든과 다르다 — UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts 로 골든을 다시 만든다");

    let tmp = TempDir::new("fetchgold");
    let mut diffs = Vec::new();
    for (n, (sc, g)) in scenarios.iter().zip(golden).enumerate() {
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

        let mirror = mirror_of(&[("r", repo.as_path())]);
        let router = Router::new(true, false, mirror.clone());
        let host = RigHost::default();
        let make_runner = |_: &antler_lib::mirror::Target| Arc::new(GitExecutor::Local(LocalExecutor::default()));
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
