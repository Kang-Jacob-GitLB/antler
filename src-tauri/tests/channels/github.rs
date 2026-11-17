//! `gh:*` 채널 골든(Phase 4 PR 9). TS(`tests/goldenGithubChannels.test.ts`)가 `tests/fixtures/net/github.scenarios.json` 으로 만든 `tests/golden/channels/github.json` 을
//! Rust 라우터(`Router::route` → `api_host`)가 같은 시나리오·같은 목 서버 규칙으로 재현하는지 본다. 비교하는 것은 단계마다 봉투(오류는 code 만)와
//! 서버가 받은 요청 목록(method·path·정렬한 query·Authorization 종류·본문)이다. 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenGithubChannels.test.ts`.
//!
//! 서버는 127.0.0.1 의 목 서버뿐이다(외부 접속 없음) — `api.github.com`·`github.com` 으로 가는 요청은 `*_for_test` 훅이 같은 목 서버로 보낸다.
//! 사이드카에 묻는 저장소 정보(`RepoQuery`)와 gh CLI(`ExternalTokenProvider`)는 시나리오의 `fake` 로 대신한다.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use antler_lib::api_host::GithubHost;
use antler_lib::github::client::{GitHubClient, ANON_FRESH};
use antler_lib::github::device_flow::DeviceFlow;
use antler_lib::github::ports::{ExternalTokenError, ExternalTokenProvider, GhRepoRef, RepoQuery};
use antler_lib::github::service::{GitHubService, ServiceOptions};
use antler_lib::jira::service::{Clock, Sleeper, SystemClock};
use antler_lib::mirror::RepoMirror;
use antler_lib::net::redact::SecretString;
use antler_lib::router::{Routed, Router};
use antler_lib::secret::memory::MemoryStore;
use antler_lib::secret::Vault;
use futures_util::future::{BoxFuture, FutureExt};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::mock::{norm, parse_routes, request_json, short, sorted, substitute, Mock};
use super::read_json;

// ── 사이드카·gh 의 가짜 ─────────────────────────────────────────────────

/// 시나리오의 `fake`(단계의 `fake` 가 키 단위로 덮는다)
#[derive(Clone)]
struct FakeState(Value);

impl FakeState {
    fn merge(&mut self, patch: &Value) {
        if let Some(o) = patch.as_object() {
            for (k, v) in o {
                self.0[k] = v.clone();
            }
        }
    }

    fn ref_of(v: &Value) -> Option<GhRepoRef> {
        let o = v.as_object()?;
        Some(GhRepoRef { owner: o["owner"].as_str()?.into(), repo: o["repo"].as_str()?.into(), remote: o["remote"].as_str()?.into() })
    }
}

#[derive(Clone)]
struct FakeRepos(Arc<Mutex<FakeState>>);

impl RepoQuery for FakeRepos {
    fn gh_repo<'a>(&'a self, repo_id: &'a str) -> BoxFuture<'a, Result<Option<GhRepoRef>, String>> {
        async move { Ok(FakeState::ref_of(&self.0.lock().unwrap().0["repos"][repo_id]["ref"])) }.boxed()
    }

    fn gh_remotes<'a>(&'a self, repo_id: &'a str, _cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<GhRepoRef>, String>> {
        async move {
            let st = self.0.lock().unwrap();
            Ok(st.0["repos"][repo_id]["remotes"].as_array().map(|a| a.iter().filter_map(FakeState::ref_of).collect()).unwrap_or_default())
        }
        .boxed()
    }

    fn pushed_shas_by_author<'a>(&'a self, repo_id: &'a str, _email: &'a str, limit: usize, _cancel: CancellationToken) -> BoxFuture<'a, Result<Vec<String>, String>> {
        async move {
            let st = self.0.lock().unwrap();
            Ok(st.0["repos"][repo_id]["pushed"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).take(limit).collect()).unwrap_or_default())
        }
        .boxed()
    }

    fn invalidate_gh_remotes(&self) {}

    fn github_client_id(&self) -> BoxFuture<'_, Option<String>> {
        // TS: 앞뒤 공백을 떼고 비면 없는 것
        async move { self.0.lock().unwrap().0["clientId"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) }.boxed()
    }

    fn repo_ids(&self) -> BoxFuture<'_, Vec<String>> {
        async move { self.0.lock().unwrap().0["repoIds"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default() }.boxed()
    }
}

struct FakeCli(Arc<Mutex<FakeState>>);

impl ExternalTokenProvider for FakeCli {
    fn available(&self) -> BoxFuture<'_, bool> {
        async move { self.0.lock().unwrap().0["cli"]["available"] == true }.boxed()
    }

    fn token(&self) -> BoxFuture<'_, Result<SecretString, ExternalTokenError>> {
        async move {
            let st = self.0.lock().unwrap();
            let cli = &st.0["cli"];
            match cli["error"].as_str() {
                Some(message) => Err(ExternalTokenError { message: message.to_string() }),
                None => Ok(SecretString::new(cli["token"].as_str().unwrap_or(""))),
            }
        }
        .boxed()
    }
}

/// 기다리지 않는 대기(Device Flow 의 간격) — 시나리오가 간격만큼 실제로 기다리지 않게 한다
struct NoSleep;

impl Sleeper for NoSleep {
    fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
        async {}.boxed()
    }
}

fn service(base: &str, vault: Arc<Vault>, state: Arc<Mutex<FakeState>>) -> GitHubService {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let flow = DeviceFlow::new_for_test(base, Duration::from_secs(5), Arc::new(NoSleep), clock.clone()).expect("Device Flow");
    let base = base.to_string();
    GitHubService::with_parts(
        vault,
        Arc::new(FakeRepos(state.clone())),
        Some(Arc::new(FakeCli(state))),
        move |p| GitHubClient::new_for_test(p, &base, Duration::from_secs(5), ANON_FRESH),
        flow,
        clock,
        ServiceOptions::default(),
    )
    .expect("GitHub 서비스")
}

fn router_with(base: &str, vault: Arc<Vault>, state: Arc<Mutex<FakeState>>, opened: Arc<Mutex<Vec<String>>>) -> (Router, GitHubService) {
    let router = Router::new(true, false, RepoMirror::default());
    let opener: antler_lib::api_host::Opener = Arc::new(move |url: &str| {
        opened.lock().unwrap().push(url.to_string());
        Ok(())
    });
    let svc = service(base, vault, state);
    router.set_github(GithubHost { service: svc.clone(), opener });
    (router, svc)
}

/// 시계에서 나오는 값(`fetchedAt`)은 골든에서 가린다
fn mask_time(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(mask_time).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), if k == "fetchedAt" { json!("<time>") } else { mask_time(x) })).collect()),
        other => other.clone(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn github_채널_시나리오가_ts_골든과_같다() {
    let sc = read_json("tests/fixtures/net/github.scenarios.json");
    let golden = read_json("tests/golden/channels/github.json");
    let steps = sc["steps"].as_array().expect("steps");
    let gsteps = golden["steps"].as_array().expect("골든 steps");
    assert_eq!(steps.len(), gsteps.len(), "단계 수가 골든과 다르다 — UPDATE_GOLDEN=1 npx vitest run tests/goldenGithubChannels.test.ts 로 골든을 다시 만든다");

    let mock = Mock::start(parse_routes(&sc["routes"]), json!({ "message": "Not Found" }));
    let state = Arc::new(Mutex::new(FakeState(sc["fake"].clone())));
    let vault = Arc::new(Vault::new(Arc::new(MemoryStore::standing_in_for_keychain())));
    let opened: Arc<Mutex<Vec<String>>> = Arc::default();
    let (mut router, mut svc) = router_with(&mock.origin, vault.clone(), state.clone(), opened.clone());
    let unsub = |s: &str| s.replace(&mock.origin, "{base}");

    let mut diffs = Vec::new();
    for (step, g) in steps.iter().zip(gsteps) {
        let name = step["name"].as_str().unwrap();
        assert_eq!(g["name"], step["name"], "{name}: 골든의 단계 순서가 다르다");
        state.lock().unwrap().merge(&step["fake"]);
        if step["restart"] == true {
            // 앱을 다시 켠다 — 같은 보관소(키체인 대역)에서 서비스를 새로 만들어 로그인을 복원한다
            svc.dispose();
            (router, svc) = router_with(&mock.origin, vault.clone(), state.clone(), opened.clone());
        }
        mock.table.lock().unwrap().reset_step(parse_routes(&step["routes"]));
        let from = mock.seen.lock().unwrap().len();
        opened.lock().unwrap().clear();
        let ch = step["ch"].as_str().unwrap();
        assert_ne!(ch, "gh:repo", "{name}: gh:repo 는 사이드카가 처리한다");
        let args = substitute(&step["args"], &mock.origin, "");
        let cancel = step["cancel"]["afterRequests"].as_u64().zip(step["cancel"]["id"].as_str().map(String::from));

        let call = router.route(ch, &args);
        let routed = match &cancel {
            Some((requests, id)) => {
                let canceller = async {
                    // 시간이 아니라 목 서버가 요청을 받은 뒤에 보낸다(느린 러너에서도 요청 목록이 같다). 최대 10초
                    let deadline = std::time::Instant::now() + Duration::from_secs(10);
                    while mock.seen.lock().unwrap().len() < from + *requests as usize && std::time::Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    // 렌더러의 req:cancel 과 같은 길(라우터가 레지스트리와 서비스를 모두 취소하고 사이드카에도 넘긴다)
                    assert!(matches!(router.route("req:cancel", &json!([id])).await, Routed::Sidecar));
                };
                tokio::join!(call, canceller).0
            }
            None => call.await,
        };
        let Routed::Done(env) = routed else { panic!("{name}: gh 채널이 사이드카로 갔다") };
        let envelope = if env["ok"] == true {
            let mut o = json!({ "ok": true });
            if let Some(d) = env.get("data") {
                o["data"] = mask_time(d);
            }
            o
        } else {
            json!({ "ok": false, "code": env["code"] })
        };
        let envelope: Value = serde_json::from_str(&unsub(&envelope.to_string())).unwrap();
        let requests = sorted(mock.seen.lock().unwrap()[from..].iter().map(request_json).collect());
        let mut got = json!({ "name": name, "envelope": envelope, "requests": requests });
        if ch == "gh:openExternal" {
            got["opened"] = json!(opened.lock().unwrap().iter().map(|u| unsub(u)).collect::<Vec<_>>());
        }
        let mut want = g.clone();
        want["requests"] = Value::Array(sorted(g["requests"].as_array().unwrap().clone()));
        if norm(&got) != norm(&want) {
            diffs.push(format!("{name}: 골든과 다르다\n  Rust: {}\n  TS  : {}", short(&got), short(&want)));
        }
    }
    svc.dispose();
    assert!(diffs.is_empty(), "{}", diffs.join("\n"));
}
