import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
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
const retireMainCursor = (s: RepoService, repoId: string) => s.log(repoId, { skip: 0, count: 1, scope: { solo: ['refs/tags/v1.0.0'] } })

// 실제 git 을 여러 번 실행한다 — 전체 테스트를 병렬로 돌릴 때 부하로 기본 5초를 넘길 수 있다
describe('RepoService.log', { timeout: 30_000 }, () => {
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

describe('RepoService 첫 페이지 문', { timeout: 30_000 }, () => {
  it('활성화하지 않은 저장소는 기다리지 않고, 활성화하면 첫 로그 페이지가 나간 뒤에 풀린다', async () => {
    const { svc, repoId } = setup()
    await svc.afterFirstPage(repoId) // 문이 없으면 즉시
    svc.armFirstPage(repoId)
    let opened = false
    const waiting = svc.afterFirstPage(repoId).then(() => (opened = true))
    await new Promise((r) => setTimeout(r, 50))
    expect(opened).toBe(false)
    await svc.log(repoId, { skip: 0, count: 10 })
    await waiting
    expect(opened).toBe(true)
  })

  it('감시가 계산한 ref 시그니처를 활동 조회가 다시 계산하지 않고 쓴다', async () => {
    const { svc, repoId } = setup()
    const first = await svc.signature(repoId)
    // 같은 값이 기억돼 있어, 동시 호출은 진행 중 계산과 합쳐져 같은 값을 돌려준다
    expect(await Promise.all([svc.signature(repoId), svc.signature(repoId)])).toEqual([first, first])
    const act = await svc.activity(repoId, undefined)
    expect(act).toBeTruthy()
  })

  it('StaleLogError 로 이력이 바뀐 것을 알면 감시가 기억한 낡은 시그니처 대신 새로 계산해 활동을 돌려준다', async () => {
    const { svc, repoId } = setup()
    await svc.signature(repoId) // 감시의 기준 시그니처
    const before = (await svc.activity(repoId, undefined)).total
    await svc.log(repoId, { skip: 0, count: 100 })
    await retireMainCursor(svc, repoId)
    execFileSync('git', ['-C', repoPath, '-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-q', '--allow-empty', '-m', 'newer'])
    // 감시는 아직 모르는 상태(기억한 시그니처가 낡았다)에서 renderer 가 이어 읽다가 이력 변경을 안다
    await expect(svc.log(repoId, { skip: 100, count: 100 })).rejects.toBeInstanceOf(StaleLogError)
    expect((await svc.activity(repoId, undefined)).total).toBe(before + 1)
  })
})

describe('RepoService.addRepo (local)', () => {
  it('하위 폴더를 골라도 최상위 경로로 등록하고, 다시 추가하면 같은 항목을 돌려준다', async () => {
    const { svc } = setup()
    const sub = join(repoPath, '하위 폴더')
    mkdirSync(sub, { recursive: true })
    const a = await svc.addRepo({ kind: 'local', path: sub })
    const b = await svc.addRepo({ kind: 'local', path: repoPath, group: '  ' })
    expect(a.path).toBe(process.platform === 'win32' ? repoPath.replace(/\//g, '\\') : repoPath)
    expect(b.id).toBe(a.id)
    expect(a.name).toBe('repo')
  })

  it('git 저장소가 아니면 실패한다', async () => {
    const { svc } = setup()
    await expect(svc.addRepo({ kind: 'local', path: mkdtempSync(join(tmpdir(), 'antler-notrepo-')) })).rejects.toThrow()
  })
})

describe('RepoService.scanRepos / addRepos (local)', { timeout: 30_000 }, () => {
  /** parent/{b(저장소), a(저장소), plain, .hidden(저장소)} */
  function makeParent(): string {
    const parent = mkdtempSync(join(root, 'parent-'))
    for (const name of ['b', 'a', '.hidden']) {
      mkdirSync(join(parent, name))
      execFileSync('git', ['init', '-q', join(parent, name)], { stdio: 'ignore' })
    }
    mkdirSync(join(parent, 'plain'))
    return parent
  }

  it('저장소가 아닌 폴더는 바로 아래 저장소들을 이름순으로 찾고, 숨김 폴더와 일반 폴더는 뺀다', async () => {
    const { svc } = setup()
    const parent = makeParent()
    const r = await svc.scanRepos({ kind: 'local', path: parent })
    expect(r.isRepo).toBe(false)
    expect(r.truncated).toBe(false)
    expect(r.repos).toEqual([
      { name: 'a', path: join(parent, 'a') },
      { name: 'b', path: join(parent, 'b') }
    ])
  })

  it('고른 폴더 자체가 저장소면 isRepo 로 알리고 하위는 보지 않는다', async () => {
    const { svc } = setup()
    const r = await svc.scanRepos({ kind: 'local', path: repoPath })
    expect(r.isRepo).toBe(true)
    expect(r.repos).toEqual([])
    // 저장소 안의 하위 폴더도 저장소로 본다(그 안의 중첩 저장소 목록을 띄우지 않는다)
    const sub = join(repoPath, 'libs')
    mkdirSync(join(sub, 'nested'), { recursive: true })
    execFileSync('git', ['init', '-q', join(sub, 'nested')], { stdio: 'ignore' })
    expect((await svc.scanRepos({ kind: 'local', path: sub })).isRepo).toBe(true)
  })

  it('여러 경로를 한 번에 추가하고, 실패한 경로는 나머지를 막지 않고 따로 돌려준다', async () => {
    const { svc } = setup()
    const parent = makeParent()
    const res = await svc.addRepos({ kind: 'local', paths: [join(parent, 'a'), join(parent, 'plain'), join(parent, 'b')], group: '묶음' })
    expect(res.added.map((r) => r.name)).toEqual(['a', 'b'])
    expect(res.added.every((r) => r.group === '묶음')).toBe(true)
    expect(res.failed.map((f) => f.path)).toEqual([join(parent, 'plain')])
    expect(res.failed[0]?.error).toBeTruthy()
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
