//! `jira:*` 라우팅(Phase 4 PR 6) 통합 테스트: 스위치(`ANTLER_RUST_API`·`ANTLER_RUST_GIT`)에 따른 라우터의 선택, 꺼졌을 때 서비스·키체인이 없다는 것,
//! 켜졌을 때 `jira:*` 가 사이드카로 가지 않는다는 것, 앱 배선 순서와 열기 경로의 소스 검사. 네트워크·git 을 쓰지 않는다
//! (연결하지 않은 서비스·잘못된 인자로만 부르므로 HTTP 요청이 나가지 않는다). 채널 골든(TS 와의 패리티)은 `channels.rs` 의 `jira_채널_시나리오가_ts_골든과_같다`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use antler_lib::api_host::JiraHost;
use antler_lib::jira::service::JiraService;
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{rust_api_enabled, Routed, Router, API_CHANNELS, RUST_CHANNELS};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------------------------------
// 소스 검사
// ---------------------------------------------------------------------------------------------------------------------

fn read_src(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 운영 코드의 끝(바이트 위치) — 들여쓰기 없는 `#[cfg(test)]` 바로 다음 줄이 `mod ` 인 곳
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

fn src_files() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&dir, &mut files);
    files.iter().map(|f| f.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect()
}

fn ipc_channels() -> Vec<String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("tests/golden/exec/channels.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    v["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()
}

#[test]
fn api_채널은_ipc_채널의_jira_채널과_정확히_같다() {
    let all = ipc_channels();
    for ch in API_CHANNELS {
        assert!(all.iter().any(|c| c == ch), "API_CHANNELS 의 {ch} 가 IPC_CHANNELS 에 없다(tests/golden/exec/channels.json)");
    }
    let mut jira: Vec<&str> = all.iter().map(String::as_str).filter(|c| c.starts_with("jira:")).collect();
    let mut api: Vec<&str> = API_CHANNELS.to_vec();
    jira.sort_unstable();
    api.sort_unstable();
    assert_eq!(api, jira, "IPC_CHANNELS 의 jira:* 와 API_CHANNELS 가 다르다 — 빠진 채널은 사이드카로 가서 토큰을 두 곳에서 다루게 된다");
    // git 채널 목록과 겹치지 않는다
    for ch in API_CHANNELS {
        assert!(!RUST_CHANNELS.contains(ch));
    }
    // 사이드카가 소유하지 않는 접두어가 같다
    let owns = read_src("../src/sidecar/apiOwns.ts");
    assert!(owns.contains("['jira:']"));
    assert!(owns.contains("value === '1'"));
    assert!(read_src("../src/sidecar/index.ts").contains("process.env['ANTLER_RUST_API']"));
}

#[test]
fn 스위치는_api_가_0_이거나_git_이_0_이면_꺼진다() {
    assert!(rust_api_enabled(None, None));
    assert!(rust_api_enabled(Some("1"), None));
    assert!(rust_api_enabled(Some(""), Some("1")));
    assert!(rust_api_enabled(Some("false"), None));
    assert!(!rust_api_enabled(Some("0"), None));
    assert!(!rust_api_enabled(None, Some("0")));
    assert!(!rust_api_enabled(Some("1"), Some("0")));
    assert!(!rust_api_enabled(Some("0"), Some("0")));
}

#[test]
fn 서비스_보관소_키체인은_스위치가_켜진_분기에서만_만든다() {
    let lib = prod_code("src/lib.rs");
    let guard = lib.find("if router.api_enabled() {").expect("스위치 분기");
    // 서비스·Vault·보관소를 만드는 호출은 이 분기 안(사이드카 기동 전)에만 있다
    let end = lib.find("let fetch_kinds").expect("분기 끝");
    for needle in ["open_store(", "Vault::new(", "JiraService::new("] {
        assert_eq!(lib.matches(needle).count(), 1, "{needle} 는 한 번만");
        let at = lib.find(needle).unwrap();
        assert!(guard < at && at < end, "{needle} 가 스위치 분기 밖에 있다");
    }
    // 보관소(키체인)를 여는 곳은 앱에서 이 한 곳뿐이다(secret 모듈 안의 정의·위임은 제외)
    for rel in src_files() {
        if rel == "lib.rs" || rel == "secret/mod.rs" {
            continue;
        }
        let code = prod_code(&format!("src/{rel}"));
        for banned in ["open_store(", "open_default_store(", "KeyringStore::open", "Vault::new("] {
            assert!(!code.contains(banned), "{rel} 가 {banned} 를 부른다");
        }
    }
    // 서비스가 실제로 만들어졌을 때만 사이드카에 알린다. 서비스는 사이드카 기동·라우터 등록보다 먼저다
    let jira = lib.find("router.set_jira(").expect("서비스 주입");
    let api = lib.find("router.api_active()").expect("사이드카 env 값");
    let manage = lib.find("app.manage(router)").expect("라우터 등록");
    let spawn = lib.find("Sidecar::spawn(").expect("사이드카 기동");
    assert!(jira < api && api < manage && manage < spawn);
    assert!(lib.contains(", fetch_kinds, rust_api)"));
}

#[test]
fn 사이드카_env_는_항상_직접_정한다() {
    let sidecar = prod_code("src/sidecar.rs");
    assert!(sidecar.contains("cmd.env(\"ANTLER_RUST_API\", \"1\")"));
    // 값이 없을 때 부모의 env 를 지운다(되돌리기가 뒤집히지 않게)
    assert!(sidecar.contains("cmd.env_remove(\"ANTLER_RUST_API\")"));
}

#[test]
fn 운영_코드에_시험_훅_호출이_없다() {
    for rel in ["api_host.rs", "router.rs", "lib.rs", "sidecar.rs"] {
        let code = prod_code(&format!("src/{rel}"));
        for banned in ["_for_test", "insecure_loopback", "standing_in_for_keychain", "test-hooks"] {
            assert!(!code.contains(banned), "{rel} 에 {banned}");
        }
    }
}

#[test]
fn 열기는_검증한_주소만_쓰고_https_가_아니면_열지_않는다() {
    let host = prod_code("src/api_host.rs");
    // opener 호출은 한 곳이고 서비스의 주소 검증(external_url) 뒤에 있다
    assert_eq!(host.matches("opener(&href)").count(), 1);
    assert!(host.find("external_url(").unwrap() < host.find("opener(&href)").unwrap());
    let lib = prod_code("src/lib.rs");
    assert_eq!(lib.matches("tauri_plugin_opener::open_url(").count(), 2, "github 링크(새 창)와 open_https 두 곳");
    let open_https = &lib[lib.find("fn open_https(").unwrap()..];
    assert!(open_https.find("starts_with(\"https://\")").unwrap() < open_https.find("open_url(").unwrap());
}

#[test]
fn api_host_는_앱_핸들과_사이드카에_기대지_않는다() {
    let host = prod_code("src/api_host.rs");
    for banned in ["AppHandle", "Sidecar", "tauri::", "tauri_plugin"] {
        assert!(!host.contains(banned), "api_host.rs 에 {banned}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 라우팅
// ---------------------------------------------------------------------------------------------------------------------

fn host() -> JiraHost {
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::new())));
    let service = JiraService::new(vault).expect("운영 클라이언트");
    JiraHost { service, opener: Arc::new(|_| Ok(())) }
}

/// 연결하지 않은 서비스에 보내도 HTTP 요청이 나가지 않는 호출(채널마다 하나)
fn offline_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("jira:auth", json!([])),
        ("jira:connect", json!([])),
        ("jira:disconnect", json!([])),
        ("jira:issue", json!(["ABC-1"])),
        ("jira:transitions", json!(["ABC-1", "r"])),
        ("jira:transition", json!(["ABC-1", "31"])),
        ("jira:comments", json!(["ABC-1"])),
        ("jira:search", json!([{ "jql": "a = b" }])),
        ("jira:dev", json!(["ABC-1"])),
        ("jira:worklogs", json!([{ "from": "2026-10-19", "to": "2026-10-20", "users": [] }])),
        ("jira:userSearch", json!(["김"])),
        ("jira:avatar", json!(["https://example.com/a.png"])),
        ("jira:openExternal", json!(["https://example.com/"])),
    ]
}

#[test]
fn 오프라인_호출표가_모든_api_채널을_덮는다() {
    let covered: Vec<&str> = offline_calls().iter().map(|(c, _)| *c).collect();
    for ch in API_CHANNELS {
        assert!(covered.contains(ch), "{ch} 호출이 표에 없다");
    }
}

#[tokio::test]
async fn 스위치가_켜지면_jira_채널은_미러와_무관하게_rust_가_처리한다() {
    // 미러는 비어 있다 — git 채널과 달리 저장소와 무관하다
    let router = Router::new(true, false, RepoMirror::default());
    assert!(router.api_enabled());
    assert!(!router.api_active(), "서비스가 주입되기 전에는 활성이 아니다");
    router.set_jira(host());
    assert!(router.api_active());
    for (ch, args) in offline_calls() {
        let Routed::Done(env) = router.route(ch, &args).await else { panic!("{ch} 가 사이드카로 갔다") };
        assert!(env["ok"].is_boolean(), "{ch}: {env}");
    }
    assert_eq!(router.rust_handled(), API_CHANNELS.len() as u64);
}

#[tokio::test]
async fn 연결하지_않으면_조회는_jira_auth_이고_검증_실패는_error_다() {
    let router = Router::new(true, false, RepoMirror::default());
    router.set_jira(host());
    let call = |ch: &'static str, args: Value| {
        let router = &router;
        async move {
            let Routed::Done(env) = router.route(ch, &args).await else { panic!("{ch}") };
            env
        }
    };
    assert_eq!(call("jira:auth", json!([])).await, json!({ "ok": true, "data": { "connected": false, "storageUnavailable": true } }));
    assert_eq!(call("jira:issue", json!(["ABC-1"])).await["code"], "JIRA_AUTH");
    assert_eq!(call("jira:issue", json!(["abc-1"])).await["code"], "ERROR");
    // 없는 인자(표식)는 undefined 다
    assert_eq!(call("jira:issue", json!([{ antler_lib::router::UNDEFINED_ARG: true }])).await["code"], "ERROR");
    assert_eq!(call("jira:connect", json!([{ "kind": "cloud", "siteUrl": "http://example.com", "email": "a@b.c", "token": "tok" }])).await["code"], "ERROR");
    // 연결한 사이트가 없으면 어떤 주소도 열지 않는다
    assert_eq!(call("jira:openExternal", json!(["https://example.com/"])).await["code"], "ERROR");
    // void 채널은 data 가 없다(사이드카의 JSON 직렬화와 같다) — 여기서는 연결이 없어 오류다
    assert_eq!(call("jira:transition", json!(["ABC-1", "31"])).await["code"], "JIRA_AUTH");
    assert_eq!(call("jira:avatar", json!(["https://example.com/a.png"])).await, json!({ "ok": true, "data": null }));
}

#[tokio::test]
async fn 스위치가_꺼지면_서비스를_두지_않고_jira_채널은_사이드카로_간다() {
    // ANTLER_RUST_API=0 에 해당: 서비스를 주입해도 받지 않는다
    let off = Router::new(true, false, RepoMirror::default()).with_api(false);
    assert!(!off.api_enabled());
    off.set_jira(host());
    assert!(!off.api_active());
    // ANTLER_RUST_GIT=0 에 해당(라우터 전체가 꺼진다)
    let git_off = Router::new(false, false, RepoMirror::default());
    git_off.set_jira(host());
    assert!(!git_off.api_active());
    for r in [&off, &git_off] {
        for (ch, args) in offline_calls() {
            assert!(matches!(r.route(ch, &args).await, Routed::Sidecar), "{ch}");
        }
        assert_eq!(r.rust_handled(), 0);
    }
}

#[tokio::test]
async fn 서비스를_만들지_못했으면_사이드카가_처리한다() {
    // 스위치는 켜졌지만 서비스가 주입되지 않았다(생성 실패) — 활성이 아니고 jira:* 는 사이드카로 간다(앱은 사이드카에 env 를 주지 않는다)
    let r = Router::new(true, false, RepoMirror::default());
    assert!(r.api_enabled() && !r.api_active());
    for (ch, args) in offline_calls() {
        assert!(matches!(r.route(ch, &args).await, Routed::Sidecar), "{ch}");
    }
}

#[tokio::test]
async fn req_cancel_은_jira_가_켜져_있어도_사이드카에_전달한다() {
    let r = Router::new(true, false, RepoMirror::default());
    r.set_jira(host());
    assert!(matches!(r.route("req:cancel", &json!(["x"])).await, Routed::Sidecar));
    assert!(matches!(r.route("req:cancel", &json!([])).await, Routed::Sidecar));
}

#[tokio::test]
async fn 등록_전에_온_cancel_은_조회를_취소하지만_전환은_취소하지_않는다() {
    let r = Router::new(true, false, RepoMirror::default());
    r.set_jira(host());
    assert!(matches!(r.route("req:cancel", &json!(["early"])).await, Routed::Sidecar));
    let Routed::Done(env) = r.route("jira:issue", &json!(["ABC-1", "early"])).await else { panic!() };
    assert_eq!(env["code"], "ABORTED");
    // 전환은 requestId 를 받지 않는다 — 같은 id 를 덧붙여도 취소되지 않고 서비스의 결과(연결 없음)가 나온다
    assert!(matches!(r.route("req:cancel", &json!(["early2"])).await, Routed::Sidecar));
    let Routed::Done(env) = r.route("jira:transition", &json!(["ABC-1", "31", "early2"])).await else { panic!() };
    assert_eq!(env["code"], "JIRA_AUTH");
    // 빈 문자열·문자열이 아닌 requestId 는 없는 것이다
    for rid in [json!(""), json!(5), json!(null)] {
        let Routed::Done(env) = r.route("jira:issue", &json!(["ABC-1", rid])).await else { panic!() };
        assert_eq!(env["code"], "JIRA_AUTH", "{rid}");
    }
}

#[tokio::test]
async fn 다른_채널은_영향을_받지_않는다() {
    let r = Router::new(true, false, RepoMirror::default());
    r.set_jira(host());
    for ch in ["gh:auth", "git:refs", "repos:list", "gh:loginPat"] {
        assert!(matches!(r.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
    }
    assert_eq!(r.rust_handled(), 0);
}
