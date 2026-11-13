//! 채널 패리티 통합 테스트. TS(`tests/goldenChannels.test.ts`)가 같은 fixture(`tests/fixtures/repos/*.fi`)·시나리오로 만든 골든
//! (`tests/golden/channels/commit.json`)을 Rust 라우터의 Rust 경로(`Router::route`)가 그대로 재현하는지 본다. 오류는 code 만 비교한다.
//! 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts`.
//!
//! 격리: `tests/exec.rs` 의 `isolate` 와 같다 — 이 테스트 바이너리가 시작될 때 `GIT_CONFIG_GLOBAL`(커밋된 `src-tauri/tests/gitconfig`)·
//! `GIT_CONFIG_NOSYSTEM`·`GCM_INTERACTIVE` 를 프로세스 환경에 넣어 사용자의 실제 git 설정을 읽지 않는다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use antler_lib::mirror::{MirrorRepo, RepoMirror};
use antler_lib::router::{Routed, Router, RUST_CHANNELS};
use serde_json::{json, Value};

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
    assert!(matches!(router.route("git:log", &json!(["basic", {}])).await, Routed::Sidecar));
    let off = Router::new(false, false, mirror_of(&[("basic", dir.as_path())]));
    assert!(matches!(off.route("git:commit", &json!(["basic", "abcd1234"])).await, Routed::Sidecar));
    assert_eq!(router.rust_handled() + off.rust_handled(), 0);
}
