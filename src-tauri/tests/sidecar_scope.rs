//! 사이드카 범위 불변식(Phase 6b PR 10): **Rust 가 설정을 소유하는 실행에서 사이드카로 가는 채널은 `update:*` 7개뿐이다.**
//! 라우터의 관문(`Router::route`)이 `update:*` 밖의 채널이 `Routed::Sidecar` 가 되는 길을 모두 오류로 바꾼다 — 포트가 없는 라우터·인자 모양이 틀린 호출·Rust 가 풀지 못한 저장소 포함.
//! 되돌리기(`ANTLER_RUST_GIT=0`)는 6c 까지 사이드카가 전부 처리하므로 그 라우터는 전부 사이드카다(C1). 사이드카 쪽 거부는 vitest(`tests/sidecarConfigOwns.test.ts`).
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
use antler_lib::router::{Routed, Router, API_CHANNELS, API_SIDECAR_CHANNELS, APP_CHANNELS, CONFIG_CHANNELS, ENV_CHANNELS, RUST_CHANNELS, UNDEFINED_ARG, UPDATE_CHANNELS};
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
        let dir = std::env::temp_dir().join(format!("antler-sidecar-scope-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
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

/// 설정을 소유한 라우터: **설정 소유자만** 달고 앱·환경·jira·github·fetch·ssh 포트는 달지 않는다(새는 길은 "포트 없음" 갈래에 있다)
fn owned(tmp: &Tmp, mirror: RepoMirror) -> Router {
    let router = Router::new(true, false, mirror);
    let store = Arc::new(ConfigStore::open_for_test(tmp.0.join("antler.config.json"), None, || "id-1".to_string(), || 0));
    let env = EnvHost::new(Arc::new(|| async { Vec::new() }.boxed()), Arc::new(Vec::new), Arc::new(|_| false), Arc::new(|_, _| GitExecutor::Local(LocalExecutor::default())));
    let host = Arc::new(ConfigHost::new(store, env, String::new(), Arc::new(|| Ok(None)), Arc::new(NoDeps)));
    router.set_config(host);
    assert!(router.config_owned());
    router
}

/// 인자 모양 표: 정상(없는 id)·빈 배열·배열 아님·null·숫자·표식·객체
fn arg_shapes() -> Vec<Value> {
    vec![json!(["nope"]), json!([]), json!("nope"), Value::Null, json!([123]), json!([{ UNDEFINED_ARG: true }]), json!([{ "kind": "local" }]), json!({ "a": 1 })]
}

fn is_sidecar(r: &Routed) -> bool {
    matches!(r, Routed::Sidecar)
}

#[test]
fn update_채널_7개는_ipc_채널의_update_접두_채널과_정확히_같다() {
    let mut want: Vec<String> = ipc_channels().into_iter().filter(|c| c.starts_with("update:")).collect();
    let mut have: Vec<String> = UPDATE_CHANNELS.iter().map(|c| (*c).to_string()).collect();
    want.sort();
    have.sort();
    assert_eq!(have, want);
    assert_eq!(UPDATE_CHANNELS.len(), 7);
    // 다른 목록과 겹치지 않는다 — update:* 는 라우터의 어떤 Rust 처리 갈래도 아니다
    for list in [RUST_CHANNELS, API_CHANNELS, API_SIDECAR_CHANNELS, APP_CHANNELS, ENV_CHANNELS, CONFIG_CHANNELS] {
        for ch in UPDATE_CHANNELS {
            assert!(!list.contains(ch), "{ch}");
        }
    }
}

#[test]
fn update_밖의_모든_ipc_채널은_rust_처리_목록_중_하나에_있다() {
    // 새 채널을 IPC 에 더하고 라우터에 처리 갈래를 빠뜨리면 소유 실행에서 "알 수 없는 채널" 오류가 난다 — 여기서 먼저 잡는다
    let mut covered: Vec<&str> = Vec::new();
    for list in [RUST_CHANNELS, API_CHANNELS, API_SIDECAR_CHANNELS, APP_CHANNELS, ENV_CHANNELS, CONFIG_CHANNELS] {
        covered.extend(list.iter().copied());
    }
    for ch in ipc_channels() {
        if UPDATE_CHANNELS.contains(&ch.as_str()) {
            continue;
        }
        assert!(covered.contains(&ch.as_str()), "{ch} 는 Rust 처리 목록에도 update:* 에도 없다 — 소유 실행에서 아무도 처리하지 않는다");
    }
}

#[tokio::test]
async fn 설정을_소유한_라우터에서_사이드카로_가는_채널은_update_7개뿐이다() {
    let tmp = Tmp::new();
    let router = owned(&tmp, mirror_with(&[("loc", "local"), ("gh", "github"), ("ssh", "ssh"), ("w", "wsl")]));
    let mut channels = ipc_channels();
    // 허용 목록 밖의 채널도 사이드카로 가지 않는다(사이드카의 허용 목록 검사에 기대지 않는다)
    channels.extend(["foo:bar", "", "update:", "update:get ", "UPDATE:GET", "update:getX", "git:log\0", "gh:repoX"].map(String::from));
    let mut sidecar_channels: Vec<String> = Vec::new();
    for ch in &channels {
        for args in arg_shapes() {
            let routed = router.route(ch, &args).await;
            if is_sidecar(&routed) {
                if !sidecar_channels.contains(ch) {
                    sidecar_channels.push(ch.clone());
                }
            } else if let Routed::Done(env) = routed {
                // Rust 가 답하면 봉투다
                assert!(env.get("ok").is_some_and(Value::is_boolean), "{ch} {args}: {env}");
            }
        }
    }
    sidecar_channels.sort();
    let mut want: Vec<String> = UPDATE_CHANNELS.iter().map(|c| (*c).to_string()).collect();
    want.sort();
    assert_eq!(sidecar_channels, want, "Rust 가 설정을 소유한 실행에서 사이드카로 가는 채널 집합");
    // update:* 는 인자 모양과 무관하게 사이드카다(상태·진행은 사이드카의 UpdateService 가 쥔다 — 6c 까지)
    for ch in UPDATE_CHANNELS {
        for args in arg_shapes() {
            assert!(is_sidecar(&router.route(ch, &args).await), "{ch} {args}");
        }
    }
}

#[tokio::test]
async fn 되돌리기_라우터는_모든_채널이_사이드카다() {
    // ANTLER_RUST_GIT=0 — 소유자도 포트도 없다. 사이드카가 설정·서비스를 전부 맡는다(6c 에서 사이드카와 함께 사라진다)
    let off = Router::new(false, false, mirror_with(&[("loc", "local")]));
    assert!(!off.config_owned());
    let mut channels = ipc_channels();
    channels.extend(["foo:bar", ""].map(String::from));
    for ch in &channels {
        for args in arg_shapes() {
            assert!(is_sidecar(&off.route(ch, &args).await), "{ch} {args}");
        }
    }
    assert_eq!(off.rust_handled(), 0);
}

fn error_of(routed: Routed) -> String {
    match routed {
        Routed::Done(env) => {
            assert_eq!((env["ok"].as_bool(), env["code"].as_str()), (Some(false), Some("ERROR")), "{env}");
            env["error"].as_str().expect("오류 문구").to_string()
        }
        Routed::Sidecar => panic!("사이드카로 갔다"),
    }
}

#[tokio::test]
async fn rust_가_풀지_못한_호출은_사이드카가_아니라_ts_와_같은_문구의_오류로_답한다() {
    let tmp = Tmp::new();
    // github(서비스·캐시 실행기 없음)·ssh(풀 없음)는 이 라우터가 풀지 못한다. wsl 은 Windows 밖에서만 풀지 못한다
    let router = owned(&tmp, mirror_with(&[("loc", "local"), ("gh", "github"), ("ssh", "ssh"), ("w", "wsl")]));
    assert_eq!(error_of(router.route("git:log", &json!(["nope", { "skip": 0, "count": 1 }])).await), "unknown repo: nope");
    assert_eq!(error_of(router.route("git:refs", &json!(["nope"])).await), "unknown repo: nope");
    assert_eq!(error_of(router.route("gh:repo", &json!(["nope"])).await), "unknown repo: nope");
    assert_eq!(error_of(router.route("git:fetch", &json!(["nope", true])).await), "unknown repo: nope");
    assert_eq!(error_of(router.route("git:commit", &json!(["gh", "abcd1234"])).await), "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
    assert_eq!(error_of(router.route("git:blob", &json!(["gh", "abcd1234", "a.txt"])).await), "GitHub 원격 저장소를 쓸 수 없는 환경입니다");
    assert_eq!(error_of(router.route("git:refs", &json!(["ssh"])).await), "SSH 를 사용할 수 없습니다");
    if !cfg!(windows) {
        assert_eq!(error_of(router.route("git:refs", &json!(["w"])).await), "WSL 저장소는 Windows 에서만 열 수 있습니다");
    }
    // 긴 id 는 문구에서 자른다
    let long = "x".repeat(1000);
    assert_eq!(error_of(router.route("git:refs", &json!([long])).await).chars().count(), "unknown repo: ".chars().count() + 200);
    // 모양이 틀린 호출(저장소 id 가 없다)은 채널마다 TS 문구를 따르지 않고 하나로 답한다
    for args in [json!([]), json!("nope"), Value::Null, json!([123])] {
        assert_eq!(error_of(router.route("git:refs", &args).await), "잘못된 입력입니다", "{args}");
    }
    assert_eq!(error_of(router.route("foo:bar", &json!([])).await), "알 수 없는 채널: foo:bar");
    // 앱·서비스 채널의 포트가 없어도 사이드카로 가지 않는다(복사할 텍스트·토큰 인자가 사이드카 stdin 으로 가지 않는다)
    for (ch, args) in [("clipboard:writeText", json!(["secret"])), ("dialog:pickFolder", json!([])), ("env:wslDistros", json!([])), ("gh:loginPat", json!(["ghp_secret"]))] {
        assert!(!is_sidecar(&router.route(ch, &args).await), "{ch}");
    }
}

#[test]
fn 사이드카로_가는_길은_관문_하나이고_앱은_그_결과만_따른다() {
    let router = read_src("src/router.rs");
    // 내부 라우팅은 공개가 아니다 — 관문(`route`)을 거치지 않고는 `Routed::Sidecar` 를 얻을 수 없다
    assert!(router.contains("async fn route_inner(") && !router.contains("pub async fn route_inner("));
    assert!(router.contains("self.route_inner(channel, args).await"));
    assert_eq!(router.matches("self.route_inner(").count(), 1);
    // 앱의 호출 경로는 라우터의 `route` 결과만 따라 사이드카에 보낸다
    let commands = read_src("src/commands.rs");
    assert_eq!(commands.matches("sidecar.request(").count(), 1);
    assert!(commands.contains("router.route(&channel, &args).await"));
    // 사이드카에 요청을 보내는 곳은 이 한 곳뿐이다(다른 모듈이 `request(` 를 부르지 않는다)
    for rel in ["src/lib.rs", "src/config_host.rs", "src/env_host.rs", "src/app_host.rs", "src/api_host.rs", "src/watch_host.rs", "src/fetch_host.rs", "src/routed_repos.rs"] {
        assert!(!read_src(rel).contains("sidecar.request("), "{rel}");
    }
    // 오류 문구의 출처: TS RepoService 의 문구를 그대로 옮겼다
    let ts = read_src("../src/main/services/RepoService.ts");
    for text in ["`unknown repo: ${repoId}`", "'GitHub 원격 저장소를 쓸 수 없는 환경입니다'", "'WSL 저장소는 Windows 에서만 열 수 있습니다'"] {
        assert!(ts.contains(text), "RepoService.ts 에 {text} 가 없다");
    }
}

#[test]
fn 사이드카가_더는_받지_않는_메시지의_송수신_코드가_없다() {
    let sidecar = prod_code("src/sidecar.rs");
    for gone in ["\"fetchApprove\"", "\"githubBranch\"", "\"repoq\"", "\"repoqCancel\"", "\"ghToken\"", "\"note\"", "fn note(", "ANTLER_RUST_FETCH_KINDS", "ANTLER_RUST_API", "ANTLER_RUST_SSH"] {
        assert!(!sidecar.contains(gone), "src/sidecar.rs 에 {gone}");
    }
    assert!(!Path::new(env!("CARGO_MANIFEST_DIR")).join("src/repoq.rs").exists());
    let lib = prod_code("src/lib.rs");
    assert!(!lib.contains("set_note_sink(") && !lib.contains("mod repoq"));
    // 포커스는 설정을 소유하지 않는 실행에서만 보낸다
    assert!(sidecar.contains("if self.config_owned {\n            return;\n        }"));
    // 사이드카 쪽: 받는 메시지는 req·focus·hostres 뿐이다
    let ts = read_src("../src/sidecar/index.ts");
    for gone in ["'note'", "'fetchApprove'", "'githubBranch'", "'repoq'", "'repoqCancel'", "'ghToken'"] {
        assert!(!ts.contains(gone), "src/sidecar/index.ts 에 {gone}");
    }
    let proto = read_src("../src/sidecar/protocol.ts");
    for gone in ["NOTE_OPS", "FETCH_APPROVE_KINDS", "REPOQ_OPS", "t: 'note'", "t: 'fetchApprove'", "t: 'githubBranch'", "t: 'repoq'", "t: 'repoqCancel'", "t: 'ghToken'"] {
        assert!(!proto.contains(gone), "protocol.ts 에 {gone}");
    }
}
