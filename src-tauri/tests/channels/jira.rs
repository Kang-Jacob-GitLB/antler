//! `jira:*` 채널 골든(Phase 4 PR 6). TS(`tests/goldenJiraChannels.test.ts`)가 `tests/fixtures/net/jira.scenarios.json` 으로 만든 `tests/golden/channels/jira.json` 을
//! Rust 라우터(`Router::route` → `api_host`)가 같은 시나리오·같은 목 서버 규칙으로 재현하는지 본다. 비교하는 것은 단계마다 봉투(오류는 code 만)와
//! 서버가 받은 요청 목록(method·path·정렬한 query·Authorization 종류·본문)이다. 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts`.
//!
//! 서버는 127.0.0.1 의 목 서버뿐이다(외부 접속 없음). `test-hooks` feature 가 있어야 루프백 http 를 허용하는 클라이언트·사이트 옵션을 만들 수 있다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::api_host::{GithubHost, JiraHost};
use antler_lib::github::ports::{GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use futures_util::future::{BoxFuture, FutureExt};
use tokio_util::sync::CancellationToken;
use antler_lib::jira::client::JiraClient;
use antler_lib::jira::service::{JiraService, SystemClock, TokioSleeper};
use antler_lib::jira::validate::SiteOptions;
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{Routed, Router};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use serde_json::{json, Value};

use super::mock::{closed_origin, norm, parse_routes, request_json, short, sorted, substitute, Mock};
use super::read_json;

fn service(vault: Arc<Vault>) -> JiraService {
    let client = JiraClient::new_for_test(Duration::from_secs(5), None, Duration::from_millis(1)).expect("클라이언트");
    JiraService::with_parts(vault, client, SiteOptions::insecure_loopback_for_test(), Arc::new(SystemClock), Arc::new(TokioSleeper), Duration::from_millis(4000))
}

fn router_with(vault: Arc<Vault>, opened: Arc<Mutex<Vec<String>>>) -> Router {
    let router = Router::new(true, false, RepoMirror::default());
    let opener: antler_lib::api_host::Opener = Arc::new(move |url: &str| {
        opened.lock().unwrap().push(url.to_string());
        Ok(())
    });
    router.set_jira(JiraHost { service: service(vault), opener });
    // 앱은 Jira·GitHub 서비스가 둘 다 있을 때만 Rust 가 처리한다(`api_active`). GitHub 쪽은 쓰지 않는 서비스다
    let idle = GitHubService::new(Arc::new(Vault::new(Arc::new(MemoryStore::new()))), Arc::new(NoRepos), None, ServiceOptions::default()).expect("GitHub 서비스");
    router.set_github(GithubHost { service: idle, opener: Arc::new(|_| Ok(())) });
    router
}

/// 저장소·클라이언트 ID 가 없는 저장소 정보(쓰지 않는 GitHub 서비스용)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jira_채널_시나리오가_ts_골든과_같다() {
    let sc = read_json("tests/fixtures/net/jira.scenarios.json");
    let golden = read_json("tests/golden/channels/jira.json");
    let steps = sc["steps"].as_array().expect("steps");
    let gsteps = golden["steps"].as_array().expect("골든 steps");
    assert_eq!(steps.len(), gsteps.len(), "단계 수가 골든과 다르다 — UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts 로 골든을 다시 만든다");

    let mock = Mock::start(parse_routes(&sc["routes"]), json!({ "errorMessages": ["nf"] }));
    let closed = closed_origin();
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::standing_in_for_keychain())));
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut router = router_with(vault.clone(), opened.clone());
    let unsub = |s: &str| s.replace(&mock.origin, "{base}").replace(&closed, "{closed}");

    let mut diffs = Vec::new();
    for (step, g) in steps.iter().zip(gsteps) {
        let name = step["name"].as_str().unwrap();
        assert_eq!(g["name"], step["name"], "{name}: 골든의 단계 순서가 다르다");
        if step["restart"] == true {
            // 앱을 다시 켠다 — 같은 보관소(키체인 대역)에서 서비스를 새로 만들어 연결을 복원한다
            router = router_with(vault.clone(), opened.clone());
        }
        mock.table.lock().unwrap().reset_step(parse_routes(&step["routes"]));
        let from = mock.seen.lock().unwrap().len();
        opened.lock().unwrap().clear();
        let ch = step["ch"].as_str().unwrap();
        let args = substitute(&step["args"], &mock.origin, &closed);
        let cancel_id = step["cancelAfterMs"].as_u64().and_then(|ms| args.get(1).and_then(Value::as_str).map(|id| (ms, id.to_string())));

        let call = router.route(ch, &args);
        let routed = match &cancel_id {
            Some((ms, id)) => {
                let canceller = async {
                    tokio::time::sleep(Duration::from_millis(*ms)).await;
                    // 렌더러의 req:cancel 과 같은 길(라우터가 레지스트리와 서비스를 모두 취소하고 사이드카에도 넘긴다)
                    assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
                };
                tokio::join!(call, canceller).0
            }
            None => call.await,
        };
        let Routed::Done(env) = routed else { panic!("{name}: jira 채널이 사이드카로 갔다") };
        let envelope = if env["ok"] == true {
            let mut o = json!({ "ok": true });
            if let Some(d) = env.get("data") {
                o["data"] = d.clone();
            }
            o
        } else {
            json!({ "ok": false, "code": env["code"] })
        };
        let envelope: Value = serde_json::from_str(&unsub(&envelope.to_string())).unwrap();
        let requests = sorted(mock.seen.lock().unwrap()[from..].iter().map(request_json).collect());
        let mut got = json!({ "name": name, "envelope": envelope, "requests": requests });
        if ch == "jira:openExternal" {
            got["opened"] = json!(opened.lock().unwrap().iter().map(|u| unsub(u)).collect::<Vec<_>>());
        }
        let mut want = g.clone();
        want["requests"] = Value::Array(sorted(g["requests"].as_array().unwrap().clone()));
        if norm(&got) != norm(&want) {
            diffs.push(format!("{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&want)));
        }
    }
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}
