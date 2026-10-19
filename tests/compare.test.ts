import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { COMMIT_FILES_LIMIT, compareCommits, fileDiff } from '../src/main/git/commands'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'
import { orderCompare } from '../src/renderer/src/lib/compare'

const exec = new LocalExecutor()
let root: string
let repo: string
const h: Record<string, string> = {}

const env = () => ({
  ...process.env,
  GIT_AUTHOR_NAME: '홍길동',
  GIT_AUTHOR_EMAIL: 'dev@example.com',
  GIT_COMMITTER_NAME: '홍길동',
  GIT_COMMITTER_EMAIL: 'dev@example.com',
  GIT_CONFIG_NOSYSTEM: '1',
  GIT_CONFIG_GLOBAL: join(root, '.no-global')
})
const git = (...args: string[]) => execFileSync('git', ['-c', 'core.autocrlf=false', ...args], { cwd: repo, env: env() }).toString('utf8').trim()
const write = (rel: string, body: string | Buffer) => {
  const p = join(repo, rel)
  mkdirSync(dirname(p), { recursive: true })
  writeFileSync(p, body)
}
const lines = (n: number, tag = 'line') => Array.from({ length: n }, (_, i) => `${tag} ${i + 1}`).join('\n') + '\n'
const commit = (name: string, msg: string) => {
  git('add', '-A')
  git('commit', '-q', '-m', msg)
  h[name] = git('rev-parse', 'HEAD')
}

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-cmp-'))
  repo = join(root, 'repo')
  mkdirSync(repo)
  git('init', '-q', '-b', 'main')
  write('a.txt', lines(10))
  write('keep.txt', 'keep\n')
  write('docs/한글 문서.md', '안녕\n')
  write('old name.ts', lines(20, 'rename me'))
  write('img.bin', Buffer.from([0, 1, 2, 3, 0, 255]))
  commit('c1', 'c1')

  write('a.txt', lines(10).replace('line 2\n', 'LINE 2\n'))
  commit('c2', 'c2')

  git('mv', 'old name.ts', 'new name.ts')
  write('new name.ts', lines(20, 'rename me').replace('rename me 5\n', 'changed\n'))
  write('added.txt', 'added\n')
  git('rm', '-q', 'keep.txt')
  write('img.bin', Buffer.from([0, 1, 2, 9, 0, 255, 7]))
  write('docs/한글 문서.md', '안녕\n하세요\n')
  commit('c3', 'c3')

  // c2 에서 갈라진 side 브랜치
  git('checkout', '-q', '-b', 'side', h.c2!)
  write('side.txt', 'side\n')
  commit('side', 'side')
  git('checkout', '-q', 'main')
}, 60_000)
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('compareCommits', () => {
  it('c1 → c3: 여러 커밋을 건너뛴 변경(수정·추가·삭제·이름 변경·바이너리·한글 경로)과 합계', async () => {
    const r = await compareCommits(exec, repo, h.c1!, h.c3!)
    expect(r).toMatchObject({ from: h.c1, to: h.c3, truncated: false })
    const by = Object.fromEntries(r.files.map((f) => [f.path, f]))
    expect(by['a.txt']).toMatchObject({ status: 'M', additions: 1, deletions: 1 })
    expect(by['added.txt']).toMatchObject({ status: 'A', additions: 1, deletions: 0 })
    expect(by['keep.txt']).toMatchObject({ status: 'D', additions: 0, deletions: 1 })
    expect(by['new name.ts']).toMatchObject({ status: 'R', oldPath: 'old name.ts', additions: 1, deletions: 1 })
    expect(by['img.bin']).toMatchObject({ status: 'M', binary: true, additions: null })
    expect(by['docs/한글 문서.md']).toMatchObject({ status: 'M', additions: 1, deletions: 0 })
    expect(r.additions).toBe(1 + 1 + 0 + 1 + 1)
    expect(r.deletions).toBe(1 + 0 + 1 + 1 + 0)
  })

  it('방향을 바꾸면 추가·삭제가 뒤집힌다', async () => {
    const fwd = await compareCommits(exec, repo, h.c2!, h.c3!)
    const rev = await compareCommits(exec, repo, h.c3!, h.c2!)
    expect(fwd.files.find((f) => f.path === 'added.txt')!.status).toBe('A')
    expect(rev.files.find((f) => f.path === 'added.txt')!.status).toBe('D')
    expect(rev.additions).toBe(fwd.deletions)
    expect(rev.deletions).toBe(fwd.additions)
  })

  it('갈라진 브랜치끼리는 양쪽 변경이 모두 나온다(트리 대 트리)', async () => {
    const r = await compareCommits(exec, repo, h.side!, h.c3!)
    expect(r.files.map((f) => `${f.status}:${f.path}`)).toContain('D:side.txt')
    expect(r.files.map((f) => `${f.status}:${f.path}`)).toContain('A:added.txt')
  })

  it('같은 커밋끼리는 빈 목록', async () => {
    const r = await compareCommits(exec, repo, h.c3!, h.c3!)
    expect(r).toMatchObject({ files: [], additions: 0, deletions: 0, truncated: false })
  })

  it('해시 형식이 아닌 rev 는 git 을 부르지 않고 거부한다', async () => {
    for (const bad of ['--output=x', 'HEAD', 'main', 'abc', 'abcd ef', '']) {
      await expect(compareCommits(exec, repo, bad, h.c3!)).rejects.toThrow(/invalid from hash/)
      await expect(compareCommits(exec, repo, h.c1!, bad)).rejects.toThrow(/invalid to hash/)
    }
    expect(existsSync(join(repo, 'x'))).toBe(false)
  })

  it('없는 해시는 실패한다', async () => {
    await expect(compareCommits(exec, repo, h.c1!, 'deadbeef')).rejects.toThrow()
  })

  it('파일 diff: fileDiff 에 hash=to, parent=from 을 주면 두 커밋 사이 그 파일의 diff 다', async () => {
    const d = await fileDiff(exec, repo, { hash: h.c3!, parent: h.c1!, path: 'a.txt' })
    expect(d.hunks[0]!.lines.filter((l) => l.t !== ' ').map((l) => `${l.t}${l.text}`)).toEqual(['-line 2', '+LINE 2'])
    const renamed = await fileDiff(exec, repo, { hash: h.c3!, parent: h.c1!, path: 'new name.ts', oldPath: 'old name.ts' })
    expect(renamed.oldPath).toBe('old name.ts')
    expect(renamed.hunks[0]!.lines.filter((l) => l.t !== ' ').map((l) => `${l.t}${l.text}`)).toEqual(['-rename me 5', '+changed'])
  })
})

describe('compareCommits 파일 수 상한', () => {
  it('상한을 넘으면 목록은 자르고 합계는 전체 기준으로 알린다', async () => {
    const big = mkdtempSync(join(root, 'big-'))
    const run = (...args: string[]) => execFileSync('git', ['-c', 'core.autocrlf=false', ...args], { cwd: big, env: env() }).toString('utf8').trim()
    run('init', '-q', '-b', 'main')
    writeFileSync(join(big, 'seed.txt'), 's\n')
    run('add', '-A')
    run('commit', '-q', '-m', 'seed')
    const from = run('rev-parse', 'HEAD')
    const n = COMMIT_FILES_LIMIT + 3
    mkdirSync(join(big, 'f'))
    for (let i = 0; i < n; i++) writeFileSync(join(big, 'f', `${i}.txt`), 'x\n')
    run('add', '-A')
    run('commit', '-q', '-m', 'many')
    const to = run('rev-parse', 'HEAD')
    const r = await compareCommits(exec, big, from, to)
    expect(r.truncated).toBe(true)
    expect(r.files).toHaveLength(COMMIT_FILES_LIMIT)
    expect(r.additions).toBe(n)
  }, 60_000)
})

describe('RepoService.compare', () => {
  let svc: RepoService | null = null
  afterEach(async () => {
    await svc?.dispose()
    svc = null
  })

  it('같은 쌍은 캐시에서 같은 객체를 돌려준다', async () => {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    svc = new RepoService(config)
    const a = await svc.compare(r.id, h.c1!, h.c3!)
    const b = await svc.compare(r.id, h.c1!.toUpperCase(), h.c3!)
    expect(b).toBe(a)
    // 방향이 다르면 다른 항목
    expect(await svc.compare(r.id, h.c3!, h.c1!)).not.toBe(a)
    expect(() => svc!.compare('nope', h.c1!, h.c3!)).toThrow(/unknown repo/)
  })
})

describe('orderCompare', () => {
  const index = new Map([
    ['new', 2],
    ['old', 9]
  ])
  it('목록에서 아래(오래된)에 있는 쪽이 from', () => {
    expect(orderCompare('new', 'old', index, false)).toEqual({ from: 'old', to: 'new' })
    expect(orderCompare('old', 'new', index, false)).toEqual({ from: 'old', to: 'new' })
  })
  it('방향 바꾸기', () => {
    expect(orderCompare('new', 'old', index, true)).toEqual({ from: 'new', to: 'old' })
  })
  it('목록에 없는 쪽은 더 오래된 것으로 본다', () => {
    expect(orderCompare('new', 'far', index, false)).toEqual({ from: 'far', to: 'new' })
    expect(orderCompare('far', 'new', index, false)).toEqual({ from: 'far', to: 'new' })
  })
  it('둘 다 모르면 앞의 인자가 from', () => {
    expect(orderCompare('x', 'y', index, false)).toEqual({ from: 'x', to: 'y' })
  })
})
