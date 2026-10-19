import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { MAX_PAGES_PER_SECTION } from '../src/main/github/launchpad'
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
  dir = mkdtempSync(join(tmpdir(), 'antler-lp-svc-'))
})
afterEach(() => rmSync(dir, { recursive: true, force: true }))

type Ref = { owner: string; repo: string; remote: string }
const ref = (owner: string, repo: string, remote = 'origin'): Ref => ({ owner, repo, remote })

/** 앱에 등록된 저장소 ID → 그 저장소의 GitHub remote 들(null 이면 읽기 실패, 'hang' 이면 응답 없음) */
type Remotes = Record<string, Ref[] | null | 'hang'>

function service(
  remotes: Remotes,
  responder: (req: RecordedRequest, n: number) => Response | undefined,
  signedIn = true
): { svc: GitHubService; calls: RecordedRequest[] } {
  const { fetch, calls } = fakeFetch(responder)
  const store = new TokenStore(dir, safe)
  if (signedIn) store.save('ghp_tok', 'pat')
  const repos = {
    ghRepo: async () => null,
    ghRemotes: (id: string, signal?: AbortSignal): Promise<Ref[]> => {
      const r = remotes[id]
      if (r === 'hang') return new Promise((_, reject) => signal?.addEventListener('abort', () => reject(new Error('aborted'))))
      if (r === null || r === undefined) return Promise.reject(new Error('unreachable'))
      return Promise.resolve(r)
    }
  }
  const config = { get: () => ({ ui: {}, repos: Object.keys(remotes).map((id) => ({ id })) }) }
  return { svc: new GitHubService({ config, repos, store, fetchImpl: fetch }), calls }
}

const issue = (n: number, repo: string, over: Record<string, unknown> = {}): Record<string, unknown> => ({
  number: n,
  title: `item ${n}`,
  state: 'open',
  user: { login: 'me', avatar_url: 'https://avatars.githubusercontent.com/u/1' },
  labels: [{ name: 'bug', color: 'ff0000' }],
  assignees: [],
  comments: 2,
  updated_at: `2026-01-0${n}T00:00:00Z`,
  created_at: '2026-01-01T00:00:00Z',
  html_url: `https://github.com/${repo}/pull/${n}`,
  repository_url: `https://api.github.com/repos/${repo}`,
  ...over
})

const qOf = (req: RecordedRequest): string => new URL(req.url).searchParams.get('q') ?? ''
const section = (req: RecordedRequest): 'authored' | 'review' | 'issues' => (qOf(req).includes('author:@me') ? 'authored' : qOf(req).includes('review-requested:@me') ? 'review' : 'issues')

describe('GitHubService.launchpad', () => {
  const remotes: Remotes = {
    r1: [ref('acme', 'app')],
    r2: [ref('acme', 'lib')],
    r3: [],
    // 같은 GitHub 저장소를 대소문자만 다르게 등록한 클론
    r4: [ref('Acme', 'App')],
    // 포크: origin 은 포크, upstream 이 원본
    r5: [ref('me', 'lib-fork'), ref('acme', 'lib', 'upstream')]
  }

  it('로그인하지 않았으면 호출 없이 needsLogin 으로 알린다', async () => {
    const { svc, calls } = service(remotes, () => json({ items: [] }), false)
    const res = await svc.launchpad('req1')
    expect(res).toMatchObject({ needsLogin: true, items: [], errors: [] })
    expect(calls).toHaveLength(0)
  })

  it('사용자 기준 검색을 섹션당 한 번만 보내고(repo: 없음) 등록된 저장소의 remote 와 맞춰 거른다', async () => {
    const { svc, calls } = service(remotes, (req) => {
      expect(new URL(req.url).pathname).toBe('/search/issues')
      expect(req.headers['authorization']).toBe('Bearer ghp_tok')
      expect(new URL(req.url).searchParams.get('per_page')).toBe('100')
      const s = section(req)
      if (s === 'authored') return json({ items: [issue(1, 'acme/app', { draft: true, pull_request: { merged_at: null } }), issue(5, 'stranger/other', { pull_request: {} })] })
      if (s === 'review') return json({ items: [issue(2, 'acme/lib', { pull_request: {} })] })
      return json({ items: [issue(3, 'ACME/App'), issue(4, 'unknown/thing')] })
    })
    const res = await svc.launchpad('req1')
    expect(calls).toHaveLength(3)
    expect(calls.map(qOf).sort()).toEqual(
      ['is:issue is:open assignee:@me archived:false', 'is:pr is:open author:@me archived:false', 'is:pr is:open review-requested:@me archived:false'].sort()
    )
    expect(calls.every((c) => !qOf(c).includes('repo:'))).toBe(true)
    expect(res).toMatchObject({ needsLogin: false, incomplete: false, skipped: 1, unresolved: 0, errors: [] })
    const by = Object.fromEntries(res.items.map((i) => [i.section, i]))
    expect(res.items).toHaveLength(3)
    // 대소문자만 다른 두 등록(r1·r4)은 같은 저장소로 묶인다
    expect(by['authored']).toMatchObject({ kind: 'pull', repoIds: ['r1', 'r4'], owner: 'acme', repo: 'app', number: 1, draft: true, state: 'open' })
    // upstream remote 로 맞는다(포크 r5 와 원본 r2 모두 등록 저장소)
    expect(by['review']).toMatchObject({ kind: 'pull', repoIds: ['r2', 'r5'], number: 2 })
    expect(by['issues']).toMatchObject({ kind: 'issue', repoIds: ['r1', 'r4'], number: 3, draft: false })
    expect(JSON.stringify(res)).not.toContain('ghp_tok')
  })

  it('저장소가 아무리 많아도 검색 수는 그대로이고 repo: 묶음이 없어 422 로 죽지 않는다', async () => {
    const many: Remotes = {}
    for (let i = 0; i < 400; i++) many[`x${i}`] = [ref('org', `repo-with-a-long-name-${i}`)]
    const { svc, calls } = service(many, (req) => (qOf(req).includes('repo:') ? json({ message: 'Validation Failed' }, { status: 422 }) : json({ items: [issue(1, 'org/repo-with-a-long-name-7', { pull_request: {} })] })))
    const res = await svc.launchpad('req1')
    expect(calls).toHaveLength(3)
    expect(res.errors).toEqual([])
    expect(res.items).toHaveLength(3)
    expect(res.items.every((i) => i.repoIds.length === 1 && i.repoIds[0] === 'x7')).toBe(true)
  })

  it('검색 한도에 걸린 섹션만 errors 로 남기고 나머지는 돌려준다', async () => {
    const { svc } = service(remotes, (req) =>
      section(req) === 'review'
        ? json({ message: 'rate limit' }, { status: 403, headers: { 'x-ratelimit-remaining': '0', 'x-ratelimit-limit': '30', 'x-ratelimit-reset': '1900000000', 'x-ratelimit-resource': 'search' } })
        : json({ items: section(req) === 'authored' ? [issue(1, 'acme/app', { pull_request: {} })] : [] })
    )
    const res = await svc.launchpad('req1')
    expect(res.items.map((i) => i.number)).toEqual([1])
    expect(res.errors).toHaveLength(1)
    expect(res.errors[0]).toContain('한도')
    expect(res.incomplete).toBe(true)
  })

  it('쪽 상한에 닿으면 incomplete 로 알리고 같은 항목은 한 번만 담는다', async () => {
    const { svc, calls } = service(remotes, (req) => {
      if (section(req) !== 'authored') return json({ items: [] })
      return json({ items: [issue(1, 'acme/app', { pull_request: {} })] }, { headers: { link: '<https://api.github.com/search/issues?page=9>; rel="next"' } })
    })
    const res = await svc.launchpad('req1')
    expect(calls.filter((c) => section(c) === 'authored')).toHaveLength(MAX_PAGES_PER_SECTION)
    expect(res.incomplete).toBe(true)
    expect(res.items.filter((i) => i.section === 'authored')).toHaveLength(1)
  })

  it('incomplete_results 와 1000건 초과는 incomplete 로 알린다', async () => {
    const a = service(remotes, () => json({ items: [], incomplete_results: true }))
    expect((await a.svc.launchpad('req1')).incomplete).toBe(true)
    const b = service(remotes, () => json({ items: [], total_count: 1500 }))
    expect((await b.svc.launchpad('req1')).incomplete).toBe(true)
  })

  it('remote 를 읽지 못한 저장소(오류·응답 없음)는 건너뛰고 unresolved 로 알린다', async () => {
    const { svc } = service({ r1: [ref('acme', 'app')], bad: null, slow: 'hang' }, (req) => json({ items: section(req) === 'authored' ? [issue(1, 'acme/app', { pull_request: {} })] : [] }))
    const t0 = Date.now()
    const res = await svc.launchpad('req1')
    // 응답 없는 저장소 하나가 제한 시간(3초) 이상 붙잡지 않는다
    expect(Date.now() - t0).toBeLessThan(8000)
    expect(res).toMatchObject({ unresolved: 2, skipped: 0 })
    expect(res.items.map((i) => i.number)).toEqual([1])
  })

  it('저장소가 하나도 GitHub 가 아니어도 검색은 한 번씩 하고 결과는 비어 있다', async () => {
    const { svc, calls } = service({ r3: [] }, () => json({ items: [issue(1, 'acme/app', { pull_request: {} })] }))
    const res = await svc.launchpad('req1')
    expect(calls).toHaveLength(3)
    expect(res).toMatchObject({ items: [], skipped: 1, incomplete: false })
  })

  it('401 이면 토큰을 다시 확인하고 에러를 던진다', async () => {
    const { svc } = service(remotes, () => json({ message: 'Bad credentials' }, { status: 401 }))
    await expect(svc.launchpad('req1')).rejects.toThrow('인증')
    expect((await svc.auth()).signedIn).toBe(false)
  })
})
