//! renderer 가 부르는 Tauri command. 채널은 라우터가 모두 Rust 로 처리한다(`Router::invoke`).

use std::sync::OnceLock;
use std::time::Instant;

use tauri::State;

use crate::router::Router;

static START: OnceLock<Instant> = OnceLock::new();

/// 프로세스 시작 시각을 기록한다(run() 맨 앞에서 한 번)
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// IpcEnvelope 를 그대로 돌려준다. 오류는 envelope 안(ok:false)에 담겨 온다 — 목록에 없는 채널·Rust 가 풀지 못한 호출도 명시적 오류 envelope 다
#[tauri::command]
pub async fn antler_invoke(router: State<'_, Router>, channel: String, args: serde_json::Value) -> Result<serde_json::Value, String> {
    // 라우팅(취소 토큰 등록 포함)은 첫 await 전에 일어난다 — 뒤따르는 req:cancel 이 먼저 도착해도 router 의 tombstone 이 받아 준다
    Ok(router.invoke(&channel, &args).await)
}

/// 프로세스 시작 이후 경과 ms(콜드 스타트 측정용)
#[tauri::command]
pub fn antler_uptime_ms() -> u64 {
    START.get().map_or(0, |t| t.elapsed().as_millis() as u64)
}
