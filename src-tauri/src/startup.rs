//! 시작 화면. 메인 창은 숨긴 채 만들고, 첫 화면이 그려진 뒤에야 보인다(빈 바탕색 창이 먼저 뜨는 회귀를 막는다).
//! 그 사이에는 작은 로딩 창(스플래시, `splash.html`)이 화면 가운데에 떠 있다가 메인 창이 보일 때 닫힌다.
//! 보이는 길은 둘이다 — renderer 의 준비 신호(`antler_window_ready`)와, 신호가 없어도 창이 영영 안 뜨는 일이 없게 하는 안전망(타이머).
//! 어느 쪽이든 한 번만 보이고 나머지는 무시한다(`StartupGate`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

pub const MAIN_LABEL: &str = "main";
pub const SPLASH_LABEL: &str = "splash";

/// 토큰 --bg-base(Dark). tests/startup.rs 가 tokens.css 와 같은지 본다
pub const DARK_BG: (u8, u8, u8) = (0x14, 0x11, 0x0F);
/// 토큰 --bg-base(Light)
pub const LIGHT_BG: (u8, u8, u8) = (0xF6, 0xF1, 0xE7);

/// 페이지 로드가 끝난 뒤에도 준비 신호가 없을 때 기다리는 시간
pub const LOADED_GRACE: Duration = Duration::from_secs(3);
/// 페이지 로드 완료조차 오지 않을 때(로드 실패·멈춤) 창을 만든 뒤 기다리는 시간
pub const NEVER_LOADED_LIMIT: Duration = Duration::from_secs(10);

/// 설정 `ui.theme`(dark·light·system)과 OS 다크 여부로 창 바탕색을 고른다. 알 수 없는 값은 system 과 같다
pub fn background_rgb(theme: Option<&str>, os_dark: bool) -> (u8, u8, u8) {
    match theme {
        Some("light") => LIGHT_BG,
        Some("dark") => DARK_BG,
        _ if os_dark => DARK_BG,
        _ => LIGHT_BG,
    }
}

/// 라이트 바탕인가(스플래시의 글자색을 고르는 데 쓴다)
pub fn is_light(rgb: (u8, u8, u8)) -> bool {
    rgb == LIGHT_BG
}

/// 안전망 타이머를 얼마나 기다리는가
pub fn fallback_delay(page_loaded: bool) -> Duration {
    if page_loaded {
        LOADED_GRACE
    } else {
        NEVER_LOADED_LIMIT
    }
}

/// 스플래시 페이지에 라이트 테마를 알리는 초기화 스크립트(페이지 자체에는 스크립트가 없다). 다크는 페이지 기본값이라 없다
pub fn splash_init_script(light: bool) -> Option<&'static str> {
    light.then_some("(function(){function a(){document.documentElement.setAttribute('data-theme','light')}if(document.documentElement){a()}document.addEventListener('DOMContentLoaded',a)})()")
}

/// OS 의 앱 테마가 다크인가. 판단할 수 없으면(키 없음·Windows 가 아님) 다크다
#[cfg(windows)]
pub fn os_prefers_dark() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let mut value: u32 = 1;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`·`bytes` 는 서로 맞는 크기(DWORD)의 지역 값이다
    let got = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast()),
            Some(&mut bytes),
        )
    };
    got != ERROR_SUCCESS || value == 0
}

#[cfg(not(windows))]
pub fn os_prefers_dark() -> bool {
    true
}

/// 메인 창을 한 번만 보이게 하는 문. 앱 상태로 둔다
#[derive(Default)]
pub struct StartupGate(AtomicBool);

impl StartupGate {
    /// 처음 부른 쪽만 참을 받는다
    pub fn try_reveal(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }

    pub fn revealed(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 메인 창을 보이고 포커스를 준 뒤 스플래시를 닫는다. 두 번째부터는 아무것도 하지 않는다.
/// 메인 창이 없어도(만들기 실패) 스플래시는 닫는다 — 로딩 창이 영원히 남지 않게
pub fn reveal<R: Runtime>(app: &AppHandle<R>) {
    let Some(gate) = app.try_state::<StartupGate>() else { return };
    if !gate.try_reveal() {
        return;
    }
    if let Some(main) = app.get_webview_window(MAIN_LABEL) {
        let _ = main.show();
        let _ = main.set_focus();
    }
    close_splash(app);
}

/// 스플래시를 닫는다(없으면 무시)
pub fn close_splash<R: Runtime>(app: &AppHandle<R>) {
    if let Some(splash) = app.get_webview_window(SPLASH_LABEL) {
        let _ = splash.close();
    }
}

/// 안전망: 일정 시간 뒤 신호가 없었어도 보인다
pub fn schedule_fallback<R: Runtime>(app: AppHandle<R>, page_loaded: bool) {
    let delay = fallback_delay(page_loaded);
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        reveal(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 테마가_바탕색을_정한다() {
        assert_eq!(background_rgb(Some("light"), true), LIGHT_BG);
        assert_eq!(background_rgb(Some("dark"), false), DARK_BG);
        assert_eq!(background_rgb(Some("system"), true), DARK_BG);
        assert_eq!(background_rgb(Some("system"), false), LIGHT_BG);
        assert_eq!(background_rgb(None, false), LIGHT_BG);
        assert_eq!(background_rgb(Some("???"), true), DARK_BG);
    }

    #[test]
    fn 안전망_시간은_로드_완료_여부로_정해진다() {
        assert_eq!(fallback_delay(true), LOADED_GRACE);
        assert_eq!(fallback_delay(false), NEVER_LOADED_LIMIT);
        assert!(LOADED_GRACE < NEVER_LOADED_LIMIT);
    }

    #[test]
    fn 문은_한_번만_열린다() {
        let gate = StartupGate::default();
        assert!(!gate.revealed());
        assert!(gate.try_reveal());
        assert!(gate.revealed());
        assert!(!gate.try_reveal());
    }

    #[test]
    fn 스플래시_스크립트는_라이트에만_있다() {
        assert!(splash_init_script(true).is_some_and(|s| s.contains("light")));
        assert!(splash_init_script(false).is_none());
        assert!(is_light(LIGHT_BG) && !is_light(DARK_BG));
    }
}
