//! 시작 화면(startup.rs) 소스 검사: 메인 창은 숨긴 채 만들어 신호·안전망으로만 보이고, 로딩 창은 권한 없이 가운데 뜨며, 바탕색 상수는 tokens.css 와 같다.
//! 실제 창을 띄우지 않는다.

use std::fs;
use std::path::Path;

use antler_lib::startup::{DARK_BG, LIGHT_BG};

fn read(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap().replace("\r\n", "\n")
}

/// tokens.css 에서 `selector` 블록의 `--bg-base` 값을 읽는다
fn token_bg(selector: &str) -> (u8, u8, u8) {
    let css = read("../src/renderer/src/styles/tokens.css");
    let block = &css[css.find(selector).unwrap_or_else(|| panic!("{selector} 블록이 없다"))..];
    let line = block.lines().find(|l| l.trim_start().starts_with("--bg-base:")).unwrap();
    let hex = line.split('#').nth(1).unwrap().trim_end_matches(';').trim();
    let n = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    (n(0), n(2), n(4))
}

#[test]
fn 바탕색_상수는_tokens_css_와_같다() {
    assert_eq!(DARK_BG, token_bg(":root {"));
    assert_eq!(LIGHT_BG, token_bg(":root[data-theme='light']"));
}

#[test]
fn 메인_창은_숨긴_채_만들고_바탕색은_테마에서_온다() {
    let lib = read("src/lib.rs");
    let main = &lib[lib.find("WebviewWindowBuilder::new(app, \"main\"").unwrap()..];
    let main = &main[..main.find("win.build()").unwrap()];
    assert!(main.contains(".visible(false)"));
    assert!(main.contains(".background_color(background)"));
    assert!(main.contains("PageLoadEvent::Finished"), "페이지 로드 완료 뒤 안전망");
    assert!(lib.contains("startup::schedule_fallback(app.handle().clone(), false)"), "로드 완료가 안 와도 도는 안전망");
    assert!(!lib.contains("const BG"));
}

#[test]
fn 보이는_길은_신호와_안전망_둘이고_둘_다_reveal_을_거친다() {
    let startup = read("src/startup.rs");
    let prod = &startup[..startup.find("#[cfg(test)]").unwrap()];
    let reveal = &prod[prod.find("pub fn reveal").unwrap()..prod.find("/// 스플래시를 닫는다").unwrap()];
    assert!(reveal.contains("try_reveal") && reveal.contains(".show()") && reveal.contains(".set_focus()") && reveal.contains("close_splash"));
    assert!(prod.contains("schedule_fallback") && prod.contains("reveal(&app)"));
    // 메인 창을 show 하는 곳은 reveal 과 두 번째 실행(single_instance) 뿐이다
    let cmd = read("src/commands.rs");
    assert!(cmd.contains("pub fn antler_window_ready") && cmd.contains("crate::startup::reveal"));
    assert!(read("src/lib.rs").contains("commands::antler_window_ready"));
    assert!(!read("src/lib.rs").contains(".show()"));
}

#[test]
fn 두번째_실행은_숨은_메인_창을_보이지_않는다() {
    let si = read("src/single_instance.rs");
    let at = si.find("pub fn on_second_instance").unwrap();
    let body = &si[at..si.find("/// 단일 인스턴스 잠금").unwrap()];
    assert!(body.find("revealed()").unwrap() < body.find("bring_to_front").unwrap());
}

#[test]
fn 로딩_창은_가운데_권한_없이_탐색_가드와_함께_뜬다() {
    let lib = read("src/lib.rs");
    let start = lib.find("fn build_splash").unwrap();
    let splash = &lib[start..start + lib[start..].find("splash.build()").unwrap()];
    for needle in [".decorations(false)", ".center()", ".resizable(false)", ".maximizable(false)", ".on_navigation(is_app_url)", "NewWindowResponse::Deny", "PermissionResponse::Deny", "browser_args()", "\"webview2\"", "splash.html"] {
        assert!(splash.contains(needle), "로딩 창 builder 에 {needle} 가 없다");
    }
    // 로딩 창이 setup 의 앞쪽(라우터를 만들기 전)에서 만들어지고, 크기·위치는 기억하지 않는다
    assert!(lib.find("build_splash(app").unwrap() < lib.find("router::Router::from_env").unwrap());
    assert!(lib.contains("with_denylist(&[startup::SPLASH_LABEL])"));
}

#[test]
fn capability_는_메인_창만_지정하고_창_보이기_권한이_없다() {
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(text.contains("\"windows\": [\"main\"]"), "capability 가 창을 main 으로 한정하지 않는다");
        for banned in ["allow-show", "allow-hide", "allow-set-focus", "allow-center", "core:window:default", "splash"] {
            assert!(!text.contains(banned), "capability 에 {banned} 가 있다");
        }
    }
}

#[test]
fn renderer_는_창을_직접_보이지_않는다() {
    let bridge = read("../src/renderer/tauri/bridge.ts");
    assert!(bridge.contains("antler_window_ready"));
    assert!(!bridge.contains(".show()"));
}
