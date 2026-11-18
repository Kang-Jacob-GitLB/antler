fn main() {
    tauri_build::build();
    embed_test_manifest();
}

/// Windows(MSVC)의 **통합 시험 실행 파일**에 Common Controls v6 매니페스트를 넣는다. tauri-build 는 앱 실행 파일에만 매니페스트(리소스)를 넣는데, 가짜 앱(`tauri::test`)을 쓰는 시험
/// (tests/update_security.rs — 진짜 업데이트 플러그인의 서명 검증을 창 없이 돈다)은 tauri 를 통째로 끌어와 `comctl32` 의 `TaskDialogIndirect` 를 가져오고, v6 매니페스트가 없으면
/// 시작 때 `STATUS_ENTRYPOINT_NOT_FOUND`(0xc0000139)로 죽는다. 앱·라이브러리 빌드에는 영향이 없다(`rustc-link-arg-tests`)
fn embed_test_manifest() {
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("windows-test-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest.display());
}
