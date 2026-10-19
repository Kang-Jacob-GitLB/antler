import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { refs, search } from '../src/main/git/commands'

const solo = (...refs: string[]) => ({ solo: refs })

const exec = new LocalExecutor()
let root: string
let repo: string
let empty: string
const hashes: string[] = []

const env = (name: string) => ({
  ...process.env,
  GIT_AUTHOR_NAME: name,
  GIT_AUTHOR_EMAIL: `${name}@example.com`,
  GIT_COMMITTER_NAME: name,
  GIT_COMMITTER_EMAIL: `${name}@example.com`,
  GIT_CONFIG_NOSYSTEM: '1',
  GIT_CONFIG_GLOBAL: join(root, '.no-global')
})
const git = (cwd: string, author: string, ...args: string[]) =>
  execFileSync('git', args, { cwd, env: env(author) }).toString('utf8').trim()
const commit = (author: string, msg: string) => {
  git(repo, author, 'commit', '-q', '--allow-empty', '-m', msg)
  hashes.push(git(repo, author, 'rev-parse', 'HEAD'))
}

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-search-'))
  repo = join(root, 'repo')
  empty = join(root, 'empty')
  for (const d of [repo, empty]) {
    execFileSync('git', ['init', '-q', '-b', 'main', d])
  }
  commit('홍길동', '로그인 버그 수정')
  commit('alice', 'feat: add search')
  commit('alice', '--force 옵션 처리')
  git(repo, 'alice', 'checkout', '-q', '-b', 'side')
  commit('bob', 'side 전용 커밋')
  git(repo, 'alice', 'checkout', '-q', 'main')
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('search', () => {
  it('빈 질의는 git 을 실행하지 않고 빈 결과를 낸다', async () => {
    const dead = { run: () => Promise.reject(new Error('git 호출 금지')), stream: () => { throw new Error('git 호출 금지') } } as never
    expect(await search(dead, repo, '   ', undefined)).toEqual({ hashes: [], truncated: false })
  })

  it('한글 메시지로 찾는다', async () => {
    const r = await search(exec, repo, '로그인', undefined)
    expect(r).toEqual({ hashes: [hashes[0]], truncated: false })
  })

  it('작성자로 찾고 대소문자를 무시한다', async () => {
    const r = await search(exec, repo, 'ALICE', undefined)
    expect(r.hashes).toEqual([hashes[2], hashes[1]])
    expect((await search(exec, repo, '홍길동', undefined)).hashes).toEqual([hashes[0]])
  })

  it('메시지와 작성자 일치를 그래프 순서로 합친다', async () => {
    // 'side' 는 메시지(side 전용 커밋), 'bob' 은 작성자 — 둘 다 같은 커밋이라 중복 없이 하나
    expect((await search(exec, repo, 'side', undefined)).hashes).toEqual([hashes[3]])
    // 'a' 는 alice 두 커밋 + 메시지에 'a' 가 든 커밋들. 중복 없이 최신순(부모보다 자식 먼저)
    const r = await search(exec, repo, 'a', undefined)
    expect(new Set(r.hashes).size).toBe(r.hashes.length)
    const log = git(repo, 'alice', 'log', '--format=%H', '--date-order', '--branches', '--remotes', '--tags', 'HEAD').split('\n')
    expect(r.hashes).toEqual(log.filter((h) => r.hashes.includes(h)))
  })

  it('solo 를 주면 그 ref 이력에서만 찾는다', async () => {
    expect((await search(exec, repo, 'side', solo('refs/heads/main'))).hashes).toEqual([])
    expect((await search(exec, repo, 'side', solo('refs/heads/side'))).hashes).toEqual([hashes[3]])
  })

  it('해시 prefix 는 맨 앞에 온다', async () => {
    const r = await search(exec, repo, hashes[0]!.slice(0, 7), undefined)
    expect(r.hashes[0]).toBe(hashes[0])
    expect((await search(exec, repo, 'deadbeef', undefined)).hashes).toEqual([])
  })

  it("'-' 로 시작하는 질의가 옵션으로 해석되지 않는다", async () => {
    expect((await search(exec, repo, '--force', undefined)).hashes).toEqual([hashes[2]])
    expect((await search(exec, repo, '--output=x', undefined)).hashes).toEqual([])
    expect((await search(exec, repo, '-1', undefined)).hashes).toEqual([])
  })

  it('ref 필터 밖의 커밋은 해시로 찾아도 빠진다(그래프에서 찾아갈 수 없으므로)', async () => {
    const sideOnly = hashes[3]!
    expect((await search(exec, repo, sideOnly.slice(0, 12), solo('refs/heads/main'))).hashes).not.toContain(sideOnly)
    expect((await search(exec, repo, sideOnly.slice(0, 12), solo('refs/heads/side'))).hashes[0]).toBe(sideOnly)
  })

  it('스태시처럼 어느 브랜치에서도 닿지 않는 커밋은 해시로 찾아도 빠진다', async () => {
    const dir = join(root, 'stash-repo')
    execFileSync('git', ['init', '-q', '-b', 'main', dir])
    git(dir, 'alice', 'commit', '-q', '--allow-empty', '-m', 'base')
    writeFileSync(join(dir, 'f.txt'), 'x')
    git(dir, 'alice', 'add', 'f.txt')
    git(dir, 'alice', 'stash', 'push', '-q', '-m', 'wip')
    const stash = git(dir, 'alice', 'rev-parse', 'refs/stash')
    expect((await search(exec, dir, stash.slice(0, 10), undefined)).hashes).not.toContain(stash)
    // 그 스태시만 보는 필터에서는 찾아진다
    expect((await search(exec, dir, stash.slice(0, 10), solo(stash))).hashes[0]).toBe(stash)
  })

  it('커밋이 없는 저장소는 빈 결과다', async () => {
    expect(await search(exec, empty, '아무거나', undefined)).toEqual({ hashes: [], truncated: false })
  })
})

describe('refs', () => {
  it('빈 저장소도 던지지 않고 빈 스냅샷을 낸다', async () => {
    const s = await refs(exec, empty)
    expect(s).toMatchObject({ head: { branch: 'main', hash: null }, local: [], remotes: [], tags: [], stashes: [], submodules: [] })
    expect(s.worktrees).toHaveLength(1)
    expect(s.worktrees[0]).toMatchObject({ isMain: true, head: null })
  })

  it('브랜치·HEAD 를 채우고 스태시가 없으면 빈 배열이다', async () => {
    const s = await refs(exec, repo)
    expect(s.head).toEqual({ branch: 'main', hash: hashes[2] })
    expect(s.local.map((b) => b.name)).toEqual(['main', 'side'])
    expect(s.local.find((b) => b.isHead)!.name).toBe('main')
    expect(s.stashes).toEqual([])
    expect(s.worktrees).toHaveLength(1)
  })
})
