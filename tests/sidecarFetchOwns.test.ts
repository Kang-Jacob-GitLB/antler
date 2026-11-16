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
    for (const v of [undefined, '', ' ', 'ssh', 'github', 'LOCAL', 'x,y', ',']) {
      expect(parseRustFetchKinds(v).size, String(v)).toBe(0)
    }
    const { c, ids } = setup()
    const owns = fetchOwner(() => c.get(), parseRustFetchKinds(undefined))
    for (const id of Object.values(ids)) expect(owns(id)).toBe(true)
  })

  it('local,wsl 이면 그 환경의 저장소는 맡지 않고 ssh 와 알 수 없는 저장소는 맡는다', () => {
    const kinds = parseRustFetchKinds('local,wsl')
    expect([...kinds].sort()).toEqual(['local', 'wsl'])
    // 공백·중복·알 수 없는 토큰은 무시한다(ssh 는 Rust 가 맡을 수 없는 종류다)
    expect([...parseRustFetchKinds(' local , ssh,local,github')]).toEqual(['local'])
    const { c, ids } = setup()
    const owns = fetchOwner(() => c.get(), kinds)
    expect(owns(ids.local)).toBe(false)
    expect(owns(ids.wsl)).toBe(false)
    expect(owns(ids.ssh)).toBe(true)
    expect(owns('missing')).toBe(true)
  })

  it('소유하지 않은 저장소는 FetchService 가 fetch 도 자동 예약도 하지 않는다(이중 fetch 없음)', async () => {
    const { c, ids } = setup()
    const statuses: unknown[] = []
    // repos 는 소유하지 않은 저장소에서 건드리지 않는다 — 건드리면 이 테스트가 실패한다
    const repos = new Proxy({}, { get: (_t, p) => () => Promise.reject(new Error(`RepoService.${String(p)} 호출`)) })
    const svc = new FetchService(c, repos as never, (s) => statuses.push(s), () => {}, null, fetchOwner(() => c.get(), parseRustFetchKinds('local,wsl')))
    await expect(svc.fetch(ids.local, { manual: true })).rejects.toThrow(/맡지 않습니다/)
    await expect(svc.fetch(ids.wsl, { manual: false })).rejects.toThrow(/맡지 않습니다/)
    svc.setActive(ids.local)
    svc.onFocus()
    expect(statuses).toEqual([])
    svc.dispose()
  })
})
