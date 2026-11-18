import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoWatcher } from '../src/main/services/RepoWatcher'
import { fetchOwner, parseRustFetchKinds } from '../src/sidecar/fetchOwns'

// 사이드카의 감시 소유권(Phase 6a PR 7b): Rust 가 감시를 맡는 종류의 저장소는 사이드카 RepoWatcher 가 id 만 기억하고 git 을 부르지 않는다.
// 소유 판정은 fetch 와 같은 술어(fetchOwner)다 — src/sidecar/index.ts 가 둘에 같은 함수를 준다.

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-watchowns-')), 'antler.config.json')

function setup(): { c: ConfigStore; ids: Record<'local' | 'wsl' | 'ssh', string> } {
  const c = new ConfigStore(tmp())
  const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a' })
  const wsl = c.addRepo({ sourceId: 'wsl:Ubuntu', path: '/home/me/b', name: 'b' }, { id: 'wsl:Ubuntu', kind: 'wsl', label: 'WSL: Ubuntu', distro: 'Ubuntu' })
  const ssh = c.addRepo({ sourceId: 'ssh:h', path: '/srv/c', name: 'c' }, { id: 'ssh:h', kind: 'ssh', label: 'SSH: h', sshHost: 'h' })
  return { c, ids: { local: local.id, wsl: wsl.id, ssh: ssh.id } }
}

/** RepoService 대역: 어떤 메서드든 불리면 기록한다. 맡지 않는 저장소에서는 하나도 불리면 안 된다. 감시가 쓰는 메서드는 감시가 멈추지 않을 만큼만 답한다 */
function fakeRepos(calls: string[]): never {
  return new Proxy({}, { get: (_t, p) => (...args: unknown[]) => {
    calls.push(`${String(p)}(${args.map((a) => (typeof a === 'string' ? a : '…')).join(',')})`)
    if (p === 'afterFirstPage') return Promise.resolve()
    if (p === 'repoKind') return null
    return Promise.reject(new Error(`RepoService.${String(p)} 호출`))
  } }) as never
}

describe('사이드카 RepoWatcher 의 감시 소유권', () => {
  it('Rust 가 맡는 저장소는 id 만 기억하고 기준 시그니처·git 디렉터리·폴링·포커스 확인 어느 것도 하지 않는다', async () => {
    const { c, ids } = setup()
    const calls: string[] = []
    const changes: string[] = []
    const watcher = new RepoWatcher(fakeRepos(calls), (id) => changes.push(id), fetchOwner(() => c.get(), parseRustFetchKinds('local,wsl,ssh')))
    for (const id of Object.values(ids)) {
      await watcher.watch(id)
      // 저장소 제거 흐름(handlers.ts 의 `watcher.current === repoId`)이 그대로 동작하도록 id 는 기억한다
      expect(watcher.current, id).toBe(id)
      // 포커스를 얻어도 확인하지 않는다
      watcher.setFocused(false)
      watcher.setFocused(true)
    }
    await new Promise((r) => setTimeout(r, 50))
    expect(calls).toEqual([])
    expect(changes).toEqual([])
    await watcher.watch(null)
    expect(watcher.current).toBeNull()
    watcher.dispose()
  })

  it('소유 판정은 종류별이다 — ssh 를 Rust 가 풀지 않는 구성(local,wsl)이면 ssh 저장소는 사이드카가 감시한다', async () => {
    const { c, ids } = setup()
    const calls: string[] = []
    const watcher = new RepoWatcher(fakeRepos(calls), () => {}, fetchOwner(() => c.get(), parseRustFetchKinds('local,wsl')))
    await watcher.watch(ids.local)
    await watcher.watch(ids.wsl)
    expect(calls).toEqual([])
    await watcher.watch(ids.ssh)
    // 감시를 맡았으니 첫 페이지 문 → 기준 시그니처 순으로 RepoService 를 부른다(대역은 실패해 감시는 폴링만 건다)
    expect(calls[0]).toBe(`afterFirstPage(${ids.ssh})`)
    expect(calls.some((x) => x.startsWith('signature('))).toBe(true)
    watcher.dispose()
  })

  it('env 가 없으면(ANTLER_RUST_GIT=0·Electron) 소유 술어가 없어도 사이드카가 전부 감시한다', async () => {
    const { c, ids } = setup()
    const calls: string[] = []
    const noEnv = new RepoWatcher(fakeRepos(calls), () => {}, fetchOwner(() => c.get(), parseRustFetchKinds(undefined)))
    await noEnv.watch(ids.local)
    expect(calls[0]).toBe(`afterFirstPage(${ids.local})`)
    noEnv.dispose()
    const calls2: string[] = []
    const electron = new RepoWatcher(fakeRepos(calls2), () => {})
    await electron.watch(ids.wsl)
    expect(calls2[0]).toBe(`afterFirstPage(${ids.wsl})`)
    electron.dispose()
  })

  it('맡지 않는 저장소에서 맡는 저장소로 바꾸면 감시를 시작하고, 반대로 바꾸면 이전 감시를 멈춘다', async () => {
    const { c, ids } = setup()
    const calls: string[] = []
    const watcher = new RepoWatcher(fakeRepos(calls), () => {}, fetchOwner(() => c.get(), parseRustFetchKinds('local')))
    await watcher.watch(ids.local)
    expect(calls).toEqual([])
    await watcher.watch(ids.wsl)
    expect(calls[0]).toBe(`afterFirstPage(${ids.wsl})`)
    const before = calls.length
    await watcher.watch(ids.local)
    expect(watcher.current).toBe(ids.local)
    // 포커스·폴링이 이전(wsl) 저장소를 확인하지 않는다
    watcher.setFocused(true)
    await new Promise((r) => setTimeout(r, 50))
    expect(calls.length).toBe(before)
    watcher.dispose()
  })
})
