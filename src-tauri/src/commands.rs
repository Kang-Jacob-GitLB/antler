//! renderer 가 부르는 Tauri command. 채널은 해석하지 않고 사이드카로 넘긴다.

use std::sync::OnceLock;
use std::time::Instant;

use tauri::State;

use crate::router::{Routed, Router};
use crate::sidecar::Sidecar;

static START: OnceLock<Instant> = OnceLock::new();

/// 프로세스 시작 시각을 기록한다(run() 맨 앞에서 한 번)
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// IpcEnvelope 를 그대로 돌려준다. 오류는 envelope 안(ok:false)에 담겨 오고, Err 는 사이드카 통신 실패뿐이다.
/// 라우터가 Rust 로 처리할 채널이면 그 envelope 를, 아니면(대부분) 사이드카에 인자를 그대로 넘긴다
#[tauri::command]
pub async fn antler_invoke(
    router: State<'_, Router>,
    sidecar: State<'_, Sidecar>,
    channel: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // 라우팅(취소 토큰 등록 포함)은 첫 await 전에 일어난다 — 뒤따르는 req:cancel 이 먼저 도착해도 router 의 tombstone 이 받아 준다
    match router.route(&channel, &args).await {
        Routed::Done(envelope) => Ok(envelope),
        Routed::Sidecar => sidecar.request(channel, args).await,
    }
}

/// 프로세스 시작 이후 경과 ms(콜드 스타트 측정용)
#[tauri::command]
pub fn antler_uptime_ms() -> u64 {
    START.get().map_or(0, |t| t.elapsed().as_millis() as u64)
}
