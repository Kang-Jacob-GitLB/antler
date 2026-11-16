// 사이드카가 jira:* 채널을 소유하는가. Rust 는 ANTLER_RUST_API 스위치가 켜져 있고 Jira 서비스를 만들었을 때만 사이드카를 env ANTLER_RUST_API=1 로 띄운다 —
// 이때 jira:* 는 Rust 라우터가 가로채므로 사이드카에 닿지 않아야 하고, 닿으면(방어) 처리하지 않고 오류로 돌려준다(토큰을 두 곳에서 다루지 않게).
// 값이 없으면(스위치 꺼짐·서비스 생성 실패) 사이드카가 지금처럼 전부 맡는다(되돌리기).

/** Rust 가 소유하는 채널 접두어(src-tauri/src/api_host.rs 의 API_CHANNELS 와 같다 — Rust 테스트가 대조한다) */
export const RUST_API_PREFIXES = ['jira:'] as const

/** env 값이 정확히 '1' 일 때만 Rust 가 소유한다. 알 수 없는 값이 사이드카의 처리를 끄는 일이 없게 */
export function rustOwnsApi(value: string | undefined): boolean {
  return value === '1'
}

/** 이 채널을 사이드카가 처리하지 않아야 하는가 */
export function sidecarSkips(channel: string, rustApi: boolean): boolean {
  return rustApi && RUST_API_PREFIXES.some((p) => channel.startsWith(p))
}
