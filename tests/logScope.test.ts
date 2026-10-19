import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { commitActivity } from '../src/main/git/activity'
import { logArgs, scopeArgs, search } from '../src/main/git/commands'
import { parseLog } from '../src/main/git/parsers/log'
import { scopeKey } from '../src/shared/logScope'
import type { LogScope } from '../src/shared/types'

const exec = new LocalExecutor()
let root: string
let repo: string
let empty: string
/** 커밋 제목 → 해시 */
const at: Record<string, string> = {}

const git = (cwd: string, ...args: string[]) =>
  execFileSync('git', args, {
    cwd,
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: 't',
      GIT_AUTHOR_EMAIL: 't@example.com',
      GIT_COMMITTER_NAME: 't',
      GIT_COMMITTER_EMAIL: 't@example.com',
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: join(root, '.no-global')
    }
  })
    .toString('utf8')
    .trim()
const commit = (msg: string) => {
  git(repo, 'commit', '-q', '--allow-empty', '-m', msg)
  at[msg] = git(repo, 'rev-parse', 'HEAD')
}

/** 범위로 읽은 커밋 제목(목록 순서) */
async function subjects(scope: LogScope | undefined, cwd = repo): Promise<string[]> {
  const r = await exec.run(cwd, logArgs(scope, 0))
  expect(r.code).toBe(0)
  return parseLog(r.stdout).map((c) => c.subject)
}
const sorted = (a: string[]) => [...a].sort()

// 토폴로지:  m1 - m2 - m3(main, HEAD)
//                 \- f1(feat, tag t1, origin/r1)      o1(other, up/u1) 는 m2 에서 갈라짐
//            origin/deep/nested/x = f2(f1 위, 리모트에만 있음)
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-scope-'))
  repo = join(root, 'repo')
  empty = join(root, 'empty')
  execFileSync('git', ['init', '-q', '-b', 'main', repo])
  execFileSync('git', ['init', '-q', '-b', 'main', empty])
  commit('m1')
  commit('m2')
  git(repo, 'checkout', '-q', '-b', 'feat')
  commit('f1')
  git(repo, 'tag', 't1')
  git(repo, 'update-ref', 'refs/remotes/origin/r1', 'HEAD')
  commit('f2')
  git(repo, 'update-ref', 'refs/remotes/origin/deep/nested/x', 'HEAD')
  git(repo, 'reset', '-q', '--hard', 't1')
  git(repo, 'checkout', '-q', 'main')
  commit('m3')
  git(repo, 'checkout', '-q', '-b', 'other', 'main~1')
  commit('o1')
  git(repo, 'update-ref', 'refs/remotes/up/u1', 'HEAD')
  // clone 한 저장소처럼 origin/HEAD 는 심볼릭 ref 다(origin/r1 을 가리킨다)
  git(repo, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/r1')
  git(repo, 'checkout', '-q', 'main')
  writeFileSync(join(repo, 'w.txt'), 'x')
  git(repo, 'add', 'w.txt')
  git(repo, 'stash', 'push', '-q', '-m', 'wip')
  at.stash = git(repo, 'rev-parse', 'refs/stash')
}, 60_000)
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('scope: solo', () => {
  it('여러 브랜치를 동시에 solo 하면 그 ref 들에서 닿는 커밋의 합집합', async () => {
    expect(sorted(await subjects({ solo: ['refs/heads/other', 'refs/heads/feat'] }))).toEqual(['f1', 'm1', 'm2', 'o1'])
    expect(sorted(await subjects({ solo: ['refs/heads/main'] }))).toEqual(['m1', 'm2', 'm3'])
  })

  it('브랜치·태그·리모트 브랜치를 섞어 solo 한다', async () => {
    expect(sorted(await subjects({ solo: ['refs/tags/t1', 'refs/remotes/up/u1'] }))).toEqual(['f1', 'm1', 'm2', 'o1'])
  })

  it('리모트 전체 solo 는 하위 경로(deep/nested)까지 포함한다', async () => {
    expect(sorted(await subjects({ solo: ['refs/remotes/origin/*'] }))).toEqual(['f1', 'f2', 'm1', 'm2'])
    expect(sorted(await subjects({ solo: ['refs/remotes/origin/*', 'refs/heads/other'] }))).toEqual(['f1', 'f2', 'm1', 'm2', 'o1'])
  })

  it('스태시는 해시로 solo 한다(스태시 커밋과 그 부모들)', async () => {
    const s = await subjects({ solo: [at.stash!] })
    expect(s).toContain('On main: wip')
    expect(s).toContain('m3')
  })

  it('없어진 ref 만 남으면 HEAD 로 대체되지 않고 빈 목록이다', async () => {
    expect(await subjects({ solo: ['refs/heads/gone'] })).toEqual([])
    expect(sorted(await subjects({ solo: ['refs/heads/gone', 'refs/heads/other'] }))).toEqual(['m1', 'm2', 'o1'])
  })

  it('solo 가 있으면 hide 는 적용하지 않는다', async () => {
    expect(sorted(await subjects({ solo: ['refs/heads/main'], hide: ['refs/heads/main'] }))).toEqual(['m1', 'm2', 'm3'])
  })
})

describe('scope: hide', () => {
  const all = ['f1', 'f2', 'm1', 'm2', 'm3', 'o1']

  it('범위가 없거나 비어 있으면 기본 보기', async () => {
    expect(sorted(await subjects(undefined))).toEqual(all)
    expect(sorted(await subjects({}))).toEqual(all)
    expect(sorted(await subjects({ hide: [] }))).toEqual(all)
  })

  it('다른 ref(태그·리모트)가 닿는 커밋은 브랜치를 숨겨도 남는다', async () => {
    // feat 는 t1·origin/r1 과 같은 커밋(f1)을 가리킨다 — 브랜치만 숨겨도 f1 은 태그·리모트에서 닿아 남는다
    expect(sorted(await subjects({ hide: ['refs/heads/feat'] }))).toContain('f1')
  })

  it('여러 종류를 동시에 숨기면 그 ref 들에서만 닿던 커밋이 빠진다', async () => {
    const hide = ['refs/heads/feat', 'refs/tags/t1', 'refs/remotes/origin/r1', 'refs/remotes/origin/deep/nested/x']
    expect(sorted(await subjects({ hide }))).toEqual(['m1', 'm2', 'm3', 'o1'])
  })

  it('리모트 전체 hide (--exclude=origin/*) 는 하위 경로까지 뺀다', async () => {
    // f2 는 origin 의 하위 경로 브랜치에서만 닿는다
    expect(sorted(await subjects(undefined))).toContain('f2')
    const noOrigin = await subjects({ hide: ['refs/remotes/origin/*', 'refs/heads/feat', 'refs/tags/t1'] })
    expect(sorted(noOrigin)).toEqual(['m1', 'm2', 'm3', 'o1'])
    expect(sorted(await subjects({ hide: ['refs/remotes/up/*'] }))).toContain('f1')
    expect(sorted(await subjects({ hide: ['refs/remotes/up/*', 'refs/heads/other'] }))).not.toContain('o1')
  })

  it('현재 브랜치를 숨기면 includeHead=false 일 때만 HEAD 를 시작점에서 뺀다', async () => {
    expect(sorted(await subjects({ hide: ['refs/heads/main'] }))).toContain('m3')
    expect(sorted(await subjects({ hide: ['refs/heads/main'], includeHead: false }))).toEqual(['f1', 'f2', 'm1', 'm2', 'o1'])
  })

  it('origin/HEAD(심볼릭 ref)가 숨긴 리모트 브랜치를 다시 보이게 하지 않는다', async () => {
    // origin/HEAD → origin/r1 → f1. feat·t1 도 숨기면 f1 은 어디에서도 닿지 않아야 한다
    const hide = ['refs/heads/feat', 'refs/tags/t1', 'refs/remotes/origin/r1', 'refs/remotes/origin/deep/nested/x']
    expect(sorted(await subjects({ hide }))).toEqual(['m1', 'm2', 'm3', 'o1'])
    // 숨기지 않은 브랜치를 가리키는 origin/HEAD 를 시작점에서 빼도 그 브랜치는 그대로 보인다
    expect(sorted(await subjects({ hide: ['refs/remotes/up/u1', 'refs/heads/other'] }))).toContain('f1')
  })

  it('이름에 / 가 든 리모트도 전체 solo·hide 한다', async () => {
    const dir = join(root, 'slash-remote')
    execFileSync('git', ['init', '-q', '-b', 'main', dir])
    git(dir, 'commit', '-q', '--allow-empty', '-m', 'base')
    git(dir, 'checkout', '-q', '-b', 'tmp')
    git(dir, 'commit', '-q', '--allow-empty', '-m', 'alice-1')
    git(dir, 'update-ref', 'refs/remotes/team/alice/feat/x', 'HEAD')
    git(dir, 'checkout', '-q', 'main')
    git(dir, 'branch', '-q', '-D', 'tmp')
    const all = { solo: ['refs/remotes/team/alice/*'] }
    expect(await subjects(all, dir)).toEqual(['alice-1', 'base'])
    expect(await subjects({ hide: ['refs/remotes/team/alice/*'] }, dir)).toEqual(['base'])
    expect(await subjects(undefined, dir)).toEqual(['alice-1', 'base'])
  })

  it('모든 ref 를 숨기면 빈 목록(HEAD 로 대체되지 않는다)', async () => {
    const hide = [
      'refs/heads/main',
      'refs/heads/feat',
      'refs/heads/other',
      'refs/tags/t1',
      'refs/remotes/origin/*',
      'refs/remotes/up/*'
    ]
    expect(await subjects({ hide, includeHead: false })).toEqual([])
  })

  it('스태시 커밋은 기본 보기에 없으므로 hide 와 무관하다', async () => {
    expect(sorted(await subjects({ hide: ['refs/heads/other'] }))).not.toContain('wip on main: ' + at.m3!.slice(0, 7) + ' m3')
  })

  it('커밋이 없는 저장소도 오류 없이 빈 목록이다', async () => {
    expect(await subjects({ hide: ['refs/heads/main'], includeHead: false }, empty)).toEqual([])
  })
})

describe('scope: 검색·활동이 같은 범위를 쓴다', () => {
  it('search 는 solo 범위 밖의 커밋을 찾지 않는다', async () => {
    expect((await search(exec, repo, 'o1', { solo: ['refs/heads/main'] })).hashes).toEqual([])
    expect((await search(exec, repo, 'o1', { solo: ['refs/heads/main', 'refs/heads/other'] })).hashes).toEqual([at.o1])
  })

  it('search 는 hide 범위에서도 같다(해시 일치 포함)', async () => {
    const scope: LogScope = { hide: ['refs/heads/other', 'refs/remotes/up/*'] }
    expect((await search(exec, repo, 'o1', scope)).hashes).toEqual([])
    expect((await search(exec, repo, at.o1!.slice(0, 10), scope)).hashes).not.toContain(at.o1)
    expect((await search(exec, repo, 'm3', scope)).hashes).toEqual([at.m3])
  })

  it('활동 미니맵 합계가 목록 길이와 같다', async () => {
    for (const scope of [undefined, { solo: ['refs/heads/main', 'refs/heads/other'] }, { hide: ['refs/heads/other', 'refs/remotes/up/*'] }]) {
      const n = (await subjects(scope)).length
      expect((await commitActivity(exec, repo, scope)).total).toBe(n)
    }
  })
})

describe('scopeArgs 검증', () => {
  const bad = (scope: unknown) => expect(() => scopeArgs(scope as LogScope)).toThrow(/invalid/)

  it('옵션·임의 문자열·짧은 이름은 거절한다', () => {
    for (const item of ['--output=/tmp/x', '-n1', 'main', 'HEAD', 'stash@{0}', 'refs/heads/a b', 'refs/heads/a..b', 'refs/heads/a@{1}', 'refs/heads/', 'refs/heads/x^', 'refs/heads/x~1', 'refs/heads/x:y', 'refs/heads/x\\y', 'refs/other/x', '--glob=refs/*', 'abc123', 'refs/heads/x\n--all']) {
      bad({ solo: [item] })
      bad({ hide: [item] })
    }
  })

  it('* 는 리모트 전체 패턴에서만 허용한다', () => {
    for (const item of ['refs/*', 'refs/heads/*', 'refs/tags/*', 'refs/remotes/*', 'refs/remotes/*/*', 'refs/remotes/o/x*', 'refs/remotes//*']) {
      bad({ solo: [item] })
      bad({ hide: [item] })
    }
    expect(() => scopeArgs({ solo: ['refs/remotes/origin/*'] })).not.toThrow()
    // 이름에 '/' 가 든 리모트
    expect(() => scopeArgs({ solo: ['refs/remotes/team/alice/*'], hide: [] })).not.toThrow()
    expect(() => scopeArgs({ hide: ['refs/remotes/team/alice/*'] })).not.toThrow()
    for (const item of ['refs/remotes/a/../b/*', 'refs/remotes/a b/*', 'refs/remotes/a//b/*', 'refs/remotes/./*']) bad({ solo: [item] })
  })

  it('hide 는 해시를 받지 않는다', () => {
    bad({ hide: ['a'.repeat(40)] })
    expect(() => scopeArgs({ solo: ['a'.repeat(40)] })).not.toThrow()
  })

  it('형식 오류: 배열이 아니거나 항목이 문자열이 아니거나 includeHead 가 불리언이 아님', () => {
    bad({ solo: 'refs/heads/a' })
    bad({ solo: [1] })
    bad({ hide: [null] })
    bad({ hide: ['refs/heads/a'], includeHead: 'no' })
    bad(5)
    bad(null)
  })

  it('항목 수 상한', () => {
    const many = Array.from({ length: 201 }, (_, i) => `refs/heads/b${i}`)
    bad({ solo: many })
    bad({ hide: many })
    expect(() => scopeArgs({ solo: many.slice(0, 200) })).not.toThrow()
    bad({ solo: many.slice(0, 120), hide: many.slice(120, 201) })
  })

  it('rev 는 --end-of-options 뒤에, --glob 은 그 앞에 둔다', () => {
    const args = scopeArgs({ solo: ['refs/heads/-x', 'refs/remotes/o/*'] })
    expect(args.indexOf('--glob=refs/remotes/o/*')).toBeLessThan(args.indexOf('--end-of-options'))
    expect(args.indexOf('--end-of-options')).toBeLessThan(args.indexOf('refs/heads/-x'))
  })

  it('hide 의 --exclude 는 종류별로 해당 옵션 바로 앞에 둔다', () => {
    expect(scopeArgs({ hide: ['refs/heads/a', 'refs/remotes/o/*', 'refs/tags/t'] })).toEqual([
      '--ignore-missing',
      '--exclude=a',
      '--branches',
      '--exclude=o/*',
      '--remotes',
      '--exclude=t',
      '--tags',
      'HEAD'
    ])
  })
})

describe('scopeKey', () => {
  it('순서와 중복이 달라도 같은 범위는 같은 키', () => {
    expect(scopeKey({ solo: ['b', 'a', 'a'] })).toBe(scopeKey({ solo: ['a', 'b'] }))
    expect(scopeKey({ hide: ['b', 'a'] })).toBe(scopeKey({ hide: ['a', 'b'] }))
  })
  it('비어 있으면 빈 키, solo 가 있으면 hide 는 무시', () => {
    expect(scopeKey(undefined)).toBe('')
    expect(scopeKey({ solo: [], hide: [] })).toBe('')
    expect(scopeKey({ solo: ['a'], hide: ['x'] })).toBe(scopeKey({ solo: ['a'] }))
  })
  it('includeHead 가 다르면 다른 키', () => {
    expect(scopeKey({ hide: ['a'] })).not.toBe(scopeKey({ hide: ['a'], includeHead: false }))
  })
})
