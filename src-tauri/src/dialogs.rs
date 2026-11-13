//! 사이드카가 부탁한 확인 창·폴더/파일 선택을 Rust 네이티브 대화상자로 띄운다(webview HTML 이 아니다).
//! renderer 에는 이 대화상자를 띄우거나 누를 경로가 없다 — 플러그인의 JS command 는 capability 에 주지 않는다.
//! 실패(창 없음·잘못된 인자·닫기·Esc)는 모두 `None` 이고, 사이드카가 이를 취소(cancelId)로 바꾼다.
//! 인자 모양은 src/sidecar/protocol.ts 의 MessageBoxArg·PickPathArg 와 같아야 한다.

use std::path::Path;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
#[cfg(not(windows))]
use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind, MessageDialogResult};

/// 확인 창. 값은 `{ "response": <arg.buttons 의 원래 인덱스> }`. 눌린 버튼은 문구로 구분하므로 문구가 겹치면 거부한다.
///
/// - **Windows**: rfd 를 직접 쓴다. rfd 는 사용자 지정 버튼마다 고유 ID 를 붙여 누른 버튼은 `Custom(문구)`, Esc·창 닫기는 `Cancel` 로
///   구분해 돌려준다. 그래서 취소 버튼을 **맨 앞(= 기본 버튼, Enter)** 에 둘 수 있다 — Electron 판의 defaultId(취소)와 같다.
/// - **그 밖**: tauri-plugin-dialog 2.8 은 Esc·닫기로 생긴 `Cancel` 을 `Custom(cancel 자리 문구)` 로 바꿔 돌려준다(desktop.rs 의
///   show_message_dialog). 그 자리에 다른 버튼이 있으면 Esc 가 "연결"·"항상 허용"으로 뒤집히므로 취소를 **마지막**(cancel 자리)에 둔다.
///   대가로 Enter 가 첫 버튼(허용 쪽)을 누른다.
pub fn message_box(app: &AppHandle, arg: &Value) -> Option<Value> {
    let buttons: Vec<&str> = arg.get("buttons")?.as_array()?.iter().map(Value::as_str).collect::<Option<_>>()?;
    if buttons.is_empty() || buttons.len() > 3 || buttons.iter().any(|b| b.is_empty()) {
        return None;
    }
    if (1..buttons.len()).any(|i| buttons[..i].contains(&buttons[i])) {
        return None;
    }
    let cancel_id = usize::try_from(arg.get("cancelId")?.as_u64()?).ok().filter(|&c| c < buttons.len())?;
    let message = arg.get("message")?.as_str()?;
    let title = arg.get("title")?.as_str()?;
    let kind = arg.get("kind").and_then(Value::as_str);
    let parent = app.get_webview_window("main")?;
    let pressed = show(&parent, &buttons, cancel_id, title, message, kind)?;
    let response = buttons.iter().position(|b| *b == pressed)?;
    Some(json!({ "response": response }))
}

/// 화면 순서(원래 인덱스 목록)
fn button_order(len: usize, cancel_id: usize, cancel_first: bool) -> Vec<usize> {
    let rest = (0..len).filter(move |&i| i != cancel_id);
    if cancel_first {
        std::iter::once(cancel_id).chain(rest).collect()
    } else {
        rest.chain(std::iter::once(cancel_id)).collect()
    }
}

/// 누른 버튼의 문구. Esc·닫기·실패는 None
#[cfg(windows)]
fn show(parent: &tauri::WebviewWindow, buttons: &[&str], cancel_id: usize, title: &str, message: &str, kind: Option<&str>) -> Option<String> {
    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
    let order = button_order(buttons.len(), cancel_id, true);
    let label = |n: usize| buttons[order[n]].to_string();
    let level = match kind {
        Some("warning") => MessageLevel::Warning,
        Some("error") => MessageLevel::Error,
        _ => MessageLevel::Info,
    };
    let shown = match buttons.len() {
        1 => MessageButtons::OkCustom(label(0)),
        2 => MessageButtons::OkCancelCustom(label(0), label(1)),
        _ => MessageButtons::YesNoCancelCustom(label(0), label(1), label(2)),
    };
    // 이 함수는 사이드카 host 처리 스레드(std::thread)에서 불린다. TaskDialog 는 부르는 스레드에서 돌고 부모 창을 비활성화한다(모달)
    let result = MessageDialog::new().set_level(level).set_title(title).set_description(message).set_buttons(shown).set_parent(parent).show();
    match result {
        MessageDialogResult::Custom(pressed) => Some(pressed),
        _ => None, // Cancel(Esc·닫기) 등
    }
}

#[cfg(not(windows))]
fn show(parent: &tauri::WebviewWindow, buttons: &[&str], cancel_id: usize, title: &str, message: &str, kind: Option<&str>) -> Option<String> {
    let order = button_order(buttons.len(), cancel_id, false);
    let label = |n: usize| buttons[order[n]].to_string();
    let kind = match kind {
        Some("warning") => MessageDialogKind::Warning,
        Some("error") => MessageDialogKind::Error,
        _ => MessageDialogKind::Info,
    };
    let shown = match buttons.len() {
        1 => MessageDialogButtons::OkCustom(label(0)),
        2 => MessageDialogButtons::OkCancelCustom(label(0), label(1)),
        _ => MessageDialogButtons::YesNoCancelCustom(label(0), label(1), label(2)),
    };
    let result = parent.dialog().message(message).title(title).kind(kind).buttons(shown).parent(parent).blocking_show_with_result();
    match result {
        MessageDialogResult::Custom(pressed) => Some(pressed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::button_order;

    #[test]
    fn 플러그인_경로에서는_취소_버튼이_마지막이다() {
        // 플러그인은 Esc·닫기를 마지막 자리 버튼으로 바꿔 돌려준다 — 그 자리가 cancelId 가 아니면 Esc 가 허용이 된다
        assert_eq!(button_order(1, 0, false), vec![0]);
        assert_eq!(button_order(2, 1, false), vec![0, 1]); // SSH: [연결][취소]
        assert_eq!(button_order(2, 0, false), vec![1, 0]);
        assert_eq!(button_order(3, 2, false), vec![0, 1, 2]); // fetch: [이번만][항상][취소]
        assert_eq!(button_order(3, 0, false), vec![1, 2, 0]);
        assert_eq!(button_order(3, 1, false), vec![0, 2, 1]);
    }

    #[test]
    fn 윈도우_경로에서는_취소_버튼이_맨_앞_기본_버튼이다() {
        // rfd 직접 호출은 Esc 를 Cancel 로 따로 돌려주므로 취소를 기본 버튼(첫 자리)에 둔다
        assert_eq!(button_order(1, 0, true), vec![0]);
        assert_eq!(button_order(2, 1, true), vec![1, 0]); // SSH: [취소][연결]
        assert_eq!(button_order(3, 2, true), vec![2, 0, 1]); // fetch: [취소][이번만][항상]
        assert_eq!(button_order(3, 1, true), vec![1, 0, 2]);
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
