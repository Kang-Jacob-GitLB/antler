//! fetch 서비스(`git::fetch::service`)의 앱 쪽 호스트. 이벤트는 renderer 로, 확인 창은 Rust 네이티브 대화상자로, 승인·알림은 사이드카로 보낸다.
//!
//! 보안:
//! - 확인 창은 `dialogs::message_box`(webview HTML 이 아니다)다. 실패·닫기·Esc 는 `None` 이고 서비스의 `ConfirmPlan::decide` 가 거부로 읽는다 — 이 파일이 허용 여부를 정하지 않는다.
//! - **승인을 사이드카에 부탁하는 곳은 [`FetchHost::approve`] 구현인 이 파일의 한 곳뿐이다**(`tests/channels.rs` 가 소스로 강제한다).
//!   인자는 서비스가 두 지점(수동 fetch 성공 뒤·"항상 허용" 뒤)에서 넘긴 값이고, renderer 의 값이 이 경로로 오지 않는다.

use std::future::Future;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::dialogs;
use crate::git::fetch::confirm_text::ConfirmBox;
use crate::git::fetch::service::{ApprovalKind, FetchHost, HostEvent};
use crate::git::log_service::NoteOp;
use crate::mirror::RepoMirror;
use crate::sidecar::Sidecar;

pub struct AppFetchHost {
    app: AppHandle,
    mirror: RepoMirror,
}

impl AppFetchHost {
    pub fn new(app: AppHandle, mirror: RepoMirror) -> Self {
        AppFetchHost { app, mirror }
    }
}

/// 사이드카가 보내는 이벤트(`{t:"evt"}`)와 같은 모양으로 renderer 에 낸다
fn evt(name: &str, payload: Value) -> Value {
    json!({ "name": name, "payload": payload })
}

/// 서비스가 내보내는 이벤트를 renderer 이벤트로 바꾼다
fn host_event(event: &HostEvent) -> Value {
    match event {
        HostEvent::Status(status) => evt("fetch:status", serde_json::to_value(status).unwrap_or(Value::Null)),
        HostEvent::RepoChanged(repo_id) => evt("repo:changed", json!({ "repoId": repo_id })),
    }
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
        if let Some(sidecar) = self.app.try_state::<Sidecar>() {
            sidecar.approve(kind, repo_id, fingerprint);
        }
    }

    fn note(&self, op: NoteOp, repo_id: &str) {
        if let Some(sidecar) = self.app.try_state::<Sidecar>() {
            sidecar.note(op, repo_id);
        }
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
