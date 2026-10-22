import { describe, expect, it } from 'vitest'
import type { FetchStatus, GhAccessibleRepo, Repo } from '../src/shared/types'
import {
  filterGhRepos,
  formatPushed,
  ghKey,
  githubEmptyState,
  shouldReloadAfterFetch,
  githubUrl,
  listOwners,
  parseOwnerFilter,
  registeredGhKeys
} from '../src/renderer/src/lib/ghRepoList'

const mk = (owner: string, repo: string, over: Partial<GhAccessibleRepo> = {}): GhAccessibleRepo => ({
  owner,
  repo,
  fullName: `${owner}/${repo}`,
  ownerType: 'User',
  private: false,
  fork: false,
  archived: false,
  defaultBranch: 'main',
  description: null,
  pushedAt: 0,
  htmlUrl: `https://github.com/${owner}/${repo}`,
  ...over
})

const list = [
  mk('me', 'antler', { description: 'Git GUI client' }),
  mk('me', 'dotfiles'),
  mk('Acme', 'Web-App', { ownerType: 'Organization', description: 'Customer portal' }),
  mk('acme', 'infra', { ownerType: 'Organization', archived: true }),
  mk('Beta-Org', 'tools', { ownerType: 'Organization', fork: true }),
  mk('friend', 'shared', { ownerType: 'User' })
]

describe('listOwners', () => {
  it('조직은 대소문자를 합쳐 이름순, 개인 계정 유무를 알려 준다', () => {
    expect(listOwners(list)).toEqual({ orgs: ['acme', 'Beta-Org'].sort((a, b) => a.localeCompare(b)), hasUser: true })
    expect(listOwners([mk('Acme', 'x', { ownerType: 'Organization' })])).toEqual({ orgs: ['Acme'], hasUser: false })
    expect(listOwners([])).toEqual({ orgs: [], hasUser: false })
  })
})

describe('filterGhRepos', () => {
  const all = { kind: 'all' } as const

  it('질의가 비면 전부, 순서는 그대로', () => {
    expect(filterGhRepos(list, '  ', all).map((r) => r.fullName)).toEqual(list.map((r) => r.fullName))
  })

  it('이름·owner·설명을 대소문자 무시로 찾는다', () => {
    expect(filterGhRepos(list, 'ANTLER', all).map((r) => r.repo)).toEqual(['antler'])
    expect(filterGhRepos(list, 'acme', all).map((r) => r.repo)).toEqual(['Web-App', 'infra'])
    expect(filterGhRepos(list, 'portal', all).map((r) => r.repo)).toEqual(['Web-App'])
    expect(filterGhRepos(list, 'gui', all).map((r) => r.repo)).toEqual(['antler'])
  })

  it('공백으로 나눈 낱말은 모두 들어 있어야 한다', () => {
    expect(filterGhRepos(list, 'acme web', all).map((r) => r.repo)).toEqual(['Web-App'])
    expect(filterGhRepos(list, 'acme antler', all)).toEqual([])
  })

  it('개인 계정 / 조직으로 거른다', () => {
    expect(filterGhRepos(list, '', { kind: 'user' }).map((r) => r.repo)).toEqual(['antler', 'dotfiles', 'shared'])
    expect(filterGhRepos(list, '', { kind: 'org', owner: 'ACME' }).map((r) => r.repo)).toEqual(['Web-App', 'infra'])
    expect(filterGhRepos(list, 'tools', { kind: 'org', owner: 'acme' })).toEqual([])
  })
})

describe('parseOwnerFilter', () => {
  it('select value 를 되돌린다', () => {
    expect(parseOwnerFilter('all')).toEqual({ kind: 'all' })
    expect(parseOwnerFilter('user')).toEqual({ kind: 'user' })
    expect(parseOwnerFilter('org:Beta-Org')).toEqual({ kind: 'org', owner: 'Beta-Org' })
    expect(parseOwnerFilter('???')).toEqual({ kind: 'all' })
  })
})

describe('registeredGhKeys', () => {
  it('원격 전용 저장소만 대소문자 무시 키로 모은다', () => {
    const repos = [
      { id: '1', sourceId: 'local', path: '/a', name: 'a' },
      { id: '2', sourceId: 'github', path: '/cache/2', name: 'Web-App', github: { owner: 'Acme', repo: 'Web-App' } }
    ] as Repo[]
    const keys = registeredGhKeys(repos)
    expect(keys.has(ghKey('acme', 'web-app'))).toBe(true)
    expect(keys.has(ghKey('acme', 'infra'))).toBe(false)
    expect(keys.size).toBe(1)
  })
})

describe('formatPushed', () => {
  const NOW = Date.UTC(2026, 9, 16, 12, 0, 0)
  const ago = (sec: number): number => NOW - sec * 1000

  it('초에서 년까지 상대 표기', () => {
    expect(formatPushed(ago(10), 'en', NOW)).toMatch(/now|sec/)
    expect(formatPushed(ago(5 * 60), 'en', NOW)).toMatch(/5 min/)
    expect(formatPushed(ago(5 * 3600), 'en', NOW)).toMatch(/5 hr/)
    expect(formatPushed(ago(3 * 86400), 'en', NOW)).toMatch(/3 days?/)
    expect(formatPushed(ago(90 * 86400), 'en', NOW)).toMatch(/3 mo/)
    expect(formatPushed(ago(800 * 86400), 'en', NOW)).toMatch(/2 yr/)
    expect(formatPushed(ago(800 * 86400), 'ko', NOW)).toMatch(/2년 전/)
  })
})

describe('githubEmptyState', () => {
  const st = (over: Partial<FetchStatus> = {}): FetchStatus => ({
    repoId: 'r',
    fetching: false,
    lastOkAt: null,
    error: null,
    blocked: null,
    authPaused: false,
    needsManual: false,
    noRemote: false,
    ...over
  })

  it('상태가 오기 전·받는 중·한 번도 성공 못 한 채 대기는 받는 중', () => {
    expect(githubEmptyState(undefined)).toBe('fetching')
    expect(githubEmptyState(st({ fetching: true }))).toBe('fetching')
    expect(githubEmptyState(st())).toBe('fetching')
  })

  it('오류·차단·인증 멈춤은 실패(받는 중이 아닐 때)', () => {
    expect(githubEmptyState(st({ error: 'boom' }))).toBe('failed')
    expect(githubEmptyState(st({ blocked: { keys: ['core.fsmonitor'], confirmable: false } }))).toBe('failed')
    expect(githubEmptyState(st({ authPaused: true }))).toBe('failed')
    expect(githubEmptyState(st({ error: 'boom', fetching: true }))).toBe('fetching')
  })

  it('받았는데 커밋이 없으면 빈 저장소', () => {
    expect(githubEmptyState(st({ lastOkAt: 1 }))).toBe('empty')
  })
})

describe('shouldReloadAfterFetch', () => {
  const seen = (repoId: string | null, fetching: boolean) => ({ repoId, fetching })
  it('같은 저장소에서 fetch 가 끝났고 목록이 비었으면 다시 읽는다', () => {
    expect(shouldReloadAfterFetch(seen('a', true), seen('a', false), 0)).toBe(true)
  })
  it('이력이 있으면 읽지 않는다(선택·스크롤 보존)', () => {
    expect(shouldReloadAfterFetch(seen('a', true), seen('a', false), 5)).toBe(false)
  })
  it('저장소 전환·fetch 시작·이전 기록 없음은 읽지 않는다', () => {
    expect(shouldReloadAfterFetch(seen('a', true), seen('b', false), 0)).toBe(false)
    expect(shouldReloadAfterFetch(seen('a', false), seen('a', true), 0)).toBe(false)
    expect(shouldReloadAfterFetch(null, seen('a', false), 0)).toBe(false)
    expect(shouldReloadAfterFetch(seen('a', true), seen(null, false), 0)).toBe(false)
  })
})

describe('githubUrl', () => {
  it('owner/repo 주소', () => {
    expect(githubUrl({ owner: 'a', repo: 'b' })).toBe('https://github.com/a/b')
  })
})
