import { describe, expect, it } from 'vitest'
import { IPC_CHANNELS } from '../src/shared/ipc'
import { RUST_API_PREFIXES, SIDECAR_KEPT_CHANNELS, rustOwnsApi, sidecarSkips } from '../src/sidecar/apiOwns'

describe('사이드카 jira·gh 소유권(ANTLER_RUST_API)', () => {
  it('값이 정확히 1 일 때만 Rust 가 소유한다(없거나 다른 값이면 사이드카가 전부 맡는다)', () => {
    expect(rustOwnsApi('1')).toBe(true)
    for (const v of [undefined, '', '0', 'true', ' 1', '11']) expect(rustOwnsApi(v), String(v)).toBe(false)
  })

  it('소유하면 jira:*·gh:*(gh:repo 제외)만 처리하지 않고 다른 채널은 그대로 처리한다', () => {
    const owned = (c: string): boolean => c.startsWith('jira:') || (c.startsWith('gh:') && c !== 'gh:repo')
    for (const ch of IPC_CHANNELS.filter(owned)) {
      expect(sidecarSkips(ch, true), ch).toBe(true)
      expect(sidecarSkips(ch, false), ch).toBe(false)
    }
    for (const ch of IPC_CHANNELS.filter((c) => !owned(c))) expect(sidecarSkips(ch, true), ch).toBe(false)
  })

  it('gh:repo 는 소유해도 사이드카가 처리한다(git 으로 remote 를 읽는 일이라 RepoService 가 맡는다)', () => {
    expect(sidecarSkips('gh:repo', true)).toBe(false)
    expect(sidecarSkips('gh:repo', false)).toBe(false)
    expect([...SIDECAR_KEPT_CHANNELS]).toEqual(['gh:repo'])
    // gh:* 는 gh:repo 를 뺀 23개다
    expect(IPC_CHANNELS.filter((c) => c.startsWith('gh:') && sidecarSkips(c, true))).toHaveLength(IPC_CHANNELS.filter((c) => c.startsWith('gh:')).length - 1)
  })

  it('접두어는 jira:·gh: 다(Rust 의 API_CHANNELS 와 같다 — src-tauri/tests/api_route.rs 가 대조한다)', () => {
    expect([...RUST_API_PREFIXES]).toEqual(['jira:', 'gh:'])
  })
})
