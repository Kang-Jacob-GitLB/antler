mod commands;
mod dialogs;
pub mod git;
pub mod mirror;
pub mod router;
mod sidecar;
mod snap;

use std::path::PathBuf;

use tauri::window::Color;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_window_state::StateFlags;

/// 첫 페인트 전 흰 화면을 막는다. 토큰 --bg-base(Dark)와 같은 값
const BG: Color = Color(0x14, 0x11, 0x0F, 0xFF);

/// WebView2 실행 인자. 하드웨어 가속은 기본으로 끈다(Electron 판과 같은 이유 — ANTLER_GPU=1 로 켠다)
fn browser_args() -> String {
    let mut args = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    if std::env::var("ANTLER_GPU").as_deref() != Ok("1") {
        args.push_str(" --disable-gpu");
    }
    #[cfg(any(debug_assertions, feature = "cdp"))]
    if let Ok(port) = std::env::var("ANTLER_CDP_PORT") {
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
            args.push_str(&format!(" --remote-debugging-port={port}"));
        }
    }
    args
}

/// 앱 자신의 화면 주소인가. 배포 빌드는 tauri.localhost 만, 개발 빌드는 vite dev 서버(devUrl)도 허용한다
fn is_app_url(url: &tauri::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    let scheme_ok = matches!(url.scheme(), "http" | "https" | "tauri");
    if scheme_ok && (host == "tauri.localhost" || url.scheme() == "tauri" && host == "localhost") {
        return true;
    }
    cfg!(debug_assertions) && url.scheme() == "http" && host == "localhost" && url.port() == Some(5174)
}

/// https://github.com 의 링크인가(사용자 정보 없음, 2048자 이하)
fn is_github_link(url: &tauri::Url) -> bool {
    url.as_str().len() <= 2048
        && url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

pub fn run() {
    commands::mark_start();
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        // 확인 창·폴더 선택은 Rust(dialogs.rs)에서만 띄운다. capability 에 dialog 권한이 없어 renderer 는 부를 수 없다
        .plugin(tauri_plugin_dialog::init());
    // 창 위치·크기·최대화만 기억한다(visible·decorations·fullscreen 은 제외). ANTLER_USER_DATA 로 도는 측정·자동화 실행은
    // 실사용 창 상태를 덮어쓰지 않게 등록하지 않는다. 플러그인의 JS command 도 capability 에 주지 않는다
    if std::env::var_os("ANTLER_USER_DATA").is_none() {
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        );
    }
    builder
        .invoke_handler(tauri::generate_handler![
            snap::show_snap_overlay,
            commands::antler_invoke,
            commands::antler_uptime_ms
        ])
        .setup(|app| {
            // Electron 판의 %APPDATA%\Antler 와 설정이 섞이지 않게 기본값은 Tauri 의 app_data_dir 다
            let user_data: PathBuf = match std::env::var_os("ANTLER_USER_DATA") {
                Some(p) => p.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&user_data)?;
            // 라우터와 사이드카가 같은 미러를 본다(사이드카가 채우고 라우터가 읽는다)
            let mirror = mirror::RepoMirror::default();
            app.manage(router::Router::from_env(mirror.clone()));
            app.manage(sidecar::Sidecar::spawn(app.handle(), &user_data, mirror)?);

            let mut win = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Antler")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 600.0)
                .decorations(false)
                // 앱이 HTML5 드래그 앤 드롭을 쓴다 — Tauri 의 파일 드롭 처리가 이를 가로채지 않게 끈다
                .disable_drag_drop_handler()
                .background_color(BG)
                .data_directory(user_data.join("webview2"))
                .additional_browser_args(&browser_args())
                // src/main/index.ts 의 will-navigate·setWindowOpenHandler·권한 거부와 같다.
                // 드롭한 파일·링크로 창이 앱 밖 주소로 바뀌어 앱처럼 보이는 가짜 화면을 띄우지 못하게 한다
                .on_navigation(is_app_url)
                .on_new_window(|url, _| {
                    // 새 창은 열지 않고, github.com 링크만 기본 브라우저로 보낸다(src/main/github/validate.ts 의 allowedExternalUrl 과 같은 규칙)
                    if is_github_link(&url) {
                        let _ = tauri_plugin_opener::open_url(url.as_str(), None::<&str>);
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny);
            if std::env::var("ANTLER_MEASURE").as_deref() == Ok("1") {
                win = win.initialization_script("window.__ANTLER_MEASURE__=true");
            }
            win.build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 창이 뒤에 있을 때는 원격 폴링을 늦춘다(src/main/index.ts 의 focus·blur 처리와 같다)
            if let WindowEvent::Focused(focused) = event {
                if let Some(s) = window.app_handle().try_state::<sidecar::Sidecar>() {
                    s.set_focus(*focused);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Tauri 앱 빌드 실패")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(s) = app.try_state::<sidecar::Sidecar>() {
                    s.kill();
                }
            }
        });
}
