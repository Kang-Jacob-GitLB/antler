fn main() {
    ensure_notices_placeholder();
    tauri_build::build();
    embed_test_manifest();
}

/// `bundle.resources` 의 고지 파일(gen/licenses/THIRD_PARTY_NOTICES.txt)은 `pnpm tauri:notices` 가 만들고 커밋하지 않는다. tauri-build 는 리소스가 없으면
/// 실패하므로, 만들기 전의 `cargo check`·시험·개발 실행이 막히지 않게 자리 표시 파일을 둔다. 설치본은 `tauri build` 의 beforeBuildCommand 가 진짜 파일로 덮어쓴다
fn ensure_notices_placeholder() {
    let path = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("gen/licenses/THIRD_PARTY_NOTICES.txt");
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, "서드파티 라이선스 고지는 `pnpm tauri:notices` 로 만든다(개발 빌드의 자리 표시 파일).\n");
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
