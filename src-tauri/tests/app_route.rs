//! 앱 채널 4개(`dialog:pickFolder`·`clipboard:writeText`·`code:openLink`·`win:setTitleBarTheme`, Phase 6b PR 5)의 라우팅·입력 검증 통합 테스트.
//! 대화상자·클립보드·브라우저는 **가짜 포트**다 — 실제 창을 띄우지 않고 사용자의 클립보드를 읽거나 쓰지 않으며 브라우저를 열지 않는다.
//! 기대값은 TS 핸들러(`src/main/ipc/handlers.ts`·`external.ts`)가 낼 봉투다: 오류는 `{ok:false,error,code:"ERROR"}`, void 는 `data` 없는 `{ok:true}`.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use antler_lib::app_host::AppHost;
use antler_lib::mirror::RepoMirror;
use antler_lib::router::{Routed, Router, API_CHANNELS, API_SIDECAR_CHANNELS, APP_CHANNELS, RUST_CHANNELS, UNDEFINED_ARG};
use serde_json::{json, Value};

#[derive(Default)]
struct Calls {
    picks: Vec<Option<String>>,
    clips: Vec<String>,
    links: Vec<String>,
}

/// 포트 호출을 기록하는 가짜 host. `pick` 은 대화상자가 돌려줄 값, `fail` 이면 모든 포트가 실패한다
fn fake(calls: &Arc<Mutex<Calls>>, pick: Option<&str>, fail: bool) -> AppHost {
    let (c1, c2, c3) = (calls.clone(), calls.clone(), calls.clone());
    let pick = pick.map(str::to_string);
    AppHost {
        pick_folder: Arc::new(move |d| {
            c1.lock().unwrap().picks.push(d);
            if fail { Err("대화상자 실패".to_string()) } else { Ok(pick.clone()) }
        }),
        clipboard: Arc::new(move |t| {
            c2.lock().unwrap().clips.push(t.to_string());
            if fail { Err("복사하지 못했습니다".to_string()) } else { Ok(()) }
        }),
        open_link: Arc::new(move |u| {
            c3.lock().unwrap().links.push(u.to_string());
            if fail { Err("주소를 열지 못했습니다".to_string()) } else { Ok(()) }
        }),
    }
}

fn router(enabled: bool, host: Option<AppHost>) -> Router {
    let r = Router::new(enabled, false, RepoMirror::default());
    if let Some(h) = host {
        r.set_app(h);
    }
    r
}

async fn done(r: &Router, channel: &str, args: Value) -> Value {
    match r.route(channel, &args).await {
        Routed::Done(v) => v,
        Routed::Sidecar => panic!("{channel} 가 사이드카로 갔다"),
    }
}

fn err(msg: &str) -> Value {
    json!({ "ok": false, "error": msg, "code": "ERROR" })
}

fn ipc_channels() -> Vec<String> {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("tests/golden/exec/channels.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    v["channels"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()
}

#[test]
fn 앱_채널은_ipc_채널의_부분집합이고_다른_소유_목록과_겹치지_않는다() {
    let all = ipc_channels();
    assert_eq!(APP_CHANNELS, ["dialog:pickFolder", "clipboard:writeText", "code:openLink", "win:setTitleBarTheme"]);
    for ch in APP_CHANNELS {
        assert!(all.iter().any(|c| c == ch), "{ch} 가 IPC_CHANNELS 에 없다(tests/golden/exec/channels.json)");
        assert!(!RUST_CHANNELS.contains(ch) && !API_CHANNELS.contains(ch) && !API_SIDECAR_CHANNELS.contains(ch), "{ch} 가 다른 Rust 채널 목록에도 있다");
    }
}

#[tokio::test]
async fn 앱_채널은_사이드카로_가지_않고_사이드카_몫_채널은_계속_사이드카로_간다() {
    let calls = Arc::default();
    let r = router(true, Some(fake(&calls, None, false)));
    for ch in APP_CHANNELS {
        assert!(matches!(r.route(ch, &json!([])).await, Routed::Done(_)), "{ch}");
    }
    // 아직 사이드카에 남는 채널(설정·저장소 관리·external·update — env·scan 은 PR 8 부터 Rust 다, tests/env_route.rs)
    for ch in ["config:get", "config:setUi", "repos:add", "repos:addMany", "repos:addGithub", "external:open", "external:pickEditorExe", "update:get", "req:cancel"] {
        assert!(matches!(r.route(ch, &json!([])).await, Routed::Sidecar), "{ch}");
    }
    assert_eq!(r.rust_handled(), APP_CHANNELS.len() as u64);
}

#[tokio::test]
async fn 스위치가_꺼졌거나_포트가_없으면_전부_사이드카다() {
    let calls: Arc<Mutex<Calls>> = Arc::default();
    // ANTLER_RUST_GIT=0: 포트를 달아도(주입 자체를 무시한다) 사이드카다
    let off = router(false, Some(fake(&calls, Some("C:\\x"), false)));
    // 포트 없음
    let none = router(true, None);
    for r in [&off, &none] {
        for ch in APP_CHANNELS {
            assert!(matches!(r.route(ch, &json!(["x"])).await, Routed::Sidecar), "{ch}");
        }
    }
    let c = calls.lock().unwrap();
    assert!(c.picks.is_empty() && c.clips.is_empty() && c.links.is_empty());
}

#[tokio::test]
async fn 폴더_선택은_시작_위치_관문을_통과한_값만_대화상자에_넘긴다() {
    let calls: Arc<Mutex<Calls>> = Arc::default();
    let r = router(true, Some(fake(&calls, Some("C:\\repos\\a"), false)));
    // 통과: 로컬·리눅스·WSL 공유(대소문자·슬래시 방향 무관)
    for ok in ["C:\\Users\\a", "/home/a", "\\\\wsl.localhost\\Ubuntu\\home", "//WSL$/Ubuntu/home"] {
        assert_eq!(done(&r, "dialog:pickFolder", json!([ok])).await, json!({ "ok": true, "data": "C:\\repos\\a" }), "{ok}");
    }
    // 버린다(대화상자는 시작 위치 없이 뜬다): UNC·NT 네임스페이스·빈 문자열·문자열 아님·없음
    for bad in [json!(["\\\\host\\share"]), json!(["//host/share"]), json!(["\\??\\UNC\\host\\share"]), json!(["C:\\a\\??\\b"]), json!([""]), json!([5]), json!([null]), json!([]), json!([{ UNDEFINED_ARG: true }]), json!("x")] {
        assert_eq!(done(&r, "dialog:pickFolder", bad.clone()).await["ok"], json!(true), "{bad}");
    }
    let picks = calls.lock().unwrap().picks.clone();
    let want: Vec<Option<String>> = ["C:\\Users\\a", "/home/a", "\\\\wsl.localhost\\Ubuntu\\home", "//WSL$/Ubuntu/home"].iter().map(|s| Some(s.to_string())).chain((0..10).map(|_| None)).collect();
    assert_eq!(picks, want, "관문을 통과하지 못한 시작 위치가 포트로 넘어갔다");
}

#[tokio::test]
async fn 폴더_선택은_취소면_null_이고_실패는_오류_봉투다() {
    let calls = Arc::default();
    let cancel = router(true, Some(fake(&calls, None, false)));
    assert_eq!(done(&cancel, "dialog:pickFolder", json!([])).await, json!({ "ok": true, "data": null }));
    let failing = router(true, Some(fake(&calls, None, true)));
    assert_eq!(done(&failing, "dialog:pickFolder", json!([])).await, err("대화상자 실패"));
}

#[tokio::test]
async fn 클립보드는_문자열이고_백만_utf16_단위_이하만_쓴다() {
    let calls: Arc<Mutex<Calls>> = Arc::default();
    let r = router(true, Some(fake(&calls, None, false)));
    let void = json!({ "ok": true });
    assert_eq!(done(&r, "clipboard:writeText", json!(["hello"])).await, void);
    assert_eq!(done(&r, "clipboard:writeText", json!([""])).await, void);
    // NUL 은 TS 와 같게 걸러내지 않고 그대로 넘긴다
    assert_eq!(done(&r, "clipboard:writeText", json!(["a\u{0}b"])).await, void);
    assert_eq!(done(&r, "clipboard:writeText", json!(["a".repeat(1_000_000)])).await, void);
    // 이모지는 2단위: 500,000 개 = 1,000,000 단위(통과), 500,001 개(초과)
    assert_eq!(done(&r, "clipboard:writeText", json!(["😀".repeat(500_000)])).await, void);
    let denied = err("복사할 수 없는 내용입니다");
    assert_eq!(done(&r, "clipboard:writeText", json!(["a".repeat(1_000_001)])).await, denied);
    assert_eq!(done(&r, "clipboard:writeText", json!(["😀".repeat(500_001)])).await, denied);
    for bad in [json!([]), json!([null]), json!([5]), json!([["a"]]), json!([{ UNDEFINED_ARG: true }]), json!("a"), json!(null)] {
        assert_eq!(done(&r, "clipboard:writeText", bad.clone()).await, denied, "{bad}");
    }
    let c = calls.lock().unwrap();
    assert_eq!(c.clips.len(), 5);
    assert_eq!(c.clips[2], "a\u{0}b");
}

#[tokio::test]
async fn 클립보드_쓰기_실패는_오류_봉투다() {
    let calls = Arc::default();
    let r = router(true, Some(fake(&calls, None, true)));
    assert_eq!(done(&r, "clipboard:writeText", json!(["x"])).await, err("복사하지 못했습니다"));
}

#[tokio::test]
async fn 링크는_자격_증명_없는_https_만_정규화해_연다() {
    let calls: Arc<Mutex<Calls>> = Arc::default();
    let r = router(true, Some(fake(&calls, None, false)));
    assert_eq!(done(&r, "code:openLink", json!(["https://example.com"])).await, json!({ "ok": true }));
    assert_eq!(done(&r, "code:openLink", json!(["https://example.com/a?b=1#c"])).await, json!({ "ok": true }));
    let denied = err("https 주소만 열 수 있습니다");
    for bad in [
        json!(["http://example.com"]),
        json!(["javascript:alert(1)"]),
        json!(["file:///etc/passwd"]),
        json!(["https://user@example.com"]),
        json!(["https://user:pw@example.com"]),
        json!(["not a url"]),
        json!([""]),
        json!([format!("https://e.com/{}", "a".repeat(4096))]),
        json!([5]),
        json!([]),
        json!([{ UNDEFINED_ARG: true }]),
    ] {
        assert_eq!(done(&r, "code:openLink", bad.clone()).await, denied, "{bad}");
    }
    // 포트에는 정규화된 href 만 간다
    assert_eq!(calls.lock().unwrap().links, ["https://example.com/", "https://example.com/a?b=1#c"]);
    let failing = router(true, Some(fake(&calls, None, true)));
    assert_eq!(done(&failing, "code:openLink", json!(["https://example.com"])).await, err("주소를 열지 못했습니다"));
}

#[tokio::test]
async fn 타이틀바_테마는_불리언_값만_허용하고_아무것도_하지_않는다() {
    let calls: Arc<Mutex<Calls>> = Arc::default();
    let r = router(true, Some(fake(&calls, None, false)));
    for ok in [json!([{ "dark": true }]), json!([{ "dark": false, "dim": true }]), json!([{ "dark": true, "dim": false }]), json!([{ "dark": true, "extra": 1 }])] {
        assert_eq!(done(&r, "win:setTitleBarTheme", ok.clone()).await, json!({ "ok": true }), "{ok}");
    }
    let bad_input = err("잘못된 입력입니다");
    for bad in [
        json!([]),
        json!([null]),
        json!(["dark"]),
        json!([true]),
        json!([[]]),
        json!([{}]),
        json!([{ "dark": "true" }]),
        json!([{ "dark": 1 }]),
        json!([{ "dark": null }]),
        json!([{ "dark": true, "dim": null }]),
        json!([{ "dark": true, "dim": "yes" }]),
        json!([{ UNDEFINED_ARG: true }]),
        json!({ "dark": true }),
    ] {
        assert_eq!(done(&r, "win:setTitleBarTheme", bad.clone()).await, bad_input, "{bad}");
    }
    let c = calls.lock().unwrap();
    assert!(c.picks.is_empty() && c.clips.is_empty() && c.links.is_empty(), "타이틀바 테마가 포트를 불렀다");
}

#[test]
fn 앱_채널_모듈은_프로세스를_띄우지_않고_클립보드를_읽지_않는다() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(root.join("src/app_host.rs")).unwrap();
    let code: String = text.split("#[cfg(test)]").next().unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    for banned in ["Command", "std::process", "tokio::process", "tauri_plugin_opener", "tauri_plugin_clipboard", "read_text", "ShellExecute"] {
        assert!(!code.contains(banned), "app_host.rs 가 {banned} 를 쓴다");
    }
    // 링크 열기는 lib.rs 의 open_https(https 이중 확인) 하나이고, 열기 플러그인 호출 수는 api_route.rs 가 센다
    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains("open_link: std::sync::Arc::new(open_https)"));
    assert_eq!(lib.matches("write_text(").count(), 1, "클립보드 쓰기 포트는 한 곳이다");
    assert!(!lib.contains("read_text("), "클립보드 읽기 포트는 없다");
}
