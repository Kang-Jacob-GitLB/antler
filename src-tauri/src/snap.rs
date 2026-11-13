//! Win11 스냅 레이아웃.
//!
//! Electron 의 titleBarOverlay 는 OS 가 최대화 버튼을 그려 마우스를 올리면 스냅 레이아웃이 뜬다.
//! Tauri 는 그 기능이 없고 창 버튼을 웹 화면에 직접 그리므로 WM_NCHITTEST 가 HTMAXBUTTON 을 돌려주지 못한다.
//! 그래서 최대화 버튼 호버 때 Win+Z(스냅 레이아웃 단축키)를 보내 같은 플라이아웃을 띄운다.
//! (tauri-plugin-decorum 과 같은 방식. 그 플러그인은 Tauri 2.12 에서 버튼 주입이 동작하지 않아 쓰지 않는다)

/// 앱 창이 전면일 때만 호출해야 한다 — 키 입력은 전면 창으로 간다.
#[tauri::command]
pub fn show_snap_overlay(window: tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        // 다른 앱이 전면이면 그 창에 Win+Z 가 가므로 보내지 않는다
        if !window.is_focused().unwrap_or(false) {
            return;
        }
        send_win_z();
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
fn send_win_z() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LWIN,
        VK_MENU,
    };

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    let vk_z = VIRTUAL_KEY(b'Z' as u16);
    let combo = [key(VK_LWIN, false), key(vk_z, false), key(vk_z, true), key(VK_LWIN, true)];
    // Alt 한 번 — 플라이아웃의 숫자 접근 키 표시를 숨긴다(decorum 과 같은 처리)
    let alt = [key(VK_MENU, false), key(VK_MENU, true)];
    unsafe {
        SendInput(&combo, std::mem::size_of::<INPUT>() as i32);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    unsafe {
        SendInput(&alt, std::mem::size_of::<INPUT>() as i32);
    }
}
