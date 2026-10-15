import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { parseWipStatus } from '../src/main/git/parsers/wipStatus'
import { wipFileDiff, wipStatus } from '../src/main/git/wip'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

const exec = new LocalExecutor()
let root: string
let repo: string

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

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-wip-'))
  repo = join(root, 'repo')
  mkdirSync(repo)
  git('init', '-q', '-b', 'main')
  write('a.txt', 'one\ntwo\nthree\n')
  write('staged me.txt', 'x\n')
  write('old name.ts', Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n') + '\n')
  write('docs/한글 문서.md', '안녕\n')
  write('both.txt', 'b1\n')
  git('add', '-A')
  git('commit', '-q', '-m', 'init')

  write('a.txt', 'one\nTWO\nthree\n') // 작업 트리만 수정
  write('staged me.txt', 'x\ny\n') // 스테이지만
  git('add', 'staged me.txt')
  git('mv', 'old name.ts', 'new name.ts') // 스테이지된 이름 변경
  write('docs/한글 문서.md', '안녕\n하세요\n') // 한글 경로, 작업 트리만
  write('both.txt', 'b1\nb2\n') // 스테이지 + 작업 트리
  git('add', 'both.txt')
  write('both.txt', 'b1\nb2\nb3\n')
  write('untracked 파일.txt', 'u1\nu2\n') // 추적 안 됨(공백·한글)
  write('dir/nested/deep.txt', 'deep\n') // -uall 은 폴더 안 파일을 하나씩 낸다
  write('bin.dat', Buffer.from([0, 1, 2, 3, 0, 255]))
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('wipStatus', () => {
  it('스테이지됨·변경됨·추적 안 됨을 나눠 담는다', async () => {
    const s = await wipStatus(exec, repo)
    const paths = (l: { path: string }[]) => l.map((f) => f.path).sort()
    expect(paths(s.staged)).toEqual(['both.txt', 'new name.ts', 'staged me.txt'].sort())
    expect(paths(s.unstaged)).toEqual(['a.txt', 'both.txt', 'docs/한글 문서.md'])
    expect(s.untracked.slice().sort()).toEqual(['bin.dat', 'dir/nested/deep.txt', 'untracked 파일.txt'])
    expect(s.untrackedMore).toBe(0)
    expect(s.conflicted).toBe(0)
  })

  it('이름 변경은 새 경로와 원래 경로를 함께 담는다', async () => {
    const s = await wipStatus(exec, repo)
    const r = s.staged.find((f) => f.status === 'R')
    expect(r).toMatchObject({ path: 'new name.ts', oldPath: 'old name.ts' })
  })

  it('MM(스테이지 + 작업 트리)은 두 목록에 모두 들어가고, 개수는 경로 기준이다', async () => {
    const s = await wipStatus(exec, repo)
    expect(s.staged.some((f) => f.path === 'both.txt' && f.status === 'M')).toBe(true)
    expect(s.unstaged.some((f) => f.path === 'both.txt' && f.status === 'M')).toBe(true)
    // 수정: a, both, 한글 문서, staged me, new name(이름 변경) / 추가: 추적 안 됨 3개
    expect(s.counts).toMatchObject({ modified: 5, added: 3, staged: 3, unstaged: 3, untracked: 3 })
  })

  it('추적 안 되는 파일은 상한까지만 담고 나머지는 개수로 알린다', async () => {
    const s = await wipStatus(exec, repo, undefined, 2)
    expect(s.untracked).toHaveLength(2)
    expect(s.untrackedMore).toBe(1)
    expect(s.counts.untracked).toBe(3)
    expect(s.counts.added).toBe(3)
  })

  it('충돌 항목(u)과 새 파일 스테이지(A.)를 해석한다', () => {
    const h = 'a'.repeat(40)
    const out = [
      `u UU N... 100644 100644 100644 100644 ${h} ${h} ${h} conflict file.txt`,
      `1 A. N... 000000 100644 100644 ${'0'.repeat(40)} ${h} added.txt`,
      `? 새 파일.txt`
    ].join('\0') + '\0'
    const s = parseWipStatus(Buffer.from(out))
    expect(s.conflicted).toBe(1)
    expect(s.unstaged).toMatchObject([{ status: 'U', path: 'conflict file.txt' }])
    expect(s.staged).toMatchObject([{ status: 'A', path: 'added.txt' }])
    expect(s.untracked).toEqual(['새 파일.txt'])
    expect(s.counts).toMatchObject({ modified: 1, added: 2 })
  })

  it('깨끗한 작업 트리는 빈 결과', async () => {
    const clean = join(root, 'clean')
    mkdirSync(clean)
    execFileSync('git', ['init', '-q', '-b', 'main'], { cwd: clean, env: env() })
    const s = await wipStatus(exec, clean)
    expect(s).toMatchObject({ staged: [], unstaged: [], untracked: [], conflicted: 0 })
  })
})

describe('wipFileDiff', () => {
  const added = (d: { hunks: { lines: { t: string; text: string }[] }[] }) => d.hunks.flatMap((h) => h.lines.filter((l) => l.t === '+').map((l) => l.text))
  const removed = (d: { hunks: { lines: { t: string; text: string }[] }[] }) => d.hunks.flatMap((h) => h.lines.filter((l) => l.t === '-').map((l) => l.text))

  it('작업 트리(unstaged) diff', async () => {
    const d = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'a.txt' })
    expect(removed(d)).toEqual(['two'])
    expect(added(d)).toEqual(['TWO'])
  })

  it('스테이지된 diff 는 인덱스와 HEAD 를 비교한다(MM 파일은 영역마다 다름)', async () => {
    const staged = await wipFileDiff(exec, repo, { area: 'staged', path: 'both.txt' })
    expect(added(staged)).toEqual(['b2'])
    const unstaged = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'both.txt' })
    expect(added(unstaged)).toEqual(['b3'])
  })

  it('스테이지된 이름 변경 diff', async () => {
    const d = await wipFileDiff(exec, repo, { area: 'staged', path: 'new name.ts', oldPath: 'old name.ts' })
    expect(d.meta.some((m) => m.startsWith('rename from'))).toBe(true)
  })

  it('한글 경로', async () => {
    const d = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'docs/한글 문서.md' })
    expect(added(d)).toEqual(['하세요'])
  })

  it('추적 안 되는 파일은 전부 추가된 diff (종료 코드 1 은 오류가 아니다)', async () => {
    const d = await wipFileDiff(exec, repo, { area: 'untracked', path: 'untracked 파일.txt' })
    expect(added(d)).toEqual(['u1', 'u2'])
    expect(removed(d)).toEqual([])
    expect(d.meta.some((m) => m.startsWith('new file mode'))).toBe(true)
    const deep = await wipFileDiff(exec, repo, { area: 'untracked', path: 'dir/nested/deep.txt' })
    expect(added(deep)).toEqual(['deep'])
  })

  it('추적 안 되는 바이너리는 binary 로 표시한다', async () => {
    const d = await wipFileDiff(exec, repo, { area: 'untracked', path: 'bin.dat' })
    expect(d.binary).toBe(true)
  })

  it('잘못된 입력은 거부한다', async () => {
    const bad = (req: object) => wipFileDiff(exec, repo, req as never)
    await expect(bad({ area: 'nope', path: 'a.txt' })).rejects.toThrow('invalid area')
    await expect(bad({ area: 'untracked', path: '../outside.txt' })).rejects.toThrow('invalid path')
    await expect(bad({ area: 'untracked', path: 'C:\\Windows\\win.ini' })).rejects.toThrow('invalid path')
    await expect(bad({ area: 'untracked', path: '/etc/passwd' })).rejects.toThrow('invalid path')
    await expect(bad({ area: 'untracked', path: '.git/config' })).rejects.toThrow('invalid path')
    await expect(bad({ area: 'unstaged', path: 'a\0b' })).rejects.toThrow('invalid path')
    await expect(bad({ area: 'unstaged', path: '' })).rejects.toThrow('invalid path')
  })
})

describe('RepoService WIP', () => {
  it('상태·diff 를 서비스 경로로 조회한다', async () => {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    const svc = new RepoService(config)
    try {
      const [a, b] = await Promise.all([svc.wipStatus(r.id, 'w1'), svc.wipStatus(r.id, 'w2')])
      expect(a.counts.untracked).toBe(3)
      expect(b.counts.staged).toBe(3)
      const d = await svc.wipDiff(r.id, { area: 'unstaged', path: 'a.txt' }, 'd1')
      expect(d.hunks.length).toBe(1)
    } finally {
      await svc.dispose()
    }
  })
})
