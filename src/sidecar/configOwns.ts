// 설정 파일(`antler.config.json`)을 누가 쓰는가. 한 실행 동안 쓰는 쪽은 하나다 — Rust 는 라우터가 켜져 있을 때(ANTLER_RUST_GIT 이 0 이 아닐 때) 설정을 소유하고
// (Phase 6b PR 9), 그때 사이드카를 env ANTLER_RUST_CONFIG=1 로 띄운다. 이 사이드카는 설정 파일을 열지도 쓰지도 않고(읽기 전용 빈 설정), 미러·fetch 설정을 보내지 않으며,
// 고아 캐시도 정리하지 않는다. 값이 없으면(ANTLER_RUST_GIT=0) 사이드카가 지금처럼 설정의 유일한 작성자다(되돌리기 — 6c 에서 사이드카를 없앨 때까지 산다).

/** Rust 가 응답하는 설정 소유 채널(src-tauri/src/router.rs 의 CONFIG_CHANNELS 와 같다 — Rust 시험이 대조한다). 라우터가 가로채므로 사이드카에 닿지 않아야 한다 */
export const RUST_CONFIG_CHANNELS = [
  'config:get',
  'config:setUi',
  'repos:add',
  'repos:addMany',
  'repos:addGithub',
  'repos:update',
  'repos:remove',
  'repos:watch',
  'external:open',
  'external:pickEditorExe',
  'req:cancel'
] as const

/** env 값이 정확히 '1' 일 때만 Rust 가 소유한다. 알 수 없는 값이 사이드카의 쓰기를 끄는 일이 없게(대신 두 쪽이 쓰는 일도 없다 — Rust 는 소유할 때만 '1' 을 준다) */
export function rustOwnsConfig(value: string | undefined): boolean {
  return value === '1'
}

/** 이 채널을 사이드카가 처리하지 않아야 하는가 */
export function sidecarSkipsConfig(channel: string, rustConfig: boolean): boolean {
  return rustConfig && (RUST_CONFIG_CHANNELS as readonly string[]).includes(channel)
}
