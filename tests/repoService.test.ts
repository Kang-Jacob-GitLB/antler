import { execFileSync } from 'node:child_process'
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService, StaleLogError } from '../src/main/services/RepoService'

let root: string
let repoPath: string

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-svc-'))
  repoPath = join(root, 'repo')
  execFileSync('node', [resolve(__dirname, '../scripts/make-big-repo.mjs'), repoPath, '300'], { stdio: 'ignore' })
}, 60_000)
afterAll(() => rmSync(root, { recursive: true, force: true }))

let svc: RepoService | null = null
afterEach(async () => {
  await svc?.dispose()
  svc = null
})

function setup(): { svc: RepoService; repoId: string } {
  const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
  const repo = config.addRepo({ sourceId: 'local', path: repoPath, name: 'repo' })
  svc = new RepoService(config)
  return { svc, repoId: repo.id }
}

/** 다른 ref 로 요청해 기존 커서를 닫게 만든다(유휴 sweep 과 같은 효과) */
const retireMainCursor = (s: RepoService, repoId: string) => s.log(repoId, { skip: 0, count: 1, ref: 'v1.0.0' })

describe('RepoService.log', () => {
  it('커서가 닫힌 뒤에도 이력이 그대로면 끊김 없이 이어 읽는다', async () => {
    const { svc, repoId } = setup()
    const all = (await svc.log(repoId, { skip: 0, count: 1000 })).commits
    const p1 = await svc.log(repoId, { skip: 0, count: 100 })
    await retireMainCursor(svc, repoId)
    const p2 = await svc.log(repoId, { skip: 100, count: 100 })
    expect([...p1.commits, ...p2.commits].map((c) => c.hash)).toEqual(all.slice(0, 200).map((c) => c.hash))
  })

  it('페이지 사이에 커밋이 추가되면 StaleLogError 로 다시 읽게 한다', async () => {
    const { svc, repoId } = setup()
    await svc.log(repoId, { skip: 0, count: 100 })
    await retireMainCursor(svc, repoId)
    execFileSync('git', ['-C', repoPath, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'new'])
    await expect(svc.log(repoId, { skip: 100, count: 100 })).rejects.toBeInstanceOf(StaleLogError)
    // 처음부터 다시 읽으면 새 커밋이 맨 위에 온다
    const fresh = await svc.log(repoId, { skip: 0, count: 1 })
    expect(fresh.commits[0]!.subject).toBe('new')
  })
})

describe('ConfigStore', () => {
  it('손상된 설정 파일은 .bak 으로 보존하고 기본값으로 시작한다', () => {
    const dir = mkdtempSync(join(root, 'cfg-bad-'))
    const file = join(dir, 'antler.config.json')
    writeFileSync(file, '{ not json')
    const store = new ConfigStore(file)
    expect(store.get().repos).toEqual([])
    expect(readdirSync(dir).some((f) => f.includes('.corrupt-') && f.endsWith('.bak'))).toBe(true)
  })
})
