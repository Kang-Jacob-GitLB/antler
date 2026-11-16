//! `jira:*` 채널의 Rust 쪽 처리(Phase 4 PR 6). 라우터([`crate::router::Router`])가 `API_CHANNELS`(router.rs)를 이 모듈로 넘기면
//! `src/main/ipc/jira.ts` 의 핸들러와 `src/main/ipc/handlers.ts` 의 `handle()`(봉투) 과 같은 모양으로 [`JiraService`] 를 부르고 IpcEnvelope 를 돌려준다.
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

/// 서비스 호출의 결과. `None` 은 TS 의 `void`(봉투에 `data` 가 없다 — 사이드카의 JSON 직렬화와 같다)
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
    let Some(href) = host.service.external_url(raw) else { return Err(JiraError::plain(OPEN_DENIED)) };
    let opener = host.opener.clone();
    // 셸 호출은 런타임 스레드를 붙잡지 않게 따로 돌린다
    match tokio::task::spawn_blocking(move || opener(&href)).await {
        Ok(Ok(())) => Ok(None),
        Ok(Err(message)) => Err(JiraError::plain(message)),
        Err(_) => Err(JiraError::plain("주소를 열지 못했습니다")),
    }
}
