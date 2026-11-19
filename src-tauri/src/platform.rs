//! renderer 가 보는 OS 이름. Node `process.platform` 과 같은 값('win32'·'darwin'·'linux')으로 바꿔 `window.antler.platform` 에 싣는다.
//! 단축키 표기(⌘ 대 Ctrl)·창 단추·환경 배지·저장소 경로 입력 안내가 이 값으로 갈린다.

/// `std::env::consts::OS` 를 Node `process.platform` 값으로 바꾼다. 모르는 OS(BSD 등)는 'linux' 로 본다 — 둘 다 비-mac 데스크톱이다
pub fn node_platform(os: &str) -> &'static str {
    match os {
        "windows" => "win32",
        "macos" => "darwin",
        _ => "linux",
    }
}

/// 이 실행 파일이 도는 OS 의 renderer 용 이름
pub fn current() -> &'static str {
    node_platform(std::env::consts::OS)
}

/// 창을 만들 때 심는 초기화 스크립트. `bridge.ts` 가 `window.antler` 를 만들기 전에 읽는다(IPC 왕복 없이 동기로)
pub fn init_script() -> String {
    format!("window.__ANTLER_PLATFORM__={:?};", current())
}

/// OS 기본 제목 표시줄을 쓰는가. Windows 만 앱이 창 단추를 직접 그린다(`snap.rs`·`bridge.ts` 의 `mountWindowControls`)
pub fn native_titlebar() -> bool {
    current() != "win32"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_이름을_node_platform_으로_바꾼다() {
        assert_eq!(node_platform("windows"), "win32");
        assert_eq!(node_platform("macos"), "darwin");
        assert_eq!(node_platform("linux"), "linux");
        assert_eq!(node_platform("freebsd"), "linux");
    }

    #[test]
    fn 현재_os_는_세_값_중_하나다() {
        assert!(["win32", "darwin", "linux"].contains(&current()));
        assert_eq!(init_script(), format!("window.__ANTLER_PLATFORM__=\"{}\";", current()));
    }
}
