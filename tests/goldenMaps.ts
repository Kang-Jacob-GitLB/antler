// 응답 매퍼 골든의 기준 구현 구동기. tests/fixtures/mapCases.ts 의 입력 표(응답 원문)를 실제 TS 매퍼에 먹여 출력을 얻는다.
// 페이징이 든 조회 함수(getPull·listPulls·fetchLaunchpad…)는 가짜 client 로 실제 함수를 돌린다. src/main 은 건드리지 않는다.
// 결과는 { fn, in, out } 또는 { fn, in, err: true }(TS 가 던짐 — 메시지는 비교하지 않는다). 날짜는 시간대에 따라 달라지면 'tz'.
import { fetchAccessibleRepos, mapAccessibleRepo } from '../src/main/github/accessibleRepos'
import { aggregateChecks, aggregateReviews, checkRunState, commitStatusState, getComments, getFilters, getIssue, getPull, getPullCommits, getPullFiles, listIssues, listPulls, mapIssue, mapPull, mapSearchPull, mapUser, searchPullsByKey } from '../src/main/github/api'
import { CheckStateCache, fetchCheckDetails, fetchCommitChecks, listJobs, listRuns, rollupQuery, rollupState } from '../src/main/github/checks'
import type { GitHubClient } from '../src/main/github/client'
import { SECTIONS, fetchLaunchpad, slugFromRepositoryUrl } from '../src/main/github/launchpad'
import { adfToHtml } from '../src/main/jira/adf'
import { avatarOf, getComments as jiraComments, getDevInfo, getIssue as jiraIssue, getMyId, getTransitions, getWorklogs, mapSummary, myself, pickDateFields, search as jiraSearch, searchUsers } from '../src/main/jira/api'
import type { JiraClient, JiraCredential } from '../src/main/jira/client'
import { dateStrings, ghMapCases, ISO_MS, jiraMapCases, type MapCase } from './fixtures/mapCases'

export type MapOut = { fn: string; in: string[]; out: unknown } | { fn: string; in: string[]; err: true }

const slug = { owner: 'octo', repo: 'hello' }
type Routes = [string, unknown][]
type Res = { data: unknown; hasMore?: boolean }

/** 경로의 끝(쿼리 제외)이 route 이름인 응답을 돌려주는 가짜 GitHubClient */
function fake(routes: Routes | ((path: string, query: Record<string, unknown> | undefined) => Res), graphql?: unknown): GitHubClient {
  return {
    hasToken: () => true,
    get: async (path: string, opts?: { query?: Record<string, unknown> }): Promise<Res> => {
      if (typeof routes === 'function') return routes(path, opts?.query)
      for (const [k, v] of routes) if (path.endsWith(k)) return { data: v, hasMore: false }
      throw new Error(`no route ${path}`)
    },
    graphql: async () => graphql
  } as unknown as GitHubClient
}

const SHA = '0'.repeat(39) + '1'
const CHECK_ROUTES = (runs: unknown, status: unknown): Routes => [
  ['/check-runs', runs],
  ['/status', status]
]

type Impl = (a: unknown[]) => unknown | Promise<unknown>
const IMPLS: Record<string, Impl> = {
  mapPull: (a) => mapPull(a[0] as never),
  mapSearchPull: (a) => mapSearchPull(a[0] as never),
  mapIssue: (a) => mapIssue(a[0] as never),
  mapUser: (a) => mapUser(a[0] as never),
  aggregateReviews: (a) => aggregateReviews(a[0] as never, a[1] as number),
  checkRunState: (a) => checkRunState(a[0] as never),
  commitStatusState: (a) => commitStatusState(a[0] as never),
  aggregateChecks: (a) => aggregateChecks(a[0] as never),
  rollupState: (a) => rollupState(a[0] as never),
  rollupQuery: (a) => rollupQuery(a[0] as number),
  getIssue: (a) => getIssue(fake([['/issues/3', a[0]]]), slug, 3),
  getPull: (a) =>
    getPull(fake([['/pulls/7', a[0]], ['/reviews', a[1]], ...CHECK_ROUTES(a[2], a[3])]), slug, 7),
  listPulls: (a) =>
    listPulls(fake([['/pulls', a[0]], ['/reviews', a[1]], ...CHECK_ROUTES(a[2], a[3])]), slug, { state: 'open' }).then((r) => r.items),
  listPullsAnon: (a) => listPulls({ ...fake([['/pulls', a[0]]]), hasToken: () => false } as GitHubClient, slug, { state: 'open' }).then((r) => r.items),
  listPullsFiltered: (a) => listPulls(fake([['/issues', a[0]]]), slug, { state: 'open', label: 'bug' }).then((r) => r.items),
  listIssues: (a) => listIssues(fake([['/issues', a[0]]]), slug, { state: 'open' }).then((r) => r.items),
  searchPullsByKey: (a) => searchPullsByKey(fake([['/search/issues', a[0]]]), slug, 'ABC-1'),
  getPullFiles: (a) => getPullFiles(fake([['/files', a[0]]]), slug, 1),
  getPullCommits: (a) => getPullCommits(fake([['/commits', a[0]]]), slug, 1),
  getComments: (a) => getComments(fake([['/comments', a[0]]]), slug, 1),
  getFilters: (a) => getFilters(fake([['/labels', a[0]], ['/assignees', a[1]]]), slug),
  fetchCheckDetails: (a) => fetchCheckDetails(fake(CHECK_ROUTES(a[0], a[1])), slug, SHA),
  listRuns: (a) => listRuns(fake([['/actions/runs', a[0]]]), slug, 1).then((r) => r.items),
  listJobs: (a) => listJobs(fake([['/jobs', a[0]]]), slug, 77),
  mapRun: (a) => listRuns(fake([['/actions/runs', { workflow_runs: [a[0]] }]]), slug, 1).then((r) => r.items[0]),
  fetchCommitChecks: async (a) => {
    const n = a[1] as number
    const shas = Array.from({ length: n }, (_, i) => (i + 1).toString(16).padStart(40, '0'))
    const out = await fetchCommitChecks(fake([], a[0]), slug, shas, new CheckStateCache())
    return Object.entries(out)
  },
  mapAccessibleRepo: (a) => mapAccessibleRepo(a[0]),
  accessibleRepos: async (a) => {
    const pages = a.length === 1 && !Array.isArray(a[0]) ? [a[0]] : a
    const client = fake((_p, q) => ({ data: pages[(q?.['page'] as number) - 1], hasMore: (q?.['page'] as number) < pages.length }))
    const r = await fetchAccessibleRepos(client, new AbortController().signal)
    return { repos: r.repos, truncated: r.truncated }
  },
  slugFromRepositoryUrl: (a) => slugFromRepositoryUrl(a[0] as never),
  sections: () => SECTIONS,
  fetchLaunchpad: async (a) => {
    const hits = a[0] as { authored: unknown[]; review: unknown[]; issues: unknown[] }
    const remotes = a[1] as Record<string, { owner: string; repo: string }[]>
    const bySection = (q: string): unknown[] => (q.includes('author:@me') ? hits.authored : q.includes('review-requested') ? hits.review : hits.issues)
    const client = fake((_p, q) => ({ data: { items: bySection(q?.['q'] as string) }, hasMore: false }))
    const r = await fetchLaunchpad(client, Object.keys(remotes), async (id) => remotes[id]!, new AbortController().signal)
    return r.items
  },
  searchPage: async (a) => {
    // 검색 한 쪽의 항목·incomplete 만 본다(searchSection 은 비공개라 fetchLaunchpad 의 결과로 확인한다)
    const remotes = { r: [{ owner: 'octo', repo: 'hello' }] }
    const client = fake(() => ({ data: a[0], hasMore: false }))
    const r = await fetchLaunchpad(client, Object.keys(remotes), async (id) => remotes[id as 'r']!, new AbortController().signal)
    return { n: r.items.length, incomplete: r.incomplete }
  }
}

// ── Jira ──────────────────────────────────────────────────────────────────

const SITE = 'https://x.example.net'
const cred = (kind: unknown): JiraCredential => ({ kind: kind as 'cloud' | 'server', siteUrl: SITE, email: 'e@example.net', token: 't' })
const jc = (route: (path: string, query: Record<string, unknown> | undefined) => unknown): JiraClient =>
  ({ get: async (path: string, opts?: { query?: Record<string, unknown> }) => route(path, opts?.query) }) as unknown as JiraClient

const wellFormedTail = (s: string, n: number): string => s.slice(-n).replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '\ufffd')

const JIRA_IMPLS: Record<string, Impl> = {
  adfToHtml: (a) => adfToHtml(a[0]),
  adfDeep: (a) => {
    const html = adfToHtml({ type: 'doc', content: [Array.from({ length: a[0] as number }).reduce<unknown>((inner) => ({ type: 'blockquote', content: [inner] }), { type: 'paragraph', content: [{ type: 'text', text: '끝' }] })] })
    return { len: html.length, bq: html.split('<blockquote>').length - 1, text: html.includes('끝') }
  },
  adfBig: (a) => {
    const html = adfToHtml({ type: 'doc', content: Array.from({ length: a[0] as number }, () => ({ type: 'paragraph', content: [{ type: 'text', text: 'a'.repeat(a[1] as number) }] })) })
    return { len: html.length, head: html.slice(0, 20), tail: wellFormedTail(html, 20) }
  },
  adfBigEmoji: (a) => {
    const html = adfToHtml({ type: 'doc', content: [{ type: 'paragraph', content: [{ type: 'text', text: '😀'.repeat(a[0] as number) }] }] })
    return { len: html.length, head: html.slice(0, 20), tail: wellFormedTail(html, 20) }
  },
  avatarOf: (a) => avatarOf(a[0]),
  mapSummary: (a) => mapSummary(a[0]),
  myself: (a) => myself(jc(() => a[1]), cred(a[0])),
  getMyId: (a) => getMyId(jc(() => a[1]), cred(a[0])),
  searchUsers: (a) => searchUsers(jc(() => a[1]), cred(a[0]), 'q'),
  getTransitions: (a) => getTransitions(jc(() => a[1]), cred(a[0]), 'ABC-1'),
  getIssue: (a) => jiraIssue(jc(() => a[3]), cred(a[0]), a[1] as string, a[2] as { start: string[]; end: string[] }),
  getComments: (a) => {
    let n = 0
    return jiraComments(jc(() => (n++ === 0 ? a[1] : (a[2] ?? a[1]))), cred(a[0]), 'ABC-1')
  },
  getDevInfo: (a) =>
    getDevInfo(
      jc((path, q) => (path.endsWith('/summary') ? a[2] : path.endsWith('/detail') ? (q?.['dataType'] === 'pullrequest' ? a[3] : a[4]) : a[1])),
      cred(a[0]),
      'ABC-1'
    ),
  search: (a) => jiraSearch(jc(() => a[1]), cred(a[0]), { jql: 'x', cursor: (a[2] ?? undefined) as string | undefined, page: a[3] as number }),
  pickDateFields: (a) => pickDateFields(a[0]),
  getWorklogs: (a) =>
    getWorklogs(
      jc((path) => {
        if (path.includes('/search')) return a[3]
        const m = /\/issue\/([^/]+)\/worklog$/.exec(path)
        const hit = m ? (a[4] as Record<string, unknown>)[m[1]!] : undefined
        if (hit === undefined) throw new Error('no route ' + path)
        return hit
      }),
      cred(a[0]),
      a[1] as never,
      a[2] as never
    )
}

/** 시간대마다 값이 다르면 'tz' */
function tz<T>(fn: () => T): T | 'tz' {
  const saved = process.env.TZ
  try {
    process.env.TZ = 'UTC'
    const a = JSON.stringify(fn())
    process.env.TZ = 'Asia/Seoul'
    const b = JSON.stringify(fn())
    process.env.TZ = 'America/New_York'
    const d = JSON.stringify(fn())
    return a === b && a === d ? (JSON.parse(a) as T) : 'tz'
  } finally {
    if (saved === undefined) delete process.env.TZ
    else process.env.TZ = saved
  }
}

export async function runCases(cases: MapCase[], impls: Record<string, Impl>): Promise<MapOut[]> {
  const out: MapOut[] = []
  for (const k of cases) {
    const impl = impls[k.fn]
    if (!impl) throw new Error(`구현이 없는 fn: ${k.fn}`)
    try {
      const v = await impl(k.in.map((t) => JSON.parse(t) as unknown))
      out.push({ fn: k.fn, in: k.in, out: v === undefined ? null : JSON.parse(JSON.stringify(v)) })
    } catch {
      out.push({ fn: k.fn, in: k.in, err: true })
    }
  }
  return out
}

function datesOut(): unknown[] {
  return dateStrings().map((s) => ({ in: s, out: tz(() => { const t = Date.parse(s); return Number.isNaN(t) ? 'NaN' : t }) }))
}

function isoOut(): unknown[] {
  return ISO_MS.map((ms) => {
    try {
      return { in: Object.is(ms, -0) ? '-0' : ms, out: new Date(ms).toISOString() }
    } catch {
      return { in: ms, err: true }
    }
  })
}

export async function buildJiraMaps(): Promise<Record<string, unknown>> {
  return { cases: await runCases(jiraMapCases(), JIRA_IMPLS) }
}

export async function buildGithubMaps(): Promise<Record<string, unknown>> {
  return { cases: await runCases(ghMapCases(), IMPLS), dates: datesOut(), iso: isoOut() }
}
