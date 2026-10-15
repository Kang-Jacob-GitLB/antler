import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { ConfigStore, sourceFor } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'
import { RepoWatcher } from '../src/main/services/RepoWatcher'

let root: string
let repo: string

const git = (...args: string[]) =>
  execFileSync('git', ['-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t', ...args], { stdio: 'ignore' })

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-watch-'))
  repo = join(root, 'repo')
  execFileSync('git', ['init', '-q', '-b', 'main', repo])
  git('commit', '-q', '--allow-empty', '-m', 'first')
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

const waitFor = async (cond: () => boolean, ms: number): Promise<boolean> => {
  const until = Date.now() + ms
  while (Date.now() < until) {
    if (cond()) return true
    await new Promise((r) => setTimeout(r, 50))
  }
  return cond()
}

describe('RepoWatcher (로컬)', () => {
  it('외부에서 커밋·브랜치를 만들면 알리고, .git 안의 다른 변화로는 알리지 않는다', async () => {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const svc = new RepoService(config)
    const r = await svc.addRepo({ kind: 'local', path: repo })
    const changes: string[] = []
    const watcher = new RepoWatcher(svc, (id) => changes.push(id))
    try {
      await watcher.watch(r.id)
      expect(watcher.current).toBe(r.id)

      // ref 가 바뀌지 않는 변화(설정 파일 수정)는 무시된다
      git('config', 'antler.test', '1')
      await new Promise((res) => setTimeout(res, 900))
      expect(changes).toEqual([])

      git('commit', '-q', '--allow-empty', '-m', 'second')
      expect(await waitFor(() => changes.length === 1, 5_000)).toBe(true)
      expect(changes[0]).toBe(r.id)

      git('branch', 'feature')
      expect(await waitFor(() => changes.length === 2, 5_000)).toBe(true)

      // 감시를 멈추면 더 알리지 않는다
      await watcher.watch(null)
      git('commit', '-q', '--allow-empty', '-m', 'third')
      await new Promise((res) => setTimeout(res, 900))
      expect(changes).toHaveLength(2)
    } finally {
      watcher.dispose()
      await svc.dispose()
    }
  }, 20_000)
})

describe('ConfigStore (M4)', () => {
  it('원격 소스는 결정적 id 로 만들고, 마지막 저장소를 지우면 함께 정리한다', () => {
    const store = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const wsl = sourceFor('wsl', 'Ubuntu-24.04')
    expect(wsl).toMatchObject({ id: 'wsl:Ubuntu-24.04', kind: 'wsl', distro: 'Ubuntu-24.04' })
    expect(sourceFor('ssh', 'devbox')).toMatchObject({ id: 'ssh:devbox', sshHost: 'devbox' })
    expect(() => sourceFor('ssh')).toThrow()

    const a = store.addRepo({ sourceId: wsl.id, path: '/home/me/a', name: 'a' }, wsl)
    const b = store.addRepo({ sourceId: wsl.id, path: '/home/me/b', name: 'b' }, wsl)
    expect(store.get().sources.filter((s) => s.id === wsl.id)).toHaveLength(1)

    store.removeRepo(a.id)
    expect(store.source(wsl.id)).toBeDefined()
    store.removeRepo(b.id)
    expect(store.source(wsl.id)).toBeUndefined()
    // 로컬 소스는 항상 남는다
    expect(store.source('local')).toBeDefined()
  })

  it('그룹·이름을 바꾸고, 빈 그룹은 해제한다', () => {
    const store = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = store.addRepo({ sourceId: 'local', path: 'C:\\x', name: 'x' })
    store.updateRepo(r.id, { group: '  회사  ', name: ' 새 이름 ' })
    expect(store.repo(r.id)).toMatchObject({ group: '회사', name: '새 이름' })
    store.updateRepo(r.id, { group: '' })
    expect(store.repo(r.id)!.group).toBeUndefined()
    store.updateRepo(r.id, { name: '   ' })
    expect(store.repo(r.id)!.name).toBe('새 이름')
  })
})
