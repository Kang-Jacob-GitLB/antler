//! 사이드카가 부탁한 확인 창·폴더/파일 선택을 Rust 네이티브 대화상자로 띄운다(webview HTML 이 아니다).
//! renderer 에는 이 대화상자를 띄우거나 누를 경로가 없다 — 플러그인의 JS command 는 capability 에 주지 않는다.
//! 실패(창 없음·잘못된 인자·닫기·Esc)는 모두 `None` 이고, 사이드카가 이를 취소(cancelId)로 바꾼다.
//! 인자 모양은 src/sidecar/protocol.ts 의 MessageBoxArg·PickPathArg 와 같아야 한다.

use std::path::Path;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

/// 확인 창. 값은 `{ "response": <arg.buttons 의 원래 인덱스> }`
///
/// **취소 버튼(`cancelId`)은 반드시 마지막(플러그인의 "cancel" 자리)에 둔다.** tauri-plugin-dialog 2.8 은 Esc·창 닫기로 생긴
/// `Cancel` 결과를 모든 플랫폼에서 `Custom(cancel 자리의 문구)` 로 바꿔 돌려준다(desktop.rs 의 show_message_dialog).
/// 그 자리에 다른 버튼이 있으면 Esc 가 "연결"·"항상 허용"으로 뒤집힌다. 마지막에 두면 Esc·닫기·취소 버튼이 모두 cancelId 다.
/// 대가: OS 기본 버튼(Enter)이 첫 버튼(허용 쪽)이 된다 — Electron 판은 defaultId(취소)가 기본이었다(docs/tauri.md 남은 일).
/// 눌린 버튼은 문구로 구분하므로 문구가 겹치면 거부한다.
pub fn message_box(app: &AppHandle, arg: &Value) -> Option<Value> {
    let buttons: Vec<&str> = arg.get("buttons")?.as_array()?.iter().map(Value::as_str).collect::<Option<_>>()?;
    if buttons.is_empty() || buttons.len() > 3 || buttons.iter().any(|b| b.is_empty()) {
        return None;
    }
    if (1..buttons.len()).any(|i| buttons[..i].contains(&buttons[i])) {
        return None;
    }
    let cancel_id = usize::try_from(arg.get("cancelId")?.as_u64()?).ok().filter(|&c| c < buttons.len())?;
    let order = button_order(buttons.len(), cancel_id);
    let label = |n: usize| buttons[order[n]].to_string();
    let kind = match arg.get("kind").and_then(Value::as_str) {
        Some("warning") => MessageDialogKind::Warning,
        Some("error") => MessageDialogKind::Error,
        _ => MessageDialogKind::Info,
    };
    let shown = match buttons.len() {
        1 => MessageDialogButtons::OkCustom(label(0)),
        2 => MessageDialogButtons::OkCancelCustom(label(0), label(1)),
        _ => MessageDialogButtons::YesNoCancelCustom(label(0), label(1), label(2)),
    };
    let parent = app.get_webview_window("main")?;
    let result = app
        .dialog()
        .message(arg.get("message")?.as_str()?)
        .title(arg.get("title")?.as_str()?)
        .kind(kind)
        .buttons(shown)
        .parent(&parent)
        .blocking_show_with_result();
    // 사용자 지정 문구 버튼만 인정한다. 매핑되지 않은 Cancel 과 알 수 없는 값은 None → 취소
    let MessageDialogResult::Custom(pressed) = result else { return None };
    let response = order.iter().copied().find(|&i| buttons[i] == pressed)?;
    Some(json!({ "response": response }))
}

/// 화면 순서(원래 인덱스 목록): 취소가 아닌 버튼(원래 순서) → 취소 버튼(마지막 = 플러그인의 cancel 자리)
fn button_order(len: usize, cancel_id: usize) -> Vec<usize> {
    (0..len).filter(|&i| i != cancel_id).chain(std::iter::once(cancel_id)).collect()
}

#[cfg(test)]
mod tests {
    use super::button_order;

    #[test]
    fn 취소_버튼은_항상_마지막이다() {
        // Esc·닫기는 마지막 자리 버튼으로 바뀌어 돌아온다 — 그 자리가 cancelId 가 아니면 Esc 가 허용이 된다
        assert_eq!(button_order(1, 0), vec![0]);
        assert_eq!(button_order(2, 1), vec![0, 1]); // SSH: [연결][취소]
        assert_eq!(button_order(2, 0), vec![1, 0]);
        assert_eq!(button_order(3, 2), vec![0, 1, 2]); // fetch: [이번만][항상][취소]
        assert_eq!(button_order(3, 0), vec![1, 2, 0]);
        assert_eq!(button_order(3, 1), vec![0, 2, 1]);
    }
}

/// 폴더·파일 선택. 값은 `{ "filePaths": [경로] }`(취소면 빈 배열). 대화상자를 못 띄우면 `None`
pub fn pick_path(app: &AppHandle, arg: &Value) -> Option<Value> {
    let parent = app.get_webview_window("main")?;
    let mut dlg = app.dialog().file().set_parent(&parent);
    if let Some(title) = arg.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        dlg = dlg.set_title(title);
    }
    // 시작 위치는 있는 폴더일 때만 쓴다(없는 경로를 줘도 대화상자는 뜨지만 OS 마다 동작이 다르다)
    if let Some(dir) = arg.get("defaultPath").and_then(Value::as_str).filter(|d| Path::new(d).is_dir()) {
        dlg = dlg.set_directory(dir);
    }
    let folder = match arg.get("mode").and_then(Value::as_str)? {
        "folder" => true,
        "file" => false,
        _ => return None,
    };
    if !folder {
        for f in arg.get("filters").and_then(Value::as_array).into_iter().flatten() {
            let name = f.get("name").and_then(Value::as_str)?;
            let exts: Vec<&str> = f.get("extensions")?.as_array()?.iter().filter_map(Value::as_str).collect();
            dlg = dlg.add_filter(name, &exts);
        }
    }
    let picked = if folder { dlg.blocking_pick_folder() } else { dlg.blocking_pick_file() };
    let paths: Vec<String> = picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .into_iter()
        .collect();
    Some(json!({ "filePaths": paths }))
}
