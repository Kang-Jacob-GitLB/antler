// 단일 인스턴스(Phase 6c PR 3). 두 번째 실행은 플러그인이 첫 번째 인스턴스에 알리고 곧바로 종료한다 —
// 첫 번째 인스턴스는 기존 `main` 창을 앞으로 가져올 뿐이다.
// **두 번째 실행의 인자·작업 디렉터리는 신뢰하지 않으므로 읽지 않는다**(Antler 는 명령줄 인자로 하는 일이 없다 — 파일 열기·딥링크 없음).
// 나중에 인자를 처리하게 되면 이 모듈이 검증 지점이다.

use std::ffi::OsStr;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

/// 앞으로 가져올 창이 해야 하는 동작(시험에서 가짜로 바꾼다)
pub trait Foregroundable {
    fn is_minimized(&self) -> bool;
    fn unminimize(&self);
    fn show(&self);
    fn set_focus(&self);
}

// 실패(창이 막 닫히는 중 등)는 할 수 있는 일이 없다 — 무시한다
impl<R: Runtime> Foregroundable for WebviewWindow<R> {
    fn is_minimized(&self) -> bool {
        WebviewWindow::is_minimized(self).unwrap_or(false)
    }
    fn unminimize(&self) {
        let _ = WebviewWindow::unminimize(self);
    }
    fn show(&self) {
        let _ = WebviewWindow::show(self);
    }
    fn set_focus(&self) {
        let _ = WebviewWindow::set_focus(self);
    }
}

/// 최소화돼 있으면 풀고, 숨겨져 있으면 보이고, 포커스를 준다
pub fn bring_to_front(win: &impl Foregroundable) {
    if win.is_minimized() {
        win.unminimize();
    }
    win.show();
    win.set_focus();
}

/// 플러그인 콜백. 인자(`_args`)와 작업 디렉터리(`_cwd`)는 쓰지 않는다. 창이 아직 없으면(첫 인스턴스가 기동 중) 아무것도 하지 않는다
pub fn on_second_instance<R: Runtime>(app: &AppHandle<R>, _args: Vec<String>, _cwd: String) {
    if let Some(win) = app.get_webview_window("main") {
        bring_to_front(&win);
    }
}

/// 단일 인스턴스 잠금을 거는가. `ANTLER_USER_DATA` 가 있는 실행(측정·자동화, 비어 있어도 — lib.rs 의 다른 분기와 같다)은
/// 잠금을 걸지도 막히지도 않아, 실사용 앱이 떠 있어도 돌고 실사용 앱도 막지 않는다
pub fn enabled(user_data_env: Option<&OsStr>) -> bool {
    user_data_env.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        minimized: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn new(minimized: bool) -> Self {
            Self { minimized, calls: RefCell::new(vec![]) }
        }
    }

    impl Foregroundable for Fake {
        fn is_minimized(&self) -> bool {
            self.minimized
        }
        fn unminimize(&self) {
            self.calls.borrow_mut().push("unminimize");
        }
        fn show(&self) {
            self.calls.borrow_mut().push("show");
        }
        fn set_focus(&self) {
            self.calls.borrow_mut().push("set_focus");
        }
    }

    #[test]
    fn 최소화된_창은_풀고_보이고_포커스한다() {
        let w = Fake::new(true);
        bring_to_front(&w);
        assert_eq!(*w.calls.borrow(), ["unminimize", "show", "set_focus"]);
    }

    #[test]
    fn 최소화가_아니면_풀지_않는다() {
        let w = Fake::new(false);
        bring_to_front(&w);
        assert_eq!(*w.calls.borrow(), ["show", "set_focus"]);
    }

    #[test]
    fn user_data_가_있으면_비어_있어도_잠금을_걸지_않는다() {
        assert!(enabled(None));
        assert!(!enabled(Some(OsStr::new("C:\\tmp\\ud"))));
        assert!(!enabled(Some(OsStr::new(""))));
    }
}
