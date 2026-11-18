//! `jira:*`(PR 6)·`gh:*`(PR 9, `gh:repo` 제외) 채널의 Rust 쪽 처리. 라우터([`crate::router::Router`])가 `API_CHANNELS`(router.rs)를 이 모듈로 넘기면
//! `src/main/ipc/jira.ts`·`github.ts` 의 핸들러와 `src/main/ipc/handlers.ts` 의 `handle()`(봉투) 과 같은 모양으로 [`JiraService`]·[`GitHubService`] 를 부르고 IpcEnvelope 를 돌려준다.
//! 아래 설명은 Jira 기준이고, GitHub([`gh_envelope`])는 취소를 서비스가 requestId 로 직접 한다는 점만 다르다(함수 문서).
//!
//! - 인자 검증은 서비스가 한다(`JiraService` 의 `parse_*` — TS `JiraService` 와 같은 검증). 이 파일은 requestId(`typeof === 'string'`)만 가려낸다.
//! - 오류 봉투의 코드 순서는 `handle()` 과 같다: 취소 `ABORTED` → Jira API 오류 `JIRA_*` → 그 밖의 `ERROR`. 문구는 서비스가 비밀을 가린 것이다.
//! - 취소: requestId 는 라우터의 [`CancelRegistry`] 에 등록한다(cancel 이 등록보다 먼저 와도 tombstone 이 기억한다). 취소되면 서비스 호출 future 를 버려 요청을 끊는다.
//!   `jira:transition` 은 requestId 를 받지 않는다 — 도중에 끊으면 적용 여부를 알 수 없게 되므로 연결이 바뀔 때만 중단된다.
//! - `jira:openExternal`: 연결한 사이트의 주소로 검증된 것만([`JiraService::external_url`]) [`Opener`] 에 넘긴다. 열기 자체는 앱이 주입한다(테스트는 기록만 한다).
//!
//! 이 모듈은 `AppHandle`·`Sidecar` 에 기대지 않는다 — 통합 테스트(`tests/channels.rs`)가 그대로 돌릴 수 있다.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::github::error::GhError;
use crate::github::service::GitHubService;
use crate::jira::service::{JiraError, JiraService};
use crate::net::js_value::stringify;
use crate::net::json::Json;
use crate::router::{is_request_id, CancelRegistry};

/// 검증을 마친 주소를 기본 브라우저로 연다. 앱이 주입한다(`lib.rs`)
pub type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Jira 서비스와 주소 여는 함수. 라우터가 하나만 가진다
#[derive(Clone)]
pub struct JiraHost {
    pub service: JiraService,
    pub opener: Opener,
}

/// 서비스 호출의 결과. `None` 은 TS 의 `void`(봉투에 `data` 가 없다)
type Outcome = Result<Option<Value>, JiraError>;

const OPEN_DENIED: &str = "연결한 Jira 사이트의 주소만 열 수 있습니다";

fn aborted() -> JiraError {
    JiraError { code: "ABORTED", status: None, message: "aborted".to_string() }
}

fn to_value<T: serde::Serialize>(v: &T) -> Outcome {
    serde_json::to_value(v).map(Some).map_err(|_| JiraError::plain("응답을 만들지 못했습니다"))
}

/// 키 순서를 지키는 [`Json`] 을 IPC 값으로
fn json_value(j: &Json) -> Outcome {
    serde_json::from_str::<Value>(&stringify(j)).map(Some).map_err(|_| JiraError::plain("응답을 만들지 못했습니다"))
}

fn envelope(outcome: Outcome) -> Value {
    match outcome {
        Ok(Some(data)) => json!({ "ok": true, "data": data }),
        Ok(None) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.message, "code": e.code }),
    }
}

/// requestId 를 받는 채널의 인자 자리(`jira.ts`: 둘째 인자)
fn request_id_slot(channel: &str) -> Option<usize> {
    matches!(channel, "jira:issue" | "jira:transitions" | "jira:comments" | "jira:dev" | "jira:search" | "jira:worklogs" | "jira:userSearch").then_some(1)
}

/// 없는 자리(undefined)는 `null` 이다 — 서비스의 검증이 TS 의 `undefined` 와 같게 거부한다
fn slot(args: &[Option<Value>], i: usize) -> &Value {
    static NULL: Value = Value::Null;
    args.get(i).and_then(Option::as_ref).unwrap_or(&NULL)
}

/// `jira:*` 채널 하나를 처리해 IpcEnvelope 를 돌려준다. `args` 는 표식을 되돌린 인자 배열(`None` = undefined)이다.
pub async fn jira_envelope(host: JiraHost, cancels: Arc<CancelRegistry>, channel: String, args: Vec<Option<Value>>) -> Value {
    let (a0, a1) = (slot(&args, 0), slot(&args, 1));
    // TS: `typeof requestId === 'string' ? requestId : undefined`. 서비스는 빈 문자열을 없는 것으로 본다
    let request_id: Option<&str> = if request_id_slot(&channel).is_some() { a1.as_str().filter(|id| !id.is_empty()) } else { None };
    // 첫 await 전에 등록한다 — 뒤따르는 req:cancel 이 먼저 도착해도 tombstone 이 받아 준다
    let reg = request_id.filter(|id| is_request_id(id)).map(|id| cancels.register(id));
    let token = reg.as_ref().map_or_else(CancellationToken::new, |r| r.token.clone());
    let svc = &host.service;
    let call = async {
        match channel.as_str() {
            "jira:auth" => to_value(&svc.auth()),
            "jira:connect" => svc.connect(a0).await.and_then(|s| to_value(&s)),
            "jira:disconnect" => svc.disconnect().await.and_then(|s| to_value(&s)),
            "jira:issue" => svc.issue(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:transitions" => svc.transitions(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:transition" => svc.transition(a0, a1).await.map(|()| None),
            "jira:comments" => svc.comments(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:dev" => svc.dev_info(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:search" => svc.search(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:worklogs" => svc.worklogs(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:userSearch" => svc.user_search(a0, request_id).await.and_then(|j| json_value(&j)),
            "jira:avatar" => Ok(Some(svc.avatar(a0).await.map_or(Value::Null, Value::String))),
            "jira:openExternal" => open_external(&host, a0).await,
            other => Err(JiraError::plain(format!("알 수 없는 채널: {other}"))),
        }
    };
    let outcome = if reg.is_some() {
        tokio::select! {
            biased;
            () = token.cancelled() => Err(aborted()),
            r = call => r,
        }
    } else {
        call.await
    };
    drop(reg);
    envelope(outcome)
}

/// `jira:openExternal`: 연결한 Jira 사이트의 주소만 연다(검증은 서비스의 `external_url`)
async fn open_external(host: &JiraHost, raw: &Value) -> Outcome {
    let href = host.service.external_url(raw);
    open_href(&host.opener, href, OPEN_DENIED).await.map(|()| None).map_err(JiraError::plain)
}

/// 서비스가 검증한 주소(`None` 이면 거부)를 연다. Jira·GitHub 가 같은 열기 자리를 쓴다 — 열기는 앱이 주입한 [`Opener`] 한 곳이다
async fn open_href(opener: &Opener, href: Option<String>, denied: &str) -> Result<(), String> {
    let Some(href) = href else { return Err(denied.to_string()) };
    let opener = opener.clone();
    // 셸 호출은 런타임 스레드를 붙잡지 않게 따로 돌린다
    match tokio::task::spawn_blocking(move || opener(&href)).await {
        Ok(result) => result,
        Err(_) => Err("주소를 열지 못했습니다".to_string()),
    }
}

// ── gh:* ─────────────────────────────────────────────────────────────────────

/// GitHub 서비스와 주소 여는 함수. 라우터가 하나만 가진다
#[derive(Clone)]
pub struct GithubHost {
    pub service: GitHubService,
    pub opener: Opener,
}

type GhOutcome = Result<Option<Value>, GhError>;

const GH_OPEN_DENIED: &str = "GitHub(https://github.com/) 주소만 열 수 있습니다";

fn gh_to_value<T: serde::Serialize>(v: &T) -> GhOutcome {
    serde_json::to_value(v).map(Some).map_err(|_| GhError::plain("응답을 만들지 못했습니다"))
}

fn gh_json_value(j: &Json) -> GhOutcome {
    serde_json::from_str::<Value>(&stringify(j)).map(Some).map_err(|_| GhError::plain("응답을 만들지 못했습니다"))
}

fn gh_envelope_of(outcome: GhOutcome) -> Value {
    match outcome {
        Ok(Some(data)) => json!({ "ok": true, "data": data }),
        Ok(None) => json!({ "ok": true }),
        // 코드 순서는 handlers.ts 의 handle() 과 같다: 취소 ABORTED → GitHub API 오류 GH_* → 그 밖 ERROR (서비스가 이미 이 순서로 코드를 정해 돌려준다)
        Err(e) => json!({ "ok": false, "error": e.message, "code": e.code }),
    }
}

/// `gh:*` 채널 하나를 처리해 IpcEnvelope 를 돌려준다(`gh:repo` 는 git 채널이라 오지 않는다 — Rust 가 맡는 저장소는 라우터가, 캐시 실행기가 없는 github kind·미러에 없는 id 는 풀 수 없다). `args` 는 표식을 되돌린 인자 배열이다.
/// 취소는 서비스가 requestId 로 직접 한다(`req:cancel` → `GitHubService::cancel`) — 로그인·아바타처럼 도중에 끊으면 상태가 어정쩡해지는 호출이 있어 future 를 버리지 않는다
pub async fn gh_envelope(host: GithubHost, channel: String, args: Vec<Option<Value>>) -> Value {
    let (a0, a1, a2) = (slot(&args, 0), slot(&args, 1), slot(&args, 2));
    let svc = &host.service;
    let outcome: GhOutcome = match channel.as_str() {
        "gh:auth" => gh_to_value(&svc.auth().await),
        "gh:loginPat" => svc.login_pat(a0).await.and_then(|s| gh_to_value(&s)),
        "gh:importCliToken" => svc.import_cli_token().await.and_then(|s| gh_to_value(&s)),
        "gh:deviceStart" => svc.device_start().await.and_then(|c| gh_to_value(&c)),
        "gh:devicePoll" => svc.device_poll(a0).await.and_then(|s| gh_to_value(&s)),
        "gh:logout" => svc.logout().await.and_then(|s| gh_to_value(&s)),
        "gh:pulls" => svc.pulls(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:pullsForKey" => svc.pulls_for_key(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:pull" => svc.pull(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:pullFiles" => svc.pull_files(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:pullCommits" => svc.pull_commits(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:issues" => svc.issues(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:issue" => svc.issue(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:comments" => svc.comments(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:filters" => svc.filters(a0).await.and_then(|j| gh_json_value(&j)),
        "gh:avatars" => svc.avatars(a0, a1, a2).await.and_then(|j| gh_json_value(&j)),
        "gh:commitChecks" => svc.commit_checks(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:commitCheckRuns" => svc.commit_check_runs(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:runs" => svc.runs(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:runJobs" => svc.run_jobs(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:launchpad" => svc.launchpad(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:accessibleRepos" => svc.accessible_repos(a0, a1).await.and_then(|j| gh_json_value(&j)),
        "gh:openExternal" => gh_open_external(&host, a0).await,
        other => Err(GhError::plain(format!("알 수 없는 채널: {other}"))),
    };
    gh_envelope_of(outcome)
}

/// `gh:openExternal`: `https://github.com` 의 주소만 연다(검증은 서비스의 `external_url`)
async fn gh_open_external(host: &GithubHost, raw: &Value) -> GhOutcome {
    let href = host.service.external_url(raw);
    open_href(&host.opener, href, GH_OPEN_DENIED).await.map(|()| None).map_err(GhError::plain)
}
