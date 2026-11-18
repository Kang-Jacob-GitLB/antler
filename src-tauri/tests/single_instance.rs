//! 단일 인스턴스(Phase 6c PR 3)의 소스 검사. 실제 두 번 실행은 사람 확인 몫이다(docs/tauri.md). 창 동작·잠금 조건은 `src/single_instance.rs` 의 단위 시험이 본다.

use std::fs;
use std::path::Path;

fn read(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// 주석 줄을 뺀 코드
fn code(rel: &str) -> String {
    read(rel).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn 플러그인은_다른_플러그인보다_먼저_등록된다() {
    let lib = code("src/lib.rs");
    let single = lib.find("tauri_plugin_single_instance::init(").expect("단일 인스턴스 플러그인 등록");
    assert_eq!(lib.matches("tauri_plugin_single_instance::init(").count(), 1);
    let run = lib.find("pub fn run()").expect("run");
    assert!(run < single);
    // run() 안의 첫 `.plugin(` 이 단일 인스턴스여야 한다
    let first_plugin = lib[run..].find(".plugin(").map(|i| i + run).expect("플러그인 등록");
    assert!(first_plugin < lib.find("tauri_plugin_clipboard_manager::init()").unwrap());
    assert!(first_plugin < lib.find("tauri_plugin_dialog::init()").unwrap());
    assert!(first_plugin < lib.find("tauri_plugin_window_state::Builder").unwrap());
    assert_eq!(first_plugin, lib[run..].find(".plugin(tauri_plugin_single_instance").map(|i| i + run).unwrap(), "첫 .plugin( 이 단일 인스턴스가 아니다");
    // ANTLER_USER_DATA 분기 안에서만 등록한다
    let before = &lib[run..single];
    assert!(before.trim_end().ends_with("builder = builder.plugin("));
    assert!(before.contains("if single_instance::enabled(std::env::var_os(\"ANTLER_USER_DATA\").as_deref())"));
}

#[test]
fn 두번째_실행의_인자는_읽지_않고_창만_앞으로_가져온다() {
    let src = code("src/single_instance.rs");
    let prod = &src[..src.find("#[cfg(test)]").unwrap()];
    assert!(prod.contains("_args: Vec<String>") && prod.contains("_cwd: String"), "인자는 밑줄로 무시한다");
    for banned in ["args.", "cwd.", "Command", "open_", "std::fs", "emit"] {
        assert!(!prod.replace("_args", "").replace("_cwd", "").contains(banned), "single_instance.rs 가 {banned} 를 쓴다");
    }
    assert!(prod.contains("get_webview_window(\"main\")"));
}

#[test]
fn capability_는_늘지_않는다() {
    // 플러그인은 JS command 를 노출하지 않는다 — capabilities 에 single-instance 권한이 없어야 한다
    for entry in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities")).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("single-instance"));
    }
}
