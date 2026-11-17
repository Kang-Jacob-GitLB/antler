import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { FetchService } from '../src/main/services/FetchService'
import { fetchOwner, parseRustFetchKinds } from '../src/sidecar/fetchOwns'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-owns-')), 'antler.config.json')

function setup(): { c: ConfigStore; ids: Record<'local' | 'wsl' | 'ssh', string> } {
  const c = new ConfigStore(tmp())
  const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a' })
  const wsl = c.addRepo({ sourceId: 'wsl:Ubuntu', path: '/home/me/b', name: 'b' }, { id: 'wsl:Ubuntu', kind: 'wsl', label: 'WSL: Ubuntu', distro: 'Ubuntu' })
  const ssh = c.addRepo({ sourceId: 'ssh:h', path: '/srv/c', name: 'c' }, { id: 'ssh:h', kind: 'ssh', label: 'SSH: h', sshHost: 'h' })
  return { c, ids: { local: local.id, wsl: wsl.id, ssh: ssh.id } }
}

describe('사이드카 fetch 소유권(ANTLER_RUST_FETCH_KINDS)', () => {
  it('값이 없거나 알 수 없는 토큰뿐이면 사이드카가 전부 맡는다(되돌리기)', () => {
    for (const v of [undefined, '', ' ', 'github', 'LOCAL', 'SSH', 'x,y', ',']) {
      expect(parseRustFetchKinds(v).size, String(v)).toBe(0)
    }
    const { c, ids } = setup()
    const owns = fetchOwner(() => c.get(), parseRustFetchKinds(undefined))
    for (const id of Object.values(ids)) expect(owns(id)).toBe(true)
  })

  it('local,wsl 이면 그 환경의 저장소는 맡지 않고 ssh 와 알 수 없는 저장소는 맡는다(ssh 스위치가 꺼진 구성)', () => {
    const kinds = parseRustFetchKinds('local,wsl')
    expect([...kinds].sort()).toEqual(['local', 'wsl'])
    // 공백·중복·알 수 없는 토큰(github·오타)은 무시한다 — github 는 Rust 가 맡을 수 없는 종류다
    expect([...parseRustFetchKinds(' local , github,local,x')]).toEqual(['local'])
    const { c, ids } = setup()
    const owns = fetchOwner(() => c.get(), kinds)
    expect(owns(ids.local)).toBe(false)
    expect(owns(ids.wsl)).toBe(false)
    expect(owns(ids.ssh)).toBe(true)
    expect(owns('missing')).toBe(true)
  })

  it('local,wsl,ssh 이면 ssh 저장소도 맡지 않는다(Rust 가 ssh 를 풀 때) — 알 수 없는 저장소만 맡는다', () => {
    const kinds = parseRustFetchKinds('local,wsl,ssh')
    expect([...kinds].sort()).toEqual(['local', 'ssh', 'wsl'])
    const { c, ids } = setup()
    const owns = fetchOwner(() => c.get(), kinds)
    for (const id of Object.values(ids)) expect(owns(id), id).toBe(false)
    expect(owns('missing')).toBe(true)
    // ssh 만 있는 값도 그 종류만 맡지 않는다
    const sshOnly = fetchOwner(() => c.get(), parseRustFetchKinds('ssh'))
    expect(sshOnly(ids.ssh)).toBe(false)
    expect(sshOnly(ids.local)).toBe(true)
  })

  it('소유하지 않은 저장소는 FetchService 가 fetch 도 자동 예약도 하지 않는다(이중 fetch 없음)', async () => {
    const { c, ids } = setup()
    const statuses: unknown[] = []
    // repos 는 소유하지 않은 저장소에서 건드리지 않는다 — 건드리면 이 테스트가 실패한다
    const repos = new Proxy({}, { get: (_t, p) => () => Promise.reject(new Error(`RepoService.${String(p)} 호출`)) })
    const svc = new FetchService(c, repos as never, (s) => statuses.push(s), () => {}, null, fetchOwner(() => c.get(), parseRustFetchKinds('local,wsl,ssh')))
    await expect(svc.fetch(ids.local, { manual: true })).rejects.toThrow(/맡지 않습니다/)
    await expect(svc.fetch(ids.wsl, { manual: false })).rejects.toThrow(/맡지 않습니다/)
    await expect(svc.fetch(ids.ssh, { manual: true })).rejects.toThrow(/맡지 않습니다/)
    svc.setActive(ids.local)
    svc.onFocus()
    expect(statuses).toEqual([])
    svc.dispose()
  })
})
