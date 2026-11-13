//! renderer 가 부르는 Tauri command. 채널은 해석하지 않고 사이드카로 넘긴다.

use std::sync::OnceLock;
use std::time::Instant;

use tauri::State;

use crate::sidecar::Sidecar;

static START: OnceLock<Instant> = OnceLock::new();

/// 프로세스 시작 시각을 기록한다(run() 맨 앞에서 한 번)
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// IpcEnvelope 를 그대로 돌려준다. 오류는 envelope 안(ok:false)에 담겨 오고, Err 는 사이드카 통신 실패뿐이다
#[tauri::command]
pub async fn antler_invoke(
    sidecar: State<'_, Sidecar>,
    channel: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    sidecar.request(channel, args).await
}

/// 프로세스 시작 이후 경과 ms(콜드 스타트 측정용)
#[tauri::command]
pub fn antler_uptime_ms() -> u64 {
    START.get().map_or(0, |t| t.elapsed().as_millis() as u64)
}
