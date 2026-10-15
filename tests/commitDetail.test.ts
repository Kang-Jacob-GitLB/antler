import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { AbortError } from '../src/main/executors/GitExecutor'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { commitDetail, fileDiff } from '../src/main/git/commands'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

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
  root = mkdtempSync(join(tmpdir(), 'antler-m3-'))
  repo = join(root, 'repo')
  mkdirSync(repo)
  git('init', '-q', '-b', 'main')
  // 루트 커밋
  write('README.md', '# 샘플\n')
  write('a.txt', lines(40))
  write('docs/한글 문서 이름.md', '안녕\n')
  write('img.bin', Buffer.from([0, 1, 2, 3, 0, 255]))
  write('old name.ts', lines(20, 'rename me'))
  commit('root', 'init\n\n본문 첫 줄\n본문 둘째 줄')

  // 일반 커밋: 수정(hunk 2개) / 추가 / 삭제 / 이름 변경 / 바이너리 수정 / 한글 경로 수정
  write('a.txt', lines(40).replace('line 2\n', 'LINE 2\n').replace('line 38\n', 'LINE 38\n'))
  write('new.txt', 'brand new\n')
  git('rm', '-q', 'README.md')
  mkdirSync(join(repo, 'src'))
  git('mv', 'old name.ts', 'src/new name.ts')
  write('src/new name.ts', lines(20, 'rename me').replace('rename me 5\n', 'changed\n'))
  write('img.bin', Buffer.from([0, 1, 2, 9, 0, 255, 7]))
  write('docs/한글 문서 이름.md', '안녕\n하세요\n')
  commit('normal', 'feat: 여러 변경')

  // 거대한 파일 추가 (상한 테스트)
  write('big.txt', lines(5000, 'big'))
  commit('big', 'chore: big file')

  // merge: main 쪽과 side 쪽이 서로 다른 파일을 바꾼다
  git('checkout', '-q', '-b', 'side')
  write('side.txt', 'side\n')
  commit('side', 'side change')
  git('checkout', '-q', 'main')
  write('main-only.txt', 'main\n')
  commit('mainOnly', 'main change')
  git('merge', '-q', '--no-ff', 'side', '-m', 'Merge side')
  h.merge = git('rev-parse', 'HEAD')
}, 60_000)
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('commitDetail', () => {
  it('루트 커밋: 부모 없음, 전부 추가, 제목/본문 분리', async () => {
    const d = await commitDetail(exec, repo, h.root!)
    expect(d.hash).toBe(h.root)
    expect(d.parents).toEqual([])
    expect(d.baseParent).toBeNull()
    expect(d.subject).toBe('init')
    expect(d.body).toBe('본문 첫 줄\n본문 둘째 줄')
    expect(d.authorName).toBe('홍길동')
    expect(d.files.map((f) => f.status)).toEqual(['A', 'A', 'A', 'A', 'A'])
    expect(d.files.map((f) => f.path).sort()).toEqual(['README.md', 'a.txt', 'docs/한글 문서 이름.md', 'img.bin', 'old name.ts'])
    expect(d.files.find((f) => f.path === 'img.bin')).toMatchObject({ binary: true, additions: null, deletions: null })
    expect(d.truncated).toBe(false)
  })

  it('일반 커밋: 수정/추가/삭제/이름 변경/바이너리/한글 경로와 합계(바이너리 제외)', async () => {
    const d = await commitDetail(exec, repo, h.normal!)
    expect(d.baseParent).toBe(h.root)
    const by = Object.fromEntries(d.files.map((f) => [f.path, f]))
    expect(by['a.txt']).toMatchObject({ status: 'M', additions: 2, deletions: 2 })
    expect(by['new.txt']).toMatchObject({ status: 'A', additions: 1, deletions: 0 })
    expect(by['README.md']).toMatchObject({ status: 'D', additions: 0, deletions: 1 })
    expect(by['src/new name.ts']).toMatchObject({ status: 'R', oldPath: 'old name.ts', additions: 1, deletions: 1 })
    expect(by['src/new name.ts']!.similarity).toBeGreaterThan(50)
    expect(by['img.bin']).toMatchObject({ status: 'M', binary: true, additions: null })
    expect(by['docs/한글 문서 이름.md']).toMatchObject({ status: 'M', additions: 1, deletions: 0 })
    expect(d.additions).toBe(2 + 1 + 0 + 1 + 1)
    expect(d.deletions).toBe(2 + 0 + 1 + 1 + 0)
  })

  it('merge: 첫 부모 대비 변경만 보여 준다', async () => {
    const d = await commitDetail(exec, repo, h.merge!)
    expect(d.parents).toEqual([h.mainOnly, h.side])
    expect(d.baseParent).toBe(h.mainOnly)
    expect(d.files.map((f) => [f.status, f.path])).toEqual([['A', 'side.txt']])
  })

  it('잘못된 해시는 git 을 부르지 않고 거부한다', async () => {
    for (const bad of ['--output=x', 'HEAD', 'zzzz', 'abc', 'abcd ef', '']) {
      await expect(commitDetail(exec, repo, bad)).rejects.toThrow(/invalid commit hash/)
    }
    expect(existsSync(join(repo, 'x'))).toBe(false)
  })

  it('없는 해시는 GitError 로 실패한다', async () => {
    await expect(commitDetail(exec, repo, 'deadbeef')).rejects.toThrow()
  })
})

describe('fileDiff', () => {
  it('수정: hunk 2개와 줄 번호', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'a.txt' })
    expect(d.binary).toBe(false)
    expect(d.truncated).toBe(false)
    expect(d.hunks).toHaveLength(2)
    expect(d.hunks[0]!.lines.filter((l) => l.t === '-').map((l) => [l.text, l.o])).toEqual([['line 2', 2]])
    expect(d.hunks[0]!.lines.filter((l) => l.t === '+').map((l) => [l.text, l.n])).toEqual([['LINE 2', 2]])
    expect(d.hunks[1]!.lines.filter((l) => l.t === '+').map((l) => [l.text, l.n])).toEqual([['LINE 38', 38]])
  })

  it('추가: new file mode 와 전부 + 줄', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'new.txt' })
    expect(d.meta).toEqual(['new file mode 100644'])
    expect(d.hunks[0]!.lines).toEqual([{ t: '+', text: 'brand new', n: 1 }])
  })

  it('삭제', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'README.md' })
    expect(d.meta).toEqual(['deleted file mode 100644'])
    expect(d.hunks[0]!.lines).toEqual([{ t: '-', text: '# 샘플', o: 1 }])
  })

  it('이름 변경: oldPath 를 같이 주면 한 파일의 diff 로 나온다', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'src/new name.ts', oldPath: 'old name.ts' })
    expect(d.oldPath).toBe('old name.ts')
    expect(d.meta.some((m) => m.startsWith('rename from old name.ts'))).toBe(true)
    expect(d.hunks[0]!.lines.filter((l) => l.t !== ' ').map((l) => `${l.t}${l.text}`)).toEqual(['-rename me 5', '+changed'])
  })

  it('바이너리', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'img.bin' })
    expect(d.binary).toBe(true)
    expect(d.hunks).toEqual([])
  })

  it('한글 + 공백 경로', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: 'docs/한글 문서 이름.md' })
    expect(d.hunks[0]!.lines.filter((l) => l.t === '+').map((l) => l.text)).toEqual(['하세요'])
  })

  it('루트 커밋은 빈 트리 대비', async () => {
    const d = await fileDiff(exec, repo, { hash: h.root!, path: 'README.md' })
    expect(d.hunks[0]!.lines).toEqual([{ t: '+', text: '# 샘플', n: 1 }])
  })

  it('merge: 기본은 첫 부모, parent 로 둘째 부모 대비를 고를 수 있다', async () => {
    const first = await fileDiff(exec, repo, { hash: h.merge!, path: 'side.txt' })
    expect(first.hunks[0]!.lines).toEqual([{ t: '+', text: 'side', n: 1 }])
    const second = await fileDiff(exec, repo, { hash: h.merge!, path: 'main-only.txt', parent: h.side! })
    expect(second.hunks[0]!.lines).toEqual([{ t: '+', text: 'main', n: 1 }])
    // 첫 부모 대비로는 main-only.txt 가 변경되지 않았다
    const none = await fileDiff(exec, repo, { hash: h.merge!, path: 'main-only.txt' })
    expect(none.hunks).toEqual([])
  })

  it('pathspec magic 은 문자 그대로 해석돼 아무 파일에도 일치하지 않는다', async () => {
    for (const path of [':(top)', '*', ':!a.txt', ':(glob)**/*.txt', ':/']) {
      const d = await fileDiff(exec, repo, { hash: h.normal!, path })
      expect(d.hunks, path).toEqual([])
      expect(d.binary, path).toBe(false)
      expect(d.meta, path).toEqual([])
    }
  })

  it('--output=x 같은 경로는 옵션이 아니라 경로다', async () => {
    const d = await fileDiff(exec, repo, { hash: h.normal!, path: '--output=x' })
    expect(d.hunks).toEqual([])
    expect(existsSync(join(repo, 'x'))).toBe(false)
  })

  it('잘못된 해시·부모·경로를 거부한다', async () => {
    await expect(fileDiff(exec, repo, { hash: '--help', path: 'a' })).rejects.toThrow(/invalid commit hash/)
    await expect(fileDiff(exec, repo, { hash: h.normal!, path: 'a', parent: '--output=x' })).rejects.toThrow(/invalid parent/)
    await expect(fileDiff(exec, repo, { hash: h.normal!, path: '' })).rejects.toThrow(/invalid path/)
    await expect(fileDiff(exec, repo, { hash: h.normal!, path: 'a\0b' })).rejects.toThrow(/invalid path/)
  })

  it('상한: 기본은 3000 줄에서 자르고 전체 줄 수를 알려 주며, full 이면 전부 담는다', async () => {
    const part = await fileDiff(exec, repo, { hash: h.big!, path: 'big.txt' })
    expect(part.truncated).toBe(true)
    expect(part.hunks[0]!.lines).toHaveLength(3000)
    expect(part.totalLines).toBe(5000)
    expect(part.hunks[0]!.lines[2999]).toEqual({ t: '+', text: 'big 3000', n: 3000 })

    const full = await fileDiff(exec, repo, { hash: h.big!, path: 'big.txt', full: true })
    expect(full.truncated).toBe(false)
    expect(full.hunks[0]!.lines).toHaveLength(5000)
    expect(full.totalLines).toBe(5000)
  })
})

describe('RepoService 캐시·취소', () => {
  let svc: RepoService | null = null
  afterEach(async () => {
    await svc?.dispose()
    svc = null
  })
  function setup(): { svc: RepoService; repoId: string } {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    svc = new RepoService(config)
    return { svc, repoId: r.id }
  }

  it('같은 요청은 캐시에서 같은 객체를 돌려준다', async () => {
    const { svc, repoId } = setup()
    const a = await svc.commit(repoId, h.normal!)
    const b = await svc.commit(repoId, h.normal!.toUpperCase())
    expect(b).toBe(a)
    const d1 = await svc.fileDiff(repoId, { hash: h.normal!, path: 'a.txt' })
    const d2 = await svc.fileDiff(repoId, { hash: h.normal!, path: 'a.txt' })
    expect(d2).toBe(d1)
    // full 은 별도 항목
    const d3 = await svc.fileDiff(repoId, { hash: h.normal!, path: 'a.txt', full: true })
    expect(d3).not.toBe(d1)
  })

  it('진행 중인 같은 요청은 공유하고, 한쪽이 취소해도 다른 쪽은 끝까지 간다', async () => {
    const { svc, repoId } = setup()
    const p1 = svc.commit(repoId, h.merge!, 'r1')
    const p2 = svc.commit(repoId, h.merge!, 'r2')
    svc.cancel('r1')
    await expect(p1).rejects.toBeInstanceOf(AbortError)
    const d = await p2
    expect(d.hash).toBe(h.merge)
  })

  it('모든 구독자가 취소하면 요청이 중단되고, 이후 새 요청은 정상 동작한다', async () => {
    const { svc, repoId } = setup()
    const p1 = svc.fileDiff(repoId, { hash: h.big!, path: 'big.txt', full: true }, 'c1')
    svc.cancel('c1')
    await expect(p1).rejects.toBeInstanceOf(AbortError)
    const d = await svc.fileDiff(repoId, { hash: h.big!, path: 'big.txt', full: true })
    expect(d.totalLines).toBe(5000)
  })

  it('알 수 없는 저장소는 거부한다', () => {
    const { svc } = setup()
    expect(() => svc.commit('nope', h.root!)).toThrow(/unknown repo/)
  })
})
