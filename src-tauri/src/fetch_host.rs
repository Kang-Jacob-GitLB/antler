//! fetch 서비스(`git::fetch::service`)의 앱 쪽 호스트. 이벤트는 renderer 로, 확인 창은 Rust 네이티브 대화상자로, 승인·기본 브랜치는 설정(`ConfigStore`)에, 알림은 사이드카로 보낸다.
//!
//! 보안:
//! - 확인 창은 `dialogs::message_box`(webview HTML 이 아니다)다. 실패·닫기·Esc 는 `None` 이고 서비스의 `ConfirmPlan::decide` 가 거부로 읽는다 — 이 파일이 허용 여부를 정하지 않는다.
//! - **승인을 설정에 쓰는 곳은 [`FetchHost::approve`] 구현인 이 파일의 한 곳뿐이다**(`tests/config_route.rs` 가 소스로 강제한다 — 승인 변경자 `set_fetch_approval`·`set_trusted_config` 는 다른 어디서도 부르지 않는다).
//!   인자는 서비스가 두 지점(수동 fetch 성공 뒤·"항상 허용" 뒤)에서 넘긴 값이고, renderer 의 값이 이 경로로 오지 않는다 — `config:setUi` 는 `ui` 아래로만 쓰므로 승인을 위조할 수 없다.

use std::future::Future;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::config::ConfigStore;
use crate::dialogs;
use crate::git::fetch::confirm_text::ConfirmBox;
use crate::git::fetch::service::{is_fingerprint, ApprovalKind, FetchHost, HostEvent};
use crate::git::github_link::is_safe_branch;
use crate::git::log_service::{NoteOp, NoteSink};
use crate::mirror::RepoMirror;
use crate::net::json::Json;
use crate::router::ChangeSink;

pub struct AppFetchHost {
    app: AppHandle,
    mirror: RepoMirror,
    /// 라우터의 알림 길(`Router::note_sink`) — Rust 의 공유 상태에 먼저 반영하고 사이드카로 보낸다
    notes: NoteSink,
    /// 설정 저장소(Phase 6b PR 9 — Rust 가 설정 파일의 유일한 작성자다). 승인·기본 브랜치를 사이드카에 부탁하지 않고 여기에 직접 쓴다
    store: Arc<ConfigStore>,
}

impl AppFetchHost {
    pub fn new(app: AppHandle, mirror: RepoMirror, notes: NoteSink, store: Arc<ConfigStore>) -> Self {
        AppFetchHost { app, mirror, notes, store }
    }
}

/// 승인을 설정에 쓴다. **승인을 쓰는 곳은 이 함수 하나이고 부르는 곳은 [`FetchHost::approve`] 구현(서비스의 두 지점 — 수동 fetch 성공 뒤·"항상 허용" 뒤)뿐이다**
/// (`tests/config_route.rs` 가 소스로 강제한다 — renderer 의 채널 처리기는 승인 변경자를 부르지 않는다). 검증은 사이드카의 `fetchApprove.ts` 와 같다:
/// 저장소가 설정에 있고 지문이 소문자 hex 64 여야 한다. 쓰기 실패(백신·OneDrive 가 설정 파일을 잡음, 디스크 가득 참)는 "다음에 다시 묻는다"로 끝낸다 — fetch 를 실패시키지 않는다
fn write_approval(store: &ConfigStore, kind: ApprovalKind, repo_id: &str, fingerprint: &str) {
    if !is_fingerprint(fingerprint) || store.repo(repo_id).is_none() {
        return;
    }
    let written = match kind {
        ApprovalKind::Approval => store.set_fetch_approval(repo_id, Some(fingerprint)),
        ApprovalKind::Trusted => store.set_trusted_config(repo_id, fingerprint),
    };
    if let Err(e) = written {
        eprintln!("[fetch] 승인을 저장하지 못했다(다음에 다시 묻는다): {e}");
    }
}

/// 첫 fetch 가 알아낸 GitHub 저장소의 기본 브랜치를 레코드에 남긴다. 저장소가 있고 소스가 github 이고 쓸 수 있는 이름일 때만이다(`ConfigStore` 가 링크·같은 값도 거른다). 실패는 로그만 — 다음 fetch 가 다시 알아낸다
fn write_github_branch(store: &ConfigStore, repo_id: &str, branch: &str) {
    let is_github = store
        .repo(repo_id)
        .and_then(|r| r.get("sourceId").and_then(Json::as_str).and_then(|s| store.source(s)))
        .is_some_and(|s| s.get("kind").and_then(Json::as_str) == Some("github"));
    if !is_github || !is_safe_branch(branch) {
        return;
    }
    if let Err(e) = store.set_github_default_branch(repo_id, branch) {
        eprintln!("[fetch] 원격 저장소 기본 브랜치를 기록하지 못했다: {e}");
    }
}

/// 사이드카가 보내는 이벤트(`{t:"evt"}`)와 같은 모양으로 renderer 에 낸다
fn evt(name: &str, payload: Value) -> Value {
    json!({ "name": name, "payload": payload })
}

/// 저장소의 ref 가 바뀌었다는 renderer 이벤트(fetch 서비스와 감시가 같은 모양을 쓴다)
fn repo_changed_event(repo_id: &str) -> Value {
    evt("repo:changed", json!({ "repoId": repo_id }))
}

/// 서비스가 내보내는 이벤트를 renderer 이벤트로 바꾼다
fn host_event(event: &HostEvent) -> Value {
    match event {
        HostEvent::Status(status) => evt("fetch:status", serde_json::to_value(status).unwrap_or(Value::Null)),
        HostEvent::RepoChanged(repo_id) => repo_changed_event(repo_id),
    }
}

/// 감시(`Router::set_change_sink`)가 낸 변화를 renderer 의 `repo:changed` 로 보낸다
pub fn change_sink(app: AppHandle) -> ChangeSink {
    std::sync::Arc::new(move |repo_id| {
        let _ = app.emit("antler:evt", repo_changed_event(repo_id));
    })
}

impl FetchHost for AppFetchHost {
    fn emit(&self, event: HostEvent) {
        let _ = self.app.emit("antler:evt", host_event(&event));
    }

    fn confirm(&self, _repo_id: &str, confirm: &ConfirmBox) -> impl Future<Output = Option<i64>> + Send {
        let app = self.app.clone();
        let arg = confirm.to_message_box_arg();
        async move {
            // 창이 닫힐 때까지 블로킹이다 — 런타임 스레드를 붙잡지 않게 따로 돌린다. 실패(창 없음·패닉)는 None = 거부
            let shown = tauri::async_runtime::spawn_blocking(move || dialogs::message_box(&app, &arg)).await.ok().flatten();
            shown.and_then(|v| v.get("response").and_then(Value::as_i64))
        }
    }

    fn approve(&self, kind: ApprovalKind, repo_id: &str, fingerprint: &str) {
        write_approval(&self.store, kind, repo_id, fingerprint);
    }

    fn note(&self, op: NoteOp, repo_id: &str) {
        (self.notes)(op, repo_id);
    }

    fn github_branch(&self, repo_id: &str, branch: &str) {
        write_github_branch(&self.store, repo_id, branch);
    }

    fn repo_name(&self, repo_id: &str) -> Option<String> {
        self.mirror.name(repo_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::fetch::service::FetchStatus;

    #[test]
    fn 이벤트는_사이드카의_evt_와_같은_모양이다() {
        let e = host_event(&HostEvent::RepoChanged("r1".into()));
        assert_eq!(e, json!({ "name": "repo:changed", "payload": { "repoId": "r1" } }));
        let status = FetchStatus { repo_id: "r1".into(), fetching: false, last_ok_at: None, error: None, blocked: None, auth_paused: false, needs_manual: false, no_remote: false };
        let s = host_event(&HostEvent::Status(status));
        assert_eq!(s["name"], "fetch:status");
        assert_eq!(s["payload"]["repoId"], "r1");
        assert_eq!(s["payload"]["fetching"], false);
        assert!(s["payload"]["blocked"].is_null());
        assert!(s["payload"]["lastOkAt"].is_null());
    }
}
