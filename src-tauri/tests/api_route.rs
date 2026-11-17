//! `jira:*`·`gh:*` 라우팅(Phase 4 PR 6·9) 통합 테스트: 스위치(`ANTLER_RUST_API`·`ANTLER_RUST_GIT`)에 따른 라우터의 선택, 꺼졌을 때 서비스·키체인이 없다는 것,
//! 켜졌을 때 `jira:*` 가 사이드카로 가지 않는다는 것, 앱 배선 순서와 열기 경로의 소스 검사. 네트워크·git 을 쓰지 않는다
//! (연결하지 않은 서비스·잘못된 인자로만 부르므로 HTTP 요청이 나가지 않는다). 채널 골든(TS 와의 패리티)은 `channels.rs` 의 `jira_채널_시나리오가_ts_골든과_같다`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use antler_lib::api_host::{GithubHost, JiraHost};
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use antler_lib::jira::service::JiraService;
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{rust_api_enabled, Routed, Router, API_CHANNELS, API_SIDECAR_CHANNELS, RUST_CHANNELS};
use futures_util::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;
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
fn api_채널은_ipc_채널의_jira_gh_채널에서_사이드카_몫을_뺀_것과_정확히_같다() {
    let all = ipc_channels();
    for ch in API_CHANNELS.iter().chain(API_SIDECAR_CHANNELS) {
        assert!(all.iter().any(|c| c == ch), "{ch} 가 IPC_CHANNELS 에 없다(tests/golden/exec/channels.json)");
    }
    let mut want: Vec<&str> = all.iter().map(String::as_str).filter(|c| (c.starts_with("jira:") || c.starts_with("gh:")) && !API_SIDECAR_CHANNELS.contains(c)).collect();
    let mut api: Vec<&str> = API_CHANNELS.to_vec();
    want.sort_unstable();
    api.sort_unstable();
    assert_eq!(api, want, "IPC_CHANNELS 의 jira:*·gh:*(gh:repo 제외)와 API_CHANNELS 가 다르다 — 빠진 채널은 사이드카로 가서 토큰을 두 곳에서 다루게 된다");
    assert_eq!(API_SIDECAR_CHANNELS, ["gh:repo"], "사이드카에 남기는 gh 채널은 gh:repo 뿐이다");
    // git 채널 목록과 겹치지 않는다
    for ch in API_CHANNELS {
        assert!(!RUST_CHANNELS.contains(ch));
    }
    // 사이드카가 소유하지 않는 접두어와 예외가 같다
    let owns = read_src("../src/sidecar/apiOwns.ts");
    assert!(owns.contains("['jira:', 'gh:']"));
    assert!(owns.contains("SIDECAR_KEPT_CHANNELS = ['gh:repo']"));
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
    for needle in ["open_store(", "Vault::new(", "JiraService::new(", "GitHubService::new("] {
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
    let github = lib.find("router.set_github(").expect("GitHub 서비스 주입");
    let api = lib.find("router.claim_api()").expect("사이드카 env 값(라우팅과 같은 api_active 판정)");
    let manage = lib.find("app.manage(router)").expect("라우터 등록");
    let spawn = lib.find("Sidecar::spawn(").expect("사이드카 기동");
    assert!(jira < api && github < api && api < manage && manage < spawn);
    assert!(lib.contains(", fetch_kinds, rust_api)"));
    // env 하나가 두 서비스를 함께 넘기므로 둘 다 만들어졌을 때만 둘 다 주입한다(하나만 되면 둘 다 사이드카가 처리한다)
    assert_eq!(lib.matches("router.set_jira(").count(), 1);
    assert_eq!(lib.matches("router.set_github(").count(), 1);
    let both = lib.find("(Ok(jira), Ok(gh)) =>").expect("둘 다 성공한 분기");
    assert!(both < jira && both < github && github < api);
    // 토큰 사본은 Rust 가 GitHub 를 처리할 때만 사이드카로 보낸다(스위치가 꺼져 있으면 서비스가 없으니 보낼 토큰도 없다)
    assert!(lib.contains("if let (true, Some(gh)) = (rust_api, gh_service)"));
    assert_eq!(lib.matches("push_gh_token(").count(), 2, "초기 전송과 인증 변경 콜백 두 곳");
    // 사이드카를 manage 한 뒤에 초기 사본을 보낸다(그 전의 변경은 콜백이 사이드카를 못 찾아 놓칠 수 있으므로)
    assert!(lib.find("Sidecar::spawn(").unwrap() < lib.find("(rust_api, gh_service)").unwrap());
    // 한도 해제 시각은 로컬 시간대 표시를 쓴다(서비스 생성자)
    assert!(prod_code("src/github/service.rs").contains("with_format_time(super::local_time::format_epoch_sec)"));
}

#[test]
fn 사이드카_env_는_항상_직접_정한다() {
    let sidecar = prod_code("src/sidecar.rs");
    assert!(sidecar.contains("cmd.env(\"ANTLER_RUST_API\", \"1\")"));
    // 값이 없을 때 부모의 env 를 지운다(되돌리기가 뒤집히지 않게)
    assert!(sidecar.contains("cmd.env_remove(\"ANTLER_RUST_API\")"));
    // 사이드카에 env 로 가는 것은 이 넷뿐이다 — 토큰은 env 에 없다(stdin 의 ghToken 줄로만 간다)
    let envs: Vec<&str> = sidecar.lines().filter(|l| l.contains("cmd.env(") || l.contains(".env(") || l.contains("env_remove(")).collect();
    for l in &envs {
        assert!(
            ["ANTLER_USER_DATA", "ANTLER_RUST_FETCH_KINDS", "ANTLER_RUST_API"].iter().any(|n| l.contains(n)),
            "사이드카에 알 수 없는 env 를 넘긴다: {l}"
        );
    }
    assert!(!sidecar.to_lowercase().contains("token") || !envs.iter().any(|l| l.to_lowercase().contains("token")));
    // 토큰 줄은 한 곳(push_gh_token)에서만 만든다
    assert_eq!(sidecar.matches("gh_token_line(").count(), 1);
}

#[test]
fn 운영_코드에_시험_훅_호출이_없다() {
    for rel in ["api_host.rs", "router.rs", "lib.rs", "sidecar.rs", "repoq.rs", "github/local_time.rs"] {
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
    assert_eq!(host.matches("opener(&href)").count(), 1, "Jira·GitHub 가 같은 열기 자리를 쓴다");
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

/// 사이드카에 묻지 않는 저장소 정보(저장소도 클라이언트 ID 도 없다) — 오프라인 호출이 사이드카를 필요로 하지 않게
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

/// 둘 다 주입한 라우터(앱이 만드는 모양)
fn router_with_both() -> Router {
    let r = Router::new(true, false, RepoMirror::default());
    r.set_jira(host());
    r.set_github(gh_host());
    r
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
        // gh:* — 로그아웃 상태·잘못된 인자·없는 저장소 정보라 HTTP 요청이 나가지 않는다
        ("gh:auth", json!([])),
        ("gh:loginPat", json!(["has space"])),
        ("gh:importCliToken", json!([])),
        ("gh:deviceStart", json!([])),
        ("gh:devicePoll", json!(["poll-1"])),
        ("gh:logout", json!([])),
        ("gh:pulls", json!(["r1", { "state": "open", "page": 1 }])),
        ("gh:pullsForKey", json!(["r1", "ABC-1"])),
        ("gh:pull", json!(["r1", 1])),
        ("gh:pullFiles", json!(["r1", 1])),
        ("gh:pullCommits", json!(["r1", 1])),
        ("gh:issues", json!(["r1", { "state": "open", "page": 1 }])),
        ("gh:issue", json!(["r1", 1])),
        ("gh:comments", json!(["r1", 1])),
        ("gh:filters", json!(["r1"])),
        ("gh:avatars", json!(["r1", "av-1", []])),
        ("gh:commitChecks", json!(["r1", ["a".repeat(40)]])),
        ("gh:commitCheckRuns", json!(["r1", "a".repeat(40)])),
        ("gh:runs", json!(["r1", 1])),
        ("gh:runJobs", json!(["r1", 1])),
        ("gh:openExternal", json!(["https://example.com/"])),
        ("gh:launchpad", json!(["lp-1"])),
        ("gh:accessibleRepos", json!(["ar-1"])),
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
    assert!(!router.api_active() && !router.github_active(), "서비스가 주입되기 전에는 활성이 아니다");
    router.set_jira(host());
    router.set_github(gh_host());
    assert!(router.api_active() && router.github_active());
    for (ch, args) in offline_calls() {
        let Routed::Done(env) = router.route(ch, &args).await else { panic!("{ch} 가 사이드카로 갔다") };
        assert!(env["ok"].is_boolean(), "{ch}: {env}");
    }
    assert_eq!(router.rust_handled(), API_CHANNELS.len() as u64);
}

#[tokio::test]
async fn 연결하지_않으면_조회는_jira_auth_이고_검증_실패는_error_다() {
    let router = router_with_both();
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
    off.set_github(gh_host());
    assert!(!off.api_active() && !off.github_active());
    // ANTLER_RUST_GIT=0 에 해당(라우터 전체가 꺼진다)
    let git_off = Router::new(false, false, RepoMirror::default());
    git_off.set_jira(host());
    git_off.set_github(gh_host());
    assert!(!git_off.api_active() && !git_off.github_active());
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
    assert!(r.api_enabled() && !r.api_active() && !r.github_active());
    for (ch, args) in offline_calls() {
        assert!(matches!(r.route(ch, &args).await, Routed::Sidecar), "{ch}");
    }
}

#[tokio::test]
async fn 한쪽_서비스만_있으면_활성이_아니고_둘_다_사이드카가_처리한다() {
    // 사이드카 env 하나가 jira·gh 를 함께 넘기므로 한쪽만 있으면 api_active=false 이고(env 를 주지 않는다) 두 계열 모두 사이드카다
    let only_jira = Router::new(true, false, RepoMirror::default());
    only_jira.set_jira(host());
    let only_gh = Router::new(true, false, RepoMirror::default());
    only_gh.set_github(gh_host());
    for r in [&only_jira, &only_gh] {
        assert!(!r.api_active());
        assert!(!r.claim_api());
        for (ch, args) in offline_calls() {
            assert!(matches!(r.route(ch, &args).await, Routed::Sidecar), "{ch}");
        }
        assert_eq!(r.rust_handled(), 0);
    }
}

#[tokio::test]
async fn 둘_다_있으면_claim_api_가_참이고_채널을_rust_가_처리한다() {
    let r = router_with_both();
    assert!(r.api_active() && r.claim_api());
    assert!(matches!(r.route("gh:auth", &json!([])).await, Routed::Done(_)));
    assert!(matches!(r.route("jira:auth", &json!([])).await, Routed::Done(_)));
}

#[tokio::test]
async fn 배열이_아닌_인자와_login_pat_의_토큰은_사이드카로_가지_않는다() {
    // 서비스가 있는 소유 상태(둘 다 사이드카 아님)와 한쪽만 있는 상태 모두에서: 배열이 아닌 인자는 Done(검증 거부)이어야 하고, 사이드카로는 토큰이 가지 않는다
    let r = router_with_both();
    assert!(r.claim_api());
    for args in [json!("ghp_secret"), json!({"token": "ghp_secret"}), json!(null), json!(5)] {
        for ch in ["gh:loginPat", "jira:connect"] {
            let Routed::Done(env) = r.route(ch, &args).await else { panic!("{ch} {args} 가 사이드카로 갔다") };
            assert_eq!(env["ok"], false, "{ch}: {env}");
            assert!(!env.to_string().contains("ghp_secret"), "{env}");
        }
    }
    // 배열 인자의 loginPat 토큰도 Rust 서비스가 받는다(사이드카 stdin 으로 가지 않는다)
    for token in ["has space", "ghp_secret bad"] {
        let Routed::Done(env) = r.route("gh:loginPat", &json!([token])).await else { panic!("gh:loginPat 이 사이드카로 갔다") };
        assert_eq!(env["code"], "ERROR");
        assert!(!env.to_string().contains("secret"), "{env}");
    }
}

#[tokio::test]
async fn gh_repo_는_둘_다_켜져_있어도_사이드카가_처리한다() {
    let r = router_with_both();
    for args in [json!(["r1"]), json!([]), json!("r1"), json!(null)] {
        assert!(matches!(r.route("gh:repo", &args).await, Routed::Sidecar), "{args}");
    }
    assert_eq!(r.rust_handled(), 0);
}

#[tokio::test]
async fn gh_채널의_봉투는_코드_순서를_지킨다() {
    let r = router_with_both();
    let call = |ch: &'static str, args: Value| {
        let r = &r;
        async move {
            let Routed::Done(env) = r.route(ch, &args).await else { panic!("{ch}") };
            env
        }
    };
    // 로그아웃 상태: 상태만(토큰 필드 없음)
    let auth = call("gh:auth", json!([])).await;
    assert_eq!(auth["ok"], true);
    assert_eq!(auth["data"]["signedIn"], false);
    assert!(!auth.to_string().to_lowercase().contains("token"));
    // 검증 실패·저장소 정보 없음·클라이언트 ID 없음·gh 없음은 ERROR(코드는 ERROR 이고 문구가 있다)
    for (ch, args) in [("gh:loginPat", json!(["has space"])), ("gh:pull", json!(["r1", 1])), ("gh:deviceStart", json!([])), ("gh:importCliToken", json!([])), ("gh:devicePoll", json!(["p"]))] {
        let env = call(ch, args).await;
        assert_eq!((env["ok"].clone(), env["code"].clone()), (json!(false), json!("ERROR")), "{ch}: {env}");
        assert!(env["error"].as_str().is_some_and(|m| !m.is_empty()), "{ch}");
    }
    // void 채널: 열 수 없는 주소는 ERROR
    assert_eq!(call("gh:openExternal", json!(["https://example.com/"])).await["code"], "ERROR");
    // 로그인하지 않은 Launchpad·접근 가능 저장소는 needsLogin
    assert_eq!(call("gh:launchpad", json!(["lp"])).await["data"]["needsLogin"], true);
    assert_eq!(call("gh:accessibleRepos", json!(["ar"])).await["data"]["needsLogin"], true);
    // 없는 인자(표식)는 undefined 다
    assert_eq!(call("gh:loginPat", json!([{ antler_lib::router::UNDEFINED_ARG: true }])).await["code"], "ERROR");
}

#[tokio::test]
async fn 배열이_아닌_인자의_gh_호출도_사이드카로_보내지_않는다() {
    // 토큰을 담은 인자가 사이드카 stdin 으로 가지 않게 — 서비스 검증이 거부한다(jira 와 같은 보안 수정)
    let r = router_with_both();
    let bad = [json!({"token": "SECRET-ghp_abcdef"}), json!("SECRET-ghp_abcdef"), json!(null), json!(5)];
    for ch in ["gh:loginPat", "gh:auth", "gh:pull", "gh:devicePoll", "gh:openExternal", "gh:avatars", "gh:launchpad"] {
        for args in &bad {
            let Routed::Done(env) = r.route(ch, args).await else { panic!("{ch} 가 사이드카로 갔다: {args}") };
            assert!(!env.to_string().contains("SECRET"), "{ch}");
        }
    }
}

#[tokio::test]
async fn gh_채널은_스위치가_꺼지면_사이드카로_간다() {
    let off = Router::new(true, false, RepoMirror::default()).with_api(false);
    off.set_github(gh_host());
    for (ch, args) in offline_calls().into_iter().filter(|(c, _)| c.starts_with("gh:")) {
        assert!(matches!(off.route(ch, &args).await, Routed::Sidecar), "{ch}");
    }
}

#[tokio::test]
async fn req_cancel_은_jira_가_켜져_있어도_사이드카에_전달한다() {
    let r = router_with_both();
    assert!(matches!(r.route("req:cancel", &json!(["x"])).await, Routed::Sidecar));
    assert!(matches!(r.route("req:cancel", &json!([])).await, Routed::Sidecar));
}

#[tokio::test]
async fn 배열이_아닌_인자의_jira_호출도_사이드카로_보내지_않는다() {
    // 토큰을 담은 객체 인자가 사이드카 stdin 으로 가지 않게 — 서비스 검증이 거부한다(보안 리뷰)
    let r = router_with_both();
    let bad = json!({"kind": "cloud", "siteUrl": "https://x.atlassian.net", "email": "a@b.c", "token": "SECRET"});
    for ch in ["jira:connect", "jira:issue", "jira:auth"] {
        let Routed::Done(env) = r.route(ch, &bad).await else { panic!("{ch} 가 사이드카로 갔다") };
        assert!(!env.to_string().contains("SECRET"), "{ch}");
    }
}

#[tokio::test]
async fn 등록_전에_온_cancel_은_조회를_취소하지만_전환은_취소하지_않는다() {
    let r = router_with_both();
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
    let r = router_with_both();
    for ch in ["gh:repo", "git:refs", "repos:list", "app:version"] {
        assert!(matches!(r.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
    }
    assert_eq!(r.rust_handled(), 0);
}
