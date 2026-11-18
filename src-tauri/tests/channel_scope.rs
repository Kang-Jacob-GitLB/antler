//! 채널 범위 불변식(Phase 6c PR 5 — 옛 `sidecar_scope`): **IPC 채널 전부를 Rust 가 처리한다.** `IPC_CHANNELS` 의 합집합이 Rust 처리 목록 일곱 개와 정확히 같고(목록끼리 겹치지 않는다),
//! 목록에 없는 채널·Rust 가 풀지 못한 호출은 어디로도 가지 않고 명시적 오류 봉투(`Router::invoke`)가 된다. Node 사이드카와 되돌리기 스위치(`ANTLER_RUST_*`)의 흔적이 없다는 소스 검사도 여기 있다.
//! 파일을 쓰지 않는다: 채널 호출은 입력 검증에 걸리거나 없는 저장소라 설정 변경에 닿지 않는다(설정 파일 경로는 임시 폴더이고 만들어지지 않는다).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use antler_lib::config::ConfigStore;
use antler_lib::config_host::ConfigHost;
use antler_lib::env_host::EnvHost;
use antler_lib::external::opener::{LaunchRequest, OpenerDeps, Stat};
use antler_lib::external::plan::Platform;
use antler_lib::git::exec::{GitExecutor, LocalExecutor};
use antler_lib::mirror::{MirrorRepo, RepoMirror};
use antler_lib::router::{Routed, Router, API_CHANNELS, APP_CHANNELS, CONFIG_CHANNELS, ENV_CHANNELS, ROUTER_CHANNELS, RUST_CHANNELS, UNDEFINED_ARG, UPDATE_CHANNELS};
use futures_util::future::FutureExt;
use serde_json::{json, Value};

// unsafe: main 이전 코드는 Rust 런타임 초기화 전에 돈다 — 여기서는 환경 변수만 넣고 다른 것에 기대지 않는다
#[ctor::ctor(unsafe)]
fn isolate() {
    std::env::set_var("GIT_CONFIG_GLOBAL", Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("gitconfig"));
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    std::env::set_var("GCM_INTERACTIVE", "never");
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

/// 아무것도 실행하지 않는 주입물(이 시험의 호출은 프로그램을 띄우지 않는다)
struct NoDeps;

impl OpenerDeps for NoDeps {
    fn platform(&self) -> Platform {
        Platform::current()
    }
    fn env(&self, _name: &str) -> Option<String> {
        None
    }
    fn realpath(&self, _p: &str) -> Option<String> {
        None
    }
    fn stat(&self, _p: &str) -> Option<Stat> {
        None
    }
    fn lstat(&self, _p: &str) -> Option<Stat> {
        None
    }
    fn spawn(&self, _req: &LaunchRequest) -> Result<(), String> {
        panic!("프로그램을 띄우면 안 된다")
    }
    fn open_path(&self, _p: &str) -> Result<(), String> {
        panic!("열면 안 된다")
    }
    fn show_item(&self, _p: &str) -> Result<(), String> {
        panic!("보여 주면 안 된다")
    }
}

/// 임시 폴더(Windows 에서 핸들이 남을 수 있어 정리는 실패해도 무시한다)
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("antler-channel-scope-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mirror_with(kinds: &[(&str, &str)]) -> RepoMirror {
    let mirror = RepoMirror::default();
    let repos = kinds
        .iter()
        .map(|(id, kind)| MirrorRepo {
            id: (*id).to_string(),
            kind: (*kind).to_string(),
            path: if *kind == "ssh" || *kind == "wsl" { "/srv/repo".to_string() } else { "C:\\work\\repo".to_string() },
            distro: (*kind == "wsl").then(|| "Ubuntu".to_string()),
            ssh_host: (*kind == "ssh").then(|| "example-host".to_string()),
        })
        .collect();
    mirror.apply(1, repos);
    mirror
}

/// 설정 소유자만 단 라우터: 앱·환경·jira·github·fetch·ssh·update 포트는 달지 않는다(새는 길은 "포트 없음" 갈래에 있다)
fn owned(tmp: &Tmp, mirror: RepoMirror) -> Router {
    let router = Router::new(false, mirror);
    let store = Arc::new(ConfigStore::open_for_test(tmp.0.join("antler.config.json"), None, || "id-1".to_string(), || 0));
    let env = EnvHost::new(Arc::new(|| async { Vec::new() }.boxed()), Arc::new(Vec::new), Arc::new(|_| false), Arc::new(|_, _| GitExecutor::Local(LocalExecutor::default())));
    let host = Arc::new(ConfigHost::new(store, env, String::new(), Arc::new(|| Ok(None)), Arc::new(NoDeps)));
    router.set_config(host);
    router
}

/// 인자 모양 표: 정상(없는 id)·빈 배열·배열 아님·null·숫자·표식·객체
fn arg_shapes() -> Vec<Value> {
    vec![json!(["nope"]), json!([]), json!("nope"), Value::Null, json!([123]), json!([{ UNDEFINED_ARG: true }]), json!([{ "kind": "local" }]), json!({ "a": 1 })]
}

fn is_unserved(r: &Routed) -> bool {
    matches!(r, Routed::Unserved)
}

/// 처리 목록 일곱 개(라우터의 `route` 가 가르는 갈래 전부)
fn handled_lists() -> [(&'static str, &'static [&'static str]); 7] {
    [
        ("RUST_CHANNELS", RUST_CHANNELS),
        ("API_CHANNELS", API_CHANNELS),
        ("APP_CHANNELS", APP_CHANNELS),
        ("ENV_CHANNELS", ENV_CHANNELS),
        ("CONFIG_CHANNELS", CONFIG_CHANNELS),
        ("ROUTER_CHANNELS", ROUTER_CHANNELS),
        ("UPDATE_CHANNELS", UPDATE_CHANNELS),
    ]
}

#[test]
fn update_채널_7개는_ipc_채널의_update_접두_채널과_정확히_같다() {
    let mut want: Vec<String> = ipc_channels().into_iter().filter(|c| c.starts_with("update:")).collect();
    let mut have: Vec<String> = UPDATE_CHANNELS.iter().map(|c| (*c).to_string()).collect();
    want.sort();
    have.sort();
    assert_eq!(have, want);
    assert_eq!(UPDATE_CHANNELS.len(), 7);
}

/// **채널 합집합 강제**: `IPC_CHANNELS`(= `tests/golden/exec/channels.json`)의 모든 채널이 Rust 처리 목록 정확히 하나에 있고, 목록에는 IPC 에 없는 채널이 없다.
/// 새 채널을 IPC 에 더하고 처리 갈래를 빠뜨리면(또는 한 채널이 두 갈래에 걸치면) 여기서 먼저 잡는다
#[test]
fn ipc_채널의_합집합은_rust_처리_목록_일곱_개와_정확히_같고_목록끼리_겹치지_않는다() {
    let ipc = ipc_channels();
    let mut owner: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for (name, list) in handled_lists() {
        for ch in list {
            if let Some(prev) = owner.insert(ch, name) {
                panic!("{ch} 가 {prev} 와 {name} 두 곳에 있다");
            }
        }
    }
    for ch in &ipc {
        assert!(owner.contains_key(ch.as_str()), "{ch} 는 어떤 Rust 처리 목록에도 없다 — 아무도 처리하지 않는다");
    }
    let mut ipc_sorted: Vec<&str> = ipc.iter().map(String::as_str).collect();
    ipc_sorted.sort_unstable();
    ipc_sorted.dedup();
    assert_eq!(ipc_sorted.len(), ipc.len(), "IPC_CHANNELS 에 중복이 있다");
    let union: Vec<&str> = owner.keys().copied().collect();
    assert_eq!(union, ipc_sorted, "Rust 처리 목록의 합집합과 IPC_CHANNELS 가 같아야 한다");
}

#[tokio::test]
async fn 설정_소유자만_단_라우터는_ipc_전_채널을_봉투_하나로_답하고_밖으로_보내지_않는다() {
    let tmp = Tmp::new();
    let router = owned(&tmp, mirror_with(&[("loc", "local"), ("gh", "github"), ("ssh", "ssh"), ("w", "wsl")]));
    let mut channels = ipc_channels();
    // 목록 밖의 채널도 어디로도 가지 않는다
    channels.extend(["foo:bar", "", "update:", "update:get ", "UPDATE:GET", "update:getX", "git:log\0", "gh:repoX"].map(String::from));
    for ch in &channels {
        for args in arg_shapes() {
            // `route` 는 처리했거나(Done) 풀지 못했다(Unserved)이고, 앱 경계는 항상 봉투를 돌려준다
            if let Routed::Done(env) = router.route(ch, &args).await {
                assert!(env.get("ok").is_some_and(Value::is_boolean), "{ch} {args}: {env}");
            }
            let env = router.invoke(ch, &args).await;
            assert!(env.get("ok").is_some_and(Value::is_boolean), "{ch} {args}: {env}");
            if env["ok"] == false {
                assert!(env["error"].as_str().is_some_and(|m| !m.is_empty()), "{ch} {args}: {env}");
            }
        }
    }
    // 포트(앱·환경·jira·github·fetch·ssh·update)가 없는 채널은 풀지 못하지만 설정 소유자·라우터 상태 채널은 처리한다
    for ch in CONFIG_CHANNELS.iter().chain(ROUTER_CHANNELS) {
        for args in arg_shapes() {
            assert!(!is_unserved(&router.route(ch, &args).await), "{ch} {args}");
        }
    }
    for ch in APP_CHANNELS.iter().chain(ENV_CHANNELS).chain(API_CHANNELS).chain(UPDATE_CHANNELS) {
        for args in arg_shapes() {
            assert!(is_unserved(&router.route(ch, &args).await), "{ch} {args}");
        }
    }
    // 목록에 없는 채널은 명시적 오류다
    for ch in ["foo:bar", "", "update:", "UPDATE:GET", "update:getX", "gh:repoX"] {
        let env = router.invoke(ch, &json!([])).await;
        assert_eq!(env, json!({ "ok": false, "error": format!("알 수 없는 채널: {ch}"), "code": "ERROR" }), "{ch:?}");
    }
}

fn error_of(env: Value) -> String {
    assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{env}");
    env["error"].as_str().expect("오류 문구").to_string()
}

#[tokio::test]
async fn rust_가_풀지_못한_호출은_ts_와_같은_문구의_오류로_답한다() {
    let tmp = Tmp::new();
    // github(서비스·캐시 실행기 없음)·ssh(풀 없음)는 이 라우터가 풀지 못한다. wsl 은 Windows 밖에서만 풀지 못한다
    let router = owned(&tmp, mirror_with(&[("loc", "local"), ("gh", "github"), ("ssh", "ssh"), ("w", "wsl")]));
    assert_eq!(error_of(router.invoke("git:log", &json!(["nope", { "skip": 0, "count": 1 }])).await), "unknown repo: nope");
    assert_eq!(error_of(router.invoke("git:refs", &json!(["nope"])).await), "unknown repo: nope");
    assert_eq!(error_of(router.invoke("gh:repo", &json!(["nope"])).await), "unknown repo: nope");
    assert_eq!(error_of(router.invoke("git:fetch", &json!(["nope", true])).await), "unknown repo: nope");
    assert_eq!(error_of(router.invoke("git:commit", &json!(["gh", "abcd1234"])).await), "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
    assert_eq!(error_of(router.invoke("git:blob", &json!(["gh", "abcd1234", "a.txt"])).await), "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
    assert_eq!(error_of(router.invoke("git:refs", &json!(["ssh"])).await), "SSH 를 사용할 수 없습니다");
    if !cfg!(windows) {
        assert_eq!(error_of(router.invoke("git:refs", &json!(["w"])).await), "WSL 저장소는 Windows 에서만 열 수 있습니다");
    }
    // 긴 id 는 문구에서 자른다
    let long = "x".repeat(1000);
    assert_eq!(error_of(router.invoke("git:refs", &json!([long])).await).chars().count(), "unknown repo: ".chars().count() + 200);
    // 모양이 틀린 호출(저장소 id 가 없다)은 채널마다 TS 문구를 따르지 않고 하나로 답한다
    for args in [json!([]), json!("nope"), Value::Null, json!([123])] {
        assert_eq!(error_of(router.invoke("git:refs", &args).await), "잘못된 입력입니다", "{args}");
    }
    // 앱·서비스 채널의 포트가 없어도 인자는 어디로도 가지 않고 오류다(복사할 텍스트·토큰 인자)
    for (ch, args) in [("clipboard:writeText", json!(["secret"])), ("dialog:pickFolder", json!([])), ("env:wslDistros", json!([])), ("gh:loginPat", json!(["ghp_secret"]))] {
        let msg = error_of(router.invoke(ch, &args).await);
        assert!(!msg.contains("secret"), "{ch}: {msg}");
    }
    assert_eq!(error_of(router.invoke("gh:loginPat", &json!(["ghp_secret"])).await), "연결 서비스를 사용할 수 없습니다");
    assert_eq!(error_of(router.invoke("clipboard:writeText", &json!(["secret"])).await), "이 기능을 사용할 수 없습니다");
}

#[test]
fn 앱의_호출_경로는_라우터의_invoke_하나이고_사이드카_흔적이_없다() {
    let router = read_src("src/router.rs");
    // `Routed::Unserved` 를 오류 봉투로 바꾸는 곳은 `invoke` 하나다
    assert_eq!(router.matches("pub async fn invoke(").count(), 1);
    assert_eq!(router.matches("unserved_message(").count(), 2, "정의 하나 + invoke 의 호출 하나");
    let commands = read_src("src/commands.rs");
    assert!(commands.contains("router.invoke(&channel, &args).await"));
    assert!(!commands.contains("Sidecar") && !commands.contains("sidecar"));
    // 오류 문구: Electron 판 RepoService 의 문구를 옮긴 것이다(그 TS 소스는 Phase 6c PR 7 에서 지워졌다 — 이제 문구의 원천은 이 Rust 소스다)
    for text in ["unknown repo: ", "GitHub 원격 저장소를 쓸 수 없는 환경입니다"] {
        assert!(router.contains(text), "router.rs 에 {text} 가 없다");
    }
    assert!(read_src("src/external/plan.rs").contains("WSL 저장소는 Windows 에서만 열 수 있습니다"));
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Phase 6c PR 5: Tauri 판에 Node 사이드카와 되돌리기 스위치의 흔적이 없다. 주석·시험 모듈을 뺀 운영 코드(식별자·문자열)를 본다 —
/// `src-tauri/src` 전부(`update/` 포함), 앱 설정(`tauri.conf.json`·`Cargo.toml`)과 `package.json`·`tsconfig.node.json`·CI 시험 목록
#[test]
fn 사이드카와_되돌리기_스위치의_흔적이_없다() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    assert!(files.len() > 50, "소스를 찾지 못했다");
    for f in files {
        let rel = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        let code = prod_code(&rel);
        for banned in ["Sidecar", "sidecar", "ANTLER_RUST_", "ANTLER_NODE", "rust_git_enabled", "claim_api", "config_owned", "api_enabled", "ssh_enabled", "with_ssh(", "with_api("] {
            assert!(!code.contains(banned), "{rel} 에 {banned}");
        }
    }
    // 파일·설정
    let repo = root.parent().unwrap();
    for gone in ["src-tauri/src/sidecar.rs", "src/sidecar", "vite.sidecar.config.mts", "out-tauri/sidecar"] {
        assert!(!repo.join(gone).exists(), "{gone} 가 남아 있다");
    }
    for (rel, banned) in [
        ("src-tauri/tauri.conf.json", ["sidecar", "externalBin", "tauri:sidecar"].as_slice()),
        ("src-tauri/Cargo.toml", ["sidecar"].as_slice()),
        ("package.json", ["sidecar"].as_slice()),
        ("tsconfig.node.json", ["sidecar"].as_slice()),
        (".github/workflows/tauri.yml", ["sidecar_scope", "src/sidecar"].as_slice()),
        (".github/workflows/tauri-package.yml", ["sidecar"].as_slice()),
    ] {
        let text = read_src(&format!("../{rel}")).to_lowercase();
        for b in banned {
            assert!(!text.contains(&b.to_lowercase()), "{rel} 에 {b}");
        }
    }
    // 되돌리기 스위치 환경변수를 읽는 곳이 없다(라우터의 로그 스위치 `ANTLER_ROUTE_LOG` 는 읽기 전용 진단이다)
    let lib = prod_code("src/lib.rs");
    assert!(!lib.contains("ANTLER_RUST") && !lib.contains("Sidecar::spawn(") && !lib.contains("app.manage(sidecar"));
}
