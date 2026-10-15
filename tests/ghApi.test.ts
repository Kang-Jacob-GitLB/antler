import { describe, expect, it } from 'vitest'
import {
  aggregateChecks,
  aggregateReviews,
  checkRunState,
  commitStatusState,
  getComments,
  getFilters,
  getIssue,
  getPull,
  getPullCommits,
  getPullFiles,
  listIssues,
  listPulls,
  toEpoch
} from '../src/main/github/api'
import { GitHubClient } from '../src/main/github/client'
import { fakeFetch, json, type RecordedRequest } from './ghFake'

const slug = { owner: 'octo', repo: 'hello' }
const user = (login: string) => ({ login, avatar_url: `https://a/${login}.png` })
const pull = (n: number, extra: Record<string, unknown> = {}) => ({
  number: n,
  title: `PR ${n}`,
  state: 'open',
  draft: false,
  merged_at: null,
  user: user('alice'),
  labels: [{ name: 'bug', color: 'ff0000' }],
  assignees: [user('bob')],
  requested_reviewers: [],
  head: { ref: `feat-${n}`, sha: `sha${n}` },
  base: { ref: 'main' },
  updated_at: '2026-01-02T00:00:00Z',
  created_at: '2026-01-01T00:00:00Z',
  html_url: `https://github.com/octo/hello/pull/${n}`,
  ...extra
})
const path = (r: RecordedRequest) => new URL(r.url).pathname
const query = (r: RecordedRequest) => Object.fromEntries(new URL(r.url).searchParams)

describe('집계 규칙', () => {
  const rv = (login: string, state: string, t: number) => ({ user: user(login), state, submitted_at: new Date(t * 1000).toISOString() })

  it('리뷰: changes_requested > approved > commented > pending > none', () => {
    expect(aggregateReviews([rv('a', 'APPROVED', 1), rv('b', 'CHANGES_REQUESTED', 2)], 0)).toBe('changes_requested')
    expect(aggregateReviews([rv('a', 'APPROVED', 1), rv('b', 'COMMENTED', 2)], 0)).toBe('approved')
    expect(aggregateReviews([rv('a', 'COMMENTED', 1)], 2)).toBe('commented')
    expect(aggregateReviews([], 1)).toBe('pending')
    expect(aggregateReviews([], 0)).toBe('none')
  })

  it('리뷰: 사용자의 마지막 판정만 본다(변경 요청 후 승인 → approved)', () => {
    expect(aggregateReviews([rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'APPROVED', 2)], 0)).toBe('approved')
    expect(aggregateReviews([rv('a', 'APPROVED', 1), rv('a', 'CHANGES_REQUESTED', 2)], 0)).toBe('changes_requested')
  })

  it('리뷰: 이후의 댓글은 판정을 지우지 않고, DISMISSED 는 판정을 취소한다', () => {
    expect(aggregateReviews([rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'COMMENTED', 2)], 0)).toBe('changes_requested')
    expect(aggregateReviews([rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'DISMISSED', 2)], 0)).toBe('commented')
  })

  it('리뷰: 임시 저장(PENDING)은 무시한다', () => {
    expect(aggregateReviews([rv('a', 'PENDING', 1)], 0)).toBe('none')
  })

  it('체크 개별 상태', () => {
    expect(checkRunState({ status: 'queued' })).toBe('pending')
    expect(checkRunState({ status: 'in_progress' })).toBe('pending')
    expect(checkRunState({ status: 'completed', conclusion: 'success' })).toBe('success')
    expect(checkRunState({ status: 'completed', conclusion: 'skipped' })).toBe('neutral')
    expect(checkRunState({ status: 'completed', conclusion: 'neutral' })).toBe('neutral')
    for (const c of ['failure', 'cancelled', 'timed_out']) expect(checkRunState({ status: 'completed', conclusion: c })).toBe('failure')
    expect(commitStatusState('success')).toBe('success')
    expect(commitStatusState('pending')).toBe('pending')
    expect(commitStatusState('error')).toBe('failure')
    expect(commitStatusState('failure')).toBe('failure')
  })

  it('체크 집계: failure > pending > success, 없으면 none', () => {
    expect(aggregateChecks([])).toBe('none')
    expect(aggregateChecks(['success', 'failure', 'pending'])).toBe('failure')
    expect(aggregateChecks(['success', 'pending'])).toBe('pending')
    expect(aggregateChecks(['success', 'neutral'])).toBe('success')
    expect(aggregateChecks(['neutral', 'neutral'])).toBe('neutral')
  })

  it('날짜는 epoch seconds', () => {
    expect(toEpoch('2026-01-01T00:00:00Z')).toBe(1767225600)
    expect(toEpoch(null)).toBe(0)
  })
})

describe('listPulls', () => {
  it('비로그인: pulls 만 부르고 merged 상태를 해석한다', async () => {
    const { fetch, calls } = fakeFetch(() =>
      json([pull(1), pull(2, { state: 'closed', merged_at: '2026-01-03T00:00:00Z' }), pull(3, { state: 'closed' }), pull(4, { draft: true, user: null })], {
        headers: { link: '<https://api.github.com/x?page=2>; rel="next"' }
      })
    )
    const res = await listPulls(new GitHubClient(fetch), slug, { state: 'all', page: 2 })
    expect(calls).toHaveLength(1)
    expect(path(calls[0]!)).toBe('/repos/octo/hello/pulls')
    expect(query(calls[0]!)).toEqual({ state: 'all', per_page: '30', page: '2', sort: 'updated', direction: 'desc' })
    expect(res.hasMore).toBe(true)
    expect(res.items.map((p) => p.state)).toEqual(['open', 'merged', 'closed', 'open'])
    expect(res.items[3]).toMatchObject({ draft: true, author: { login: 'ghost' } })
    expect(res.items[0]).toMatchObject({
      number: 1,
      headRef: 'feat-1',
      baseRef: 'main',
      headSha: 'sha1',
      labels: [{ name: 'bug', color: 'ff0000' }],
      assignees: [{ login: 'bob', avatarUrl: 'https://a/bob.png' }],
      updatedAt: 1767312000,
      createdAt: 1767225600
    })
    expect(res.items[0]!.review).toBeUndefined()
    expect(res.items[0]!.checks).toBeUndefined()
  })

  it('로그인: 리뷰·체크를 보강한다(동시 6개 이하)', async () => {
    let active = 0
    let peak = 0
    const { fetch, calls } = fakeFetch(async (req) => {
      const p = path(req)
      if (p.endsWith('/pulls')) return json(Array.from({ length: 12 }, (_, i) => pull(i + 1, i === 0 ? { requested_reviewers: [user('r')] } : {})))
      active++
      peak = Math.max(peak, active)
      await new Promise((r) => setTimeout(r, 2))
      active--
      const m = /\/pulls\/(\d+)\/reviews$/.exec(p)
      if (m) return json(m[1] === '2' ? [{ user: user('x'), state: 'APPROVED', submitted_at: '2026-01-01T00:00:00Z' }] : [])
      if (p.endsWith('/check-runs')) return json({ check_runs: p.includes('sha3') ? [{ name: 'ci', status: 'completed', conclusion: 'failure' }] : [] })
      if (p.endsWith('/status')) return json({ state: 'pending', total_count: 0, statuses: [] })
      return undefined
    })
    const res = await listPulls(new GitHubClient(fetch, () => 'tok'), slug, { state: 'open' })
    expect(res.items[0]).toMatchObject({ review: 'pending', checks: 'none' })
    expect(res.items[1]).toMatchObject({ review: 'approved' })
    expect(res.items[2]).toMatchObject({ checks: 'failure' })
    // PR 당 reviews + check-runs + status
    expect(calls).toHaveLength(1 + 12 * 3)
    expect(peak).toBeLessThanOrEqual(6 * 3)
    expect(calls.every((r) => r.headers['authorization'] === 'Bearer tok')).toBe(true)
  })

  it('보강이 실패해도 목록은 돌려준다', async () => {
    const { fetch } = fakeFetch((req) => (path(req).endsWith('/pulls') ? json([pull(1)]) : json({ message: 'boom' }, { status: 500 })))
    const res = await listPulls(new GitHubClient(fetch, () => 'tok'), slug, { state: 'open' })
    expect(res.items).toHaveLength(1)
    expect(res.items[0]!.review).toBeUndefined()
  })

  it('라벨·담당자 필터가 있으면 issues 엔드포인트 파라미터로 거르고 PR 만 남긴다(보강 없음)', async () => {
    const { fetch, calls } = fakeFetch(() =>
      json([
        { number: 7, title: 'S', state: 'closed', draft: false, user: user('a'), labels: [{ name: 'bug', color: 'f00' }], assignees: [], comments: 3, updated_at: '2026-01-02T00:00:00Z', created_at: '2026-01-01T00:00:00Z', html_url: 'u', pull_request: { merged_at: '2026-01-02T00:00:00Z' } },
        { number: 8, title: '그냥 이슈', state: 'closed', user: user('a'), labels: [], assignees: [], comments: 0, updated_at: '2026-01-02T00:00:00Z', created_at: '2026-01-01T00:00:00Z', html_url: 'u' }
      ])
    )
    const res = await listPulls(new GitHubClient(fetch, () => 'tok'), slug, { state: 'closed', label: 'good "first"', assignee: 'none', page: 3 })
    expect(calls).toHaveLength(1)
    expect(path(calls[0]!)).toBe('/repos/octo/hello/issues')
    expect(query(calls[0]!)).toMatchObject({ state: 'closed', labels: 'good "first"', assignee: 'none', per_page: '30', page: '3', sort: 'updated', direction: 'desc' })
    expect(res.items.map((i) => i.number)).toEqual([7])
    expect(res.items[0]).toMatchObject({ number: 7, state: 'merged', comments: 3, headSha: '' })
  })

  it('라벨 이름에 검색 한정자를 넣어도 쿼리 언어로 해석되지 않는다(검색 API 를 쓰지 않음)', async () => {
    const { fetch, calls } = fakeFetch(() => json([]))
    await listPulls(new GitHubClient(fetch), slug, { state: 'all', label: 'x" repo:other/repo' })
    expect(path(calls[0]!)).toBe('/repos/octo/hello/issues')
    expect(query(calls[0]!)['labels']).toBe('x" repo:other/repo')
    expect(calls.some((c) => path(c) === '/search/issues')).toBe(false)
  })
})

describe('getPull', () => {
  it('상세·리뷰·체크를 합친다', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      const p = path(req)
      if (p.endsWith('/pulls/5')) return json(pull(5, { body_html: '<p>hi</p>', mergeable: true, additions: 3, deletions: 1, changed_files: 2, comments: 4, requested_reviewers: [user('rev')] }))
      if (p.endsWith('/reviews')) return json([{ user: user('x'), state: 'CHANGES_REQUESTED', submitted_at: '2026-01-02T00:00:00Z' }, { user: user('y'), state: 'PENDING' }])
      if (p.endsWith('/check-runs')) return json({ check_runs: [{ name: 'ci', status: 'completed', conclusion: 'success', html_url: 'https://c/1' }] })
      if (p.endsWith('/status')) return json({ statuses: [{ context: 'lint', state: 'pending', target_url: null }] })
      return undefined
    })
    const d = await getPull(new GitHubClient(fetch), slug, 5)
    expect(calls[0]!.headers['accept']).toBe('application/vnd.github.html+json')
    expect(d).toMatchObject({ bodyHtml: '<p>hi</p>', mergeable: true, additions: 3, deletions: 1, changedFiles: 2, comments: 4, review: 'changes_requested', checks: 'pending' })
    expect(d.reviews).toEqual([{ author: { login: 'x', avatarUrl: 'https://a/x.png' }, state: 'changes_requested', submittedAt: 1767312000 }])
    expect(d.checkRuns).toEqual([
      { name: 'ci', state: 'success', url: 'https://c/1' },
      { name: 'lint', state: 'pending' }
    ])
    expect(d.requestedReviewers).toEqual([{ login: 'rev', avatarUrl: 'https://a/rev.png' }])
  })
})

describe('파일·커밋·댓글·필터', () => {
  it('파일 목록은 Link 를 따라 이어 받는다', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      const page = Number(query(req)['page'])
      const files = [{ filename: `f${page}.ts`, status: 'modified', additions: 1, deletions: 2, patch: '@@' }]
      if (page === 2) files.push({ filename: 'new.ts', previous_filename: 'old.ts', status: 'renamed', additions: 0, deletions: 0 } as never)
      return json(files, page < 2 ? { headers: { link: '<x>; rel="next"' } } : {})
    })
    const files = await getPullFiles(new GitHubClient(fetch), slug, 9)
    expect(calls.map((c) => query(c)['page'])).toEqual(['1', '2'])
    expect(query(calls[0]!)['per_page']).toBe('100')
    expect(files.map((f) => f.path)).toEqual(['f1.ts', 'f2.ts', 'new.ts'])
    expect(files[2]).toEqual({ path: 'new.ts', oldPath: 'old.ts', status: 'renamed', additions: 0, deletions: 0 })
    expect(files[0]).toMatchObject({ patch: '@@' })
  })

  it('파일 목록은 30쪽에서 멈춘다', async () => {
    const { fetch, calls } = fakeFetch(() => json([], { headers: { link: '<x>; rel="next"' } }))
    await getPullFiles(new GitHubClient(fetch), slug, 9)
    expect(calls).toHaveLength(30)
  })

  it('커밋은 250개까지', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      const page = Number(query(req)['page'])
      const commits = Array.from({ length: 100 }, (_, i) => ({ sha: `c${page}-${i}`, commit: { message: 'm', author: { name: 'N', date: '2026-01-01T00:00:00Z' } }, author: i === 0 ? user('al') : null }))
      return json(commits, { headers: { link: '<x>; rel="next"' } })
    })
    const commits = await getPullCommits(new GitHubClient(fetch), slug, 9)
    expect(calls).toHaveLength(3)
    expect(commits).toHaveLength(250)
    expect(commits[0]).toMatchObject({ sha: 'c1-0', message: 'm', authorName: 'N', date: 1767225600, author: { login: 'al' } })
    expect(commits[1]!.author).toBeUndefined()
  })

  it('댓글은 html Accept 로 받는다', async () => {
    const { fetch, calls } = fakeFetch(() => json([{ user: user('c'), body_html: '<p>x</p>', created_at: '2026-01-01T00:00:00Z', html_url: 'u' }]))
    const res = await getComments(new GitHubClient(fetch), slug, 3)
    expect(path(calls[0]!)).toBe('/repos/octo/hello/issues/3/comments')
    expect(calls[0]!.headers['accept']).toBe('application/vnd.github.html+json')
    expect(res).toEqual([{ author: { login: 'c', avatarUrl: 'https://a/c.png' }, bodyHtml: '<p>x</p>', createdAt: 1767225600, htmlUrl: 'u' }])
  })

  it('필터: 라벨·담당자', async () => {
    const { fetch, calls } = fakeFetch((req) => (path(req).endsWith('/labels') ? json([{ name: 'bug', color: 'f00' }]) : json([user('bob')])))
    const res = await getFilters(new GitHubClient(fetch), slug)
    expect(calls.map(path).sort()).toEqual(['/repos/octo/hello/assignees', '/repos/octo/hello/labels'])
    expect(res).toEqual({ labels: [{ name: 'bug', color: 'f00' }], assignees: [{ login: 'bob', avatarUrl: 'https://a/bob.png' }] })
  })
})

describe('이슈', () => {
  const issue = (n: number, extra: Record<string, unknown> = {}) => ({ number: n, title: `I${n}`, state: 'open', user: user('a'), labels: [], assignees: [], comments: 0, updated_at: '2026-01-01T00:00:00Z', created_at: '2026-01-01T00:00:00Z', html_url: 'u', ...extra })

  it('PR 은 걸러내고 hasMore 는 Link 로 판단한다', async () => {
    const { fetch, calls } = fakeFetch(() => json([issue(1), issue(2, { pull_request: { url: 'x' } }), issue(3, { state: 'closed' })], { headers: { link: '<x>; rel="next"' } }))
    const res = await listIssues(new GitHubClient(fetch), slug, { state: 'all', label: 'bug', assignee: 'none', page: 2 })
    expect(query(calls[0]!)).toEqual({ state: 'all', labels: 'bug', assignee: 'none', per_page: '30', page: '2', sort: 'updated', direction: 'desc' })
    expect(res.items.map((i) => i.number)).toEqual([1, 3])
    expect(res.items[1]!.state).toBe('closed')
    // 30개 미만이어도 Link 가 있으면 더 있다
    expect(res.hasMore).toBe(true)
  })

  it('전부 PR 인 쪽도 Link 가 없으면 끝', async () => {
    const { fetch } = fakeFetch(() => json([issue(1, { pull_request: {} })]))
    const res = await listIssues(new GitHubClient(fetch), slug, { state: 'open' })
    expect(res).toEqual({ items: [], hasMore: false })
  })

  it('상세는 html 본문과 closedAt', async () => {
    const { fetch, calls } = fakeFetch(() => json(issue(4, { state: 'closed', body_html: '<p>b</p>', closed_at: '2026-01-02T00:00:00Z' })))
    const d = await getIssue(new GitHubClient(fetch), slug, 4)
    expect(calls[0]!.headers['accept']).toBe('application/vnd.github.html+json')
    expect(d).toMatchObject({ number: 4, bodyHtml: '<p>b</p>', closedAt: 1767312000, state: 'closed' })
  })
})
