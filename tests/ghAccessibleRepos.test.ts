import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { MAX_ACCESSIBLE_PAGES } from '../src/main/github/accessibleRepos'
import { GitHubService } from '../src/main/github/service'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { fakeFetch, json, type RecordedRequest } from './ghFake'

const safe: SafeStorageLike = {
  isEncryptionAvailable: () => true,
  encryptString: (s) => Buffer.from(s, 'utf8'),
  decryptString: (b) => b.toString('utf8')
}

let dir: string
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-acc-'))
})
afterEach(() => rmSync(dir, { recursive: true, force: true }))

function service(responder: (req: RecordedRequest, n: number) => Response | undefined, signedIn = true): { svc: GitHubService; calls: RecordedRequest[] } {
  const { fetch, calls } = fakeFetch(responder)
  const store = new TokenStore(dir, safe)
  if (signedIn) store.save('ghp_tok', 'pat')
  const config = { get: () => ({ ui: {}, repos: [] }) }
  const repos = { ghRepo: async () => null }
  return { svc: new GitHubService({ config, repos, store, fetchImpl: fetch }), calls }
}

const raw = (owner: string, name: string, pushed: string, over: Record<string, unknown> = {}): Record<string, unknown> => ({
  name,
  full_name: `${owner}/${name}`,
  owner: { login: owner, type: 'User' },
  private: false,
  fork: false,
  archived: false,
  default_branch: 'main',
  description: null,
  pushed_at: pushed,
  html_url: `https://github.com/${owner}/${name}`,
  ...over
})

const NEXT = { link: '<https://api.github.com/user/repos?page=2>; rel="next"' }
const pageOf = (req: RecordedRequest): number => Number(new URL(req.url).searchParams.get('page') ?? '1')

describe('GitHubService.accessibleRepos', () => {
  it('응답을 매핑하고 pushedAt 내림차순으로 정렬하며, 형식이 이상한 항목은 건너뛴다', async () => {
    const { svc, calls } = service(() =>
      json([
        raw('me', 'old', '2026-01-01T00:00:00Z', { description: '옛것' }),
        null,
        { name: 'noowner', html_url: 'https://github.com/x/noowner' },
        raw('acme', 'new', '2026-03-01T00:00:00Z', { private: true, fork: true, archived: true, default_branch: 'trunk', owner: { login: 'acme', type: 'Organization' } })
      ])
    )
    const res = await svc.accessibleRepos('r1', false)
    expect(res.needsLogin).toBe(false)
    expect(res.truncated).toBe(false)
    expect(res.repos.map((r) => r.fullName)).toEqual(['acme/new', 'me/old'])
    expect(res.repos[0]).toEqual({
      owner: 'acme',
      repo: 'new',
      fullName: 'acme/new',
      ownerType: 'Organization',
      private: true,
      fork: true,
      archived: true,
      defaultBranch: 'trunk',
      description: null,
      pushedAt: Date.parse('2026-03-01T00:00:00Z'),
      htmlUrl: 'https://github.com/acme/new'
    })
    expect(res.repos[1]).toMatchObject({ ownerType: 'User', description: '옛것' })
    const q = new URL(calls[0]!.url)
    expect(q.pathname).toBe('/user/repos')
    expect(q.searchParams.get('affiliation')).toBe('owner,collaborator,organization_member')
    expect(q.searchParams.get('per_page')).toBe('100')
    expect(calls[0]!.headers['authorization']).toBe('Bearer ghp_tok')
  })

  it('Link next 가 있으면 다음 쪽까지 읽어 합친다', async () => {
    const { svc, calls } = service((req) =>
      pageOf(req) === 1 ? json([raw('me', 'a', '2026-01-01T00:00:00Z')], { headers: NEXT }) : json([raw('me', 'b', '2026-02-01T00:00:00Z')])
    )
    const res = await svc.accessibleRepos('r1', false)
    expect(calls).toHaveLength(2)
    expect(res.repos.map((r) => r.repo)).toEqual(['b', 'a'])
    expect(res.truncated).toBe(false)
  })

  it('쪽 수 상한에 걸리면 거기까지만 돌려주고 truncated 로 알린다', async () => {
    const { svc, calls } = service((req) => json([raw('me', `r${pageOf(req)}`, '2026-01-01T00:00:00Z')], { headers: NEXT }))
    const res = await svc.accessibleRepos('r1', false)
    expect(calls).toHaveLength(MAX_ACCESSIBLE_PAGES)
    expect(res.repos).toHaveLength(MAX_ACCESSIBLE_PAGES)
    expect(res.truncated).toBe(true)
  })

  it('로그인하지 않았으면 조회 없이 needsLogin', async () => {
    const { svc, calls } = service(() => json([]), false)
    const res = await svc.accessibleRepos('r1', false)
    expect(res).toMatchObject({ repos: [], needsLogin: true, truncated: false })
    expect(calls).toHaveLength(0)
  })

  it('5분 캐시를 쓰고, fresh 면 무시하고 다시 읽는다', async () => {
    const { svc, calls } = service(() => json([raw('me', 'a', '2026-01-01T00:00:00Z')]))
    await svc.accessibleRepos('r1', false)
    await svc.accessibleRepos('r2', false)
    expect(calls).toHaveLength(1)
    await svc.accessibleRepos('r3', true)
    expect(calls).toHaveLength(2)
  })

  it('로그아웃하면 캐시를 버린다', async () => {
    const { svc, calls } = service(() => json([raw('me', 'a', '2026-01-01T00:00:00Z')]))
    await svc.accessibleRepos('r1', false)
    svc.logout()
    const res = await svc.accessibleRepos('r2', false)
    expect(res.needsLogin).toBe(true)
    expect(calls).toHaveLength(1)
  })

  it('요청을 취소하면 AbortError 로 끝나고 캐시에 남기지 않는다', async () => {
    const { svc, calls } = service((req) => (pageOf(req) === 1 ? json([raw('me', 'a', '2026-01-01T00:00:00Z')], { headers: NEXT }) : json([])))
    const p = svc.accessibleRepos('r1', false)
    svc.cancel('r1')
    await expect(p).rejects.toThrow()
    const res = await svc.accessibleRepos('r2', false)
    expect(res.repos).toHaveLength(1)
    // 취소된 호출의 결과가 캐시에 남았다면 두 번째 호출은 네트워크를 타지 않는다
    expect(calls.length).toBeGreaterThanOrEqual(2)
  })

  it('401 이면 /user 로 다시 확인해 거부된 토큰을 버린다', async () => {
    const { svc } = service((req) => (new URL(req.url).pathname === '/user/repos' || new URL(req.url).pathname === '/user' ? json({ message: 'Bad credentials' }, { status: 401 }) : undefined))
    await expect(svc.accessibleRepos('r1', false)).rejects.toThrow()
    expect(svc.gitToken()).toBeNull()
  })

  it('gitToken 은 로그인 토큰을 돌려준다', () => {
    expect(service(() => json([])).svc.gitToken()).toBe('ghp_tok')
  })

  it('로그인하지 않았으면 gitToken 은 null', () => {
    expect(service(() => json([]), false).svc.gitToken()).toBeNull()
  })
})
