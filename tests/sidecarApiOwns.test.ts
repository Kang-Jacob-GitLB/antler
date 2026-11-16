import { describe, expect, it } from 'vitest'
import { IPC_CHANNELS } from '../src/shared/ipc'
import { RUST_API_PREFIXES, rustOwnsApi, sidecarSkips } from '../src/sidecar/apiOwns'

describe('사이드카 jira 소유권(ANTLER_RUST_API)', () => {
  it('값이 정확히 1 일 때만 Rust 가 소유한다(없거나 다른 값이면 사이드카가 전부 맡는다)', () => {
    expect(rustOwnsApi('1')).toBe(true)
    for (const v of [undefined, '', '0', 'true', ' 1', '11']) expect(rustOwnsApi(v), String(v)).toBe(false)
  })

  it('소유하면 jira:* 만 처리하지 않고 다른 채널은 그대로 처리한다', () => {
    for (const ch of IPC_CHANNELS.filter((c) => c.startsWith('jira:'))) {
      expect(sidecarSkips(ch, true), ch).toBe(true)
      expect(sidecarSkips(ch, false), ch).toBe(false)
    }
    for (const ch of IPC_CHANNELS.filter((c) => !c.startsWith('jira:'))) expect(sidecarSkips(ch, true), ch).toBe(false)
  })

  it('접두어는 jira: 하나다(Rust 의 API_CHANNELS 와 같다 — src-tauri/tests/api_route.rs 가 대조한다)', () => {
    expect([...RUST_API_PREFIXES]).toEqual(['jira:'])
  })
})
