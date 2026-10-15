import { readFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseRefs, parseRemoteNames, parseTrack } from '../src/main/git/parsers/refs'
import { parseStashes } from '../src/main/git/parsers/stash'
import { parseWorktrees } from '../src/main/git/parsers/worktree'

const fixture = (name: string): Buffer => readFileSync(join(__dirname, 'fixtures', name))

describe('parseTrack', () => {
  it('ahead/behind/gone 을 해석한다', () => {
    expect(parseTrack('ahead 1, behind 2')).toEqual({ ahead: 1, behind: 2 })
    expect(parseTrack('ahead 3')).toEqual({ ahead: 3 })
    expect(parseTrack('behind 1')).toEqual({ behind: 1 })
    expect(parseTrack('gone')).toEqual({ gone: true })
    expect(parseTrack('')).toEqual({})
  })
})

describe('parseRefs (fixture)', () => {
  const remoteNames = parseRemoteNames(fixture('remotes.txt'))
  const { local, remotes, tags } = parseRefs(fixture('for-each-ref.bin'), remoteNames)

  it('리모트 이름 목록을 읽는다', () => {
    expect(remoteNames).toEqual(['origin', 'team/upstream'])
  })

  it('로컬 브랜치를 이름 순으로 돌려주고 HEAD 를 표시한다', () => {
    expect(local.map((b) => b.name)).toEqual(['behind-br', 'diverged', 'feature/이름-변경', 'gone-br', 'main', 'wt-branch'])
    expect(local.filter((b) => b.isHead).map((b) => b.name)).toEqual(['main'])
    expect(local.find((b) => b.name === 'main')!.fullName).toBe('refs/heads/main')
  })

  it('업스트림 ahead/behind/gone 을 채운다', () => {
    const by = (n: string) => local.find((b) => b.name === n)!
    expect(by('behind-br')).toMatchObject({ upstream: 'origin/main', behind: 1 })
    expect(by('behind-br').ahead).toBeUndefined()
    expect(by('diverged')).toMatchObject({ upstream: 'origin/diverged', ahead: 1, behind: 3 })
    expect(by('main')).toMatchObject({ upstream: 'origin/main', ahead: 4 })
    expect(by('gone-br')).toMatchObject({ upstream: 'origin/gone-br', upstreamGone: true })
    expect(by('wt-branch').upstream).toBeUndefined()
  })

  it("'/' 가 들어간 리모트 이름도 올바르게 묶고 origin/HEAD 는 뺀다", () => {
    expect(remotes.map((r) => r.name)).toEqual(['origin', 'team/upstream'])
    expect(remotes[0]!.branches.map((b) => b.name)).toEqual(['origin/diverged', 'origin/main'])
    expect(remotes[1]!.branches.map((b) => b.name)).toEqual(['team/upstream/feature/x', 'team/upstream/main'])
  })

  it('태그는 자연 정렬하고 annotated 는 peel 한 커밋 해시를 쓴다', () => {
    expect(tags.map((t) => t.name)).toEqual(['v0.1.0', 'v0.2.0', 'v0.9.0', 'v0.10.0'])
    const v1 = tags.find((t) => t.name === 'v0.1.0')!
    const v2 = tags.find((t) => t.name === 'v0.2.0')!
    expect(v1.annotated).toBe(false)
    expect(v2.annotated).toBe(true)
    // annotated 태그 객체의 해시가 아니라 커밋 해시여야 한다
    const raw = fixture('for-each-ref.bin').toString('utf8').split('\n').find((l) => l.startsWith('refs/tags/v0.2.0'))!.split('\0')
    expect(raw[1]).not.toBe(v2.hash)
    expect(v2.hash).toBe(raw[2])
  })
})

describe('parseWorktrees (fixture)', () => {
  const wts = parseWorktrees(fixture('worktree.bin'))

  it('첫 항목만 주 작업 트리다', () => {
    expect(wts).toHaveLength(3)
    expect(wts.map((w) => w.isMain)).toEqual([true, false, false])
    expect(wts[0]).toMatchObject({ branch: 'main', detached: false })
  })

  it('한글·공백 경로와 브랜치를 읽는다', () => {
    const w = wts.find((x) => basename(x.path) === '작업 트리')!
    expect(w).toMatchObject({ branch: 'wt-branch', detached: false, locked: false, prunable: false })
    expect(w.head).toMatch(/^[0-9a-f]{40}$/)
  })

  it('detached 작업 트리는 branch 가 null 이다', () => {
    const w = wts.find((x) => basename(x.path) === 'detached-wt')!
    expect(w).toMatchObject({ branch: null, detached: true })
  })

  it('locked/prunable/bare 속성과 빈 저장소의 0 해시를 처리한다', () => {
    const raw = 'worktree /a\0HEAD ' + '0'.repeat(40) + '\0bare\0\0worktree /b c\0HEAD abc\0detached\0locked 이유\0prunable gitdir file points to non-existent location\0\0'
    const [a, b] = parseWorktrees(raw)
    expect(a).toMatchObject({ path: '/a', head: null, bare: true, isMain: true })
    expect(b).toMatchObject({ path: '/b c', locked: true, prunable: true, detached: true, isMain: false })
  })
})

describe('parseStashes (fixture)', () => {
  const stashes = parseStashes(fixture('stash.bin'))

  it('스태시를 최신순으로 읽는다', () => {
    expect(stashes.map((s) => s.ref)).toEqual(['stash@{0}', 'stash@{1}'])
    expect(stashes.map((s) => s.index)).toEqual([0, 1])
    expect(stashes[0]!.hash).toMatch(/^[0-9a-f]{40}$/)
    expect(stashes[0]!.date).toBeGreaterThan(stashes[1]!.date)
  })

  it('한글 메시지를 보존한다', () => {
    expect(stashes[1]!.message).toBe('On main: 임시 저장: 한글 메시지')
  })

  it('빈 출력은 빈 목록이다', () => {
    expect(parseStashes(Buffer.alloc(0))).toEqual([])
  })
})
