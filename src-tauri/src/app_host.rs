//! 설정이 필요 없는 앱 채널 4개(Phase 6b PR 5)의 Rust 쪽 처리: `dialog:pickFolder`·`clipboard:writeText`·`code:openLink`·`win:setTitleBarTheme`.
//! 라우터([`crate::router::Router`])가 `APP_CHANNELS`(router.rs)를 이 모듈로 넘기면 `src/main/ipc/handlers.ts`·`external.ts` 의 핸들러와 같은 입력 검증·결과·오류 봉투
//! (`handle()` 과 같은 `{ok:false,error,code:"ERROR"}`, void 는 `data` 없는 `{ok:true}`)를 돌려준다.
//!
//! 대화상자·클립보드·브라우저 열기는 **포트로 주입**한다([`AppHost`]). 운영 구현은 `lib.rs` 가 달고(대화상자는 `dialogs.rs`, 클립보드는 플러그인, 열기는 `open_https`),
//! 시험은 가짜를 달아 실제 창·사용자 클립보드·브라우저를 건드리지 않는다. 포트는 막는 호출이라 `spawn_blocking` 으로 부른다.
//! 이 모듈은 프로세스를 띄우지 않는다 — 링크 열기는 `open_https` 가 쓰는 기존 열기 지점 하나다.
//!
//! 이 모듈은 `Sidecar`·`AppHandle` 에 기대지 않는다 — 통합 테스트(`tests/app_route.rs`)가 그대로 돌릴 수 있다.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::api_host::Opener;
use crate::external::link::allowed_link_url;
use crate::git::exec::wsl_paths::is_wsl_unc;
use crate::git::fetch::guard::has_nt_namespace;
use crate::git::watcher::is_unc;

/// 클립보드에 쓸 수 있는 최대 글자 수(UTF-16 단위, `external.ts` 의 `CLIPBOARD_MAX_CHARS`)
const CLIPBOARD_MAX_CHARS: usize = 1_000_000;

const BAD_INPUT: &str = "잘못된 입력입니다";
const CLIPBOARD_DENIED: &str = "복사할 수 없는 내용입니다";
const LINK_DENIED: &str = "https 주소만 열 수 있습니다";
const FAILED: &str = "처리하지 못했습니다";

/// 폴더 선택 대화상자. 인자는 **관문을 통과한** 시작 위치(없으면 `None`)이고, 값은 고른 폴더(취소·창 없음이면 `None`)다
pub type PickFolder = Arc<dyn Fn(Option<String>) -> Result<Option<String>, String> + Send + Sync>;
/// 클립보드에 텍스트를 쓴다(읽기 포트는 없다)
pub type ClipboardWrite = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// 대화상자·클립보드·열기 포트. 라우터가 하나만 가진다
#[derive(Clone)]
pub struct AppHost {
    pub pick_folder: PickFolder,
    pub clipboard: ClipboardWrite,
    /// 검증을 마친 https 주소를 기본 브라우저로 연다
    pub open_link: Opener,
}

/// 폴더 선택의 시작 위치로 쓸 수 있는가(`handlers.ts` 의 `safeDefault`). 네트워크 경로(`\\host\share`·`\??\UNC\…`)를 시작 위치로 주면
/// 대화상자(와 `is_dir` 확인)가 SMB 인증을 시도해 NTLM 해시가 새므로 WSL 공유만 허용한다. **존재 확인(`is_dir`)보다 앞서** 판정한다 —
/// TS 처럼 앞뒤 공백을 지우지 않은 값으로 UNC 를, 공백을 지운 값으로 WSL 공유를 본다
pub fn safe_default_path(path: &str) -> bool {
    !path.is_empty() && !has_nt_namespace(path) && (!is_unc(path) || is_wsl_unc(path))
}

fn ok_void() -> Value {
    json!({ "ok": true })
}

fn ok_data(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

fn err(message: &str) -> Value {
    json!({ "ok": false, "error": message, "code": "ERROR" })
}

/// 막는 포트를 런타임 스레드를 붙잡지 않게 따로 돌린다
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|_| Err(FAILED.to_string()))
}

/// 앱 채널 하나를 처리해 IpcEnvelope 를 돌려준다. `args` 는 표식을 되돌린 인자 배열(`None` = undefined)이다
pub async fn app_envelope(host: AppHost, channel: &str, args: Vec<Option<Value>>) -> Value {
    let a0 = args.first().and_then(Option::as_ref);
    match channel {
        "dialog:pickFolder" => {
            let default = a0.and_then(Value::as_str).filter(|d| safe_default_path(d)).map(str::to_string);
            let pick = host.pick_folder.clone();
            match blocking(move || pick(default)).await {
                Ok(path) => ok_data(path.map_or(Value::Null, Value::String)),
                Err(e) => err(&e),
            }
        }
        "clipboard:writeText" => {
            let Some(text) = a0.and_then(Value::as_str).filter(|t| t.encode_utf16().count() <= CLIPBOARD_MAX_CHARS) else {
                return err(CLIPBOARD_DENIED);
            };
            let (write, text) = (host.clipboard.clone(), text.to_string());
            match blocking(move || write(&text)).await {
                Ok(()) => ok_void(),
                Err(e) => err(&e),
            }
        }
        "code:openLink" => {
            let Some(href) = a0.and_then(Value::as_str).and_then(allowed_link_url) else {
                return err(LINK_DENIED);
            };
            let open = host.open_link.clone();
            match blocking(move || open(&href)).await {
                Ok(()) => ok_void(),
                Err(e) => err(&e),
            }
        }
        "win:setTitleBarTheme" => {
            // 값 허용 목록: `{dark: bool, dim?: bool}`. 창 버튼은 앱이 그리므로(bridge.ts) 검증 뒤에 할 일이 없다(Electron 판도 창이 CSS 로 그려 아무것도 하지 않는다)
            let valid = a0.and_then(Value::as_object).is_some_and(|t| t.get("dark").is_some_and(Value::is_boolean) && t.get("dim").is_none_or(Value::is_boolean));
            if valid {
                ok_void()
            } else {
                err(BAD_INPUT)
            }
        }
        other => err(&format!("알 수 없는 채널: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 시작_위치_관문은_네트워크_경로를_거른다() {
        for ok in ["C:\\Users\\a", "/home/a", "\\\\wsl.localhost\\Ubuntu\\home", "//wsl$/Ubuntu/home", " \\\\wsl.localhost\\Ubuntu"] {
            assert!(safe_default_path(ok), "{ok}");
        }
        for bad in ["", "\\\\host\\share", "//host/share", "\\??\\UNC\\host\\share", "C:\\a\\??\\b", "/??/UNC/host/share"] {
            assert!(!safe_default_path(bad), "{bad}");
        }
    }
}
