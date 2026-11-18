// 설정 파일(`antler.config.json`)을 누가 쓰는가. 한 실행 동안 쓰는 쪽은 하나다 — Rust 는 라우터가 켜져 있을 때(ANTLER_RUST_GIT 이 0 이 아닐 때) 설정을 소유하고
// (Phase 6b PR 9), 그때 사이드카를 env ANTLER_RUST_CONFIG=1 로 띄운다. PR 10 부터 이 사이드카는 UpdateService 만 조립하고 `update:*` 7채널만 받는다(설정·저장소·fetch·감시·GitHub·Jira 서비스를 만들지 않는다). 값이 없으면(ANTLER_RUST_GIT=0) 사이드카가 지금처럼 설정의 유일한 작성자다(되돌리기 — 6c 에서 사이드카를 없앨 때까지 산다).

/** Rust 가 응답하는 설정 소유 채널(src-tauri/src/router.rs 의 CONFIG_CHANNELS 와 같다 — Rust 시험이 대조한다). 라우터가 가로채므로 사이드카에 닿지 않아야 한다 — 닿아도 아래 `sidecarAccepts` 가 거부한다 */
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

/**
 * Rust 가 설정을 소유하는 실행에서 사이드카가 받는 **유일한** 채널: 앱 업데이트 7개(6c 에서 Rust 로 옮긴다). src-tauri/src/router.rs 의 UPDATE_CHANNELS 와 같다 —
 * Rust 시험(tests/sidecar_scope.rs)과 vitest 가 IPC_CHANNELS 의 `update:` 채널과 정확히 같음을 강제한다. 라우터는 이 밖의 채널을 사이드카로 보내지 않고(Rust 가 명시적 오류로 답한다), 사이드카도 이 밖은 거부한다.
 */
export const SIDECAR_UPDATE_CHANNELS = [
  'update:get',
  'update:check',
  'update:download',
  'update:cancel',
  'update:install',
  'update:dismiss',
  'update:releaseNotes'
] as const

/** 사이드카가 이 채널을 처리해도 되는가. 설정을 Rust 가 소유하면 update:* 뿐이고, 아니면(ANTLER_RUST_GIT=0) 허용 목록의 모든 채널이다 */
export function sidecarAccepts(channel: string, rustConfig: boolean): boolean {
  return !rustConfig || (SIDECAR_UPDATE_CHANNELS as readonly string[]).includes(channel)
}
