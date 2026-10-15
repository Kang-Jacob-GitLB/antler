// GitHub REST JSON → 공유 타입 변환과 엔드포인트별 조회. 날짜는 모두 epoch seconds.
import type {
  GhCheckRun,
  GhCheckState,
  GhComment,
  GhCommitSummary,
  GhIssueDetail,
  GhIssueSummary,
  GhLabel,
  GhListQuery,
  GhListResult,
  GhPullDetail,
  GhPullFile,
  GhPullSummary,
  GhReview,
  GhReviewState,
  GhUser
} from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
import type { GitHubClient } from './client'

export interface Slug {
  owner: string
  repo: string
}

const PAGE_SIZE = 30
const ENRICH_CONCURRENCY = 6
const FILES_MAX_PAGES = 30
const COMMITS_MAX = 250
const COMMENTS_MAX_PAGES = 10
const REVIEWS_MAX_PAGES = 3

// ── 원본 JSON 모양(쓰는 필드만) ───────────────────────────

interface RawUser {
  login?: string
  avatar_url?: string
}
interface RawLabel {
  name?: string
  color?: string
}
interface RawPull {
  number: number
  title?: string
  state?: string
  draft?: boolean
  merged_at?: string | null
  user?: RawUser | null
  labels?: RawLabel[]
  assignees?: RawUser[]
  requested_reviewers?: RawUser[]
  requested_teams?: unknown[]
  head?: { ref?: string; sha?: string }
  base?: { ref?: string }
  updated_at?: string
  created_at?: string
  comments?: number
  html_url?: string
  body_html?: string | null
  mergeable?: boolean | null
  additions?: number
  deletions?: number
  changed_files?: number
}
interface RawIssue {
  number: number
  title?: string
  state?: string
  draft?: boolean
  user?: RawUser | null
  labels?: (RawLabel | string)[]
  assignees?: RawUser[]
  comments?: number
  updated_at?: string
  created_at?: string
  closed_at?: string | null
  html_url?: string
  body_html?: string | null
  pull_request?: { merged_at?: string | null }
}
interface RawReview {
  user?: RawUser | null
  state?: string
  submitted_at?: string | null
}
interface RawCheckRun {
  name?: string
  status?: string
  conclusion?: string | null
  html_url?: string | null
}
interface RawStatus {
  statuses?: { context?: string; state?: string; target_url?: string | null }[]
}

// ── 기본 변환 ─────────────────────────────────────────

const enc = encodeURIComponent
const repoPath = (r: Slug): string => `/repos/${enc(r.owner)}/${enc(r.repo)}`

export function toEpoch(iso: string | null | undefined): number {
  if (!iso) return 0
  const t = Date.parse(iso)
  return Number.isNaN(t) ? 0 : Math.floor(t / 1000)
}

export function mapUser(u: RawUser | null | undefined): GhUser {
  // 삭제된 계정은 user 가 null 이다
  return { login: u?.login ?? 'ghost', avatarUrl: u?.avatar_url ?? '' }
}

function mapLabel(l: RawLabel | string): GhLabel {
  return typeof l === 'string' ? { name: l, color: '' } : { name: l.name ?? '', color: (l.color ?? '').replace(/^#/, '') }
}

function pullState(p: { state?: string; merged_at?: string | null }): GhPullSummary['state'] {
  if (p.merged_at) return 'merged'
  return p.state === 'closed' ? 'closed' : 'open'
}

/** 목록용 PR 요약. pulls 목록 응답에는 comments 수가 없어 0 이다(상세에서 채운다) */
export function mapPull(p: RawPull): GhPullSummary {
  return {
    number: p.number,
    title: p.title ?? '',
    state: pullState(p),
    draft: p.draft === true,
    author: mapUser(p.user),
    labels: (p.labels ?? []).map(mapLabel),
    assignees: (p.assignees ?? []).map(mapUser),
    headRef: p.head?.ref ?? '',
    baseRef: p.base?.ref ?? '',
    headSha: p.head?.sha ?? '',
    updatedAt: toEpoch(p.updated_at),
    createdAt: toEpoch(p.created_at),
    comments: p.comments ?? 0,
    htmlUrl: p.html_url ?? ''
  }
}

/** 검색 API(issue 모양) 결과를 PR 요약으로 최소 변환. 브랜치·sha 는 알 수 없어 빈 문자열이다 */
export function mapSearchPull(i: RawIssue): GhPullSummary {
  return {
    number: i.number,
    title: i.title ?? '',
    state: pullState({ state: i.state, merged_at: i.pull_request?.merged_at }),
    draft: i.draft === true,
    author: mapUser(i.user),
    labels: (i.labels ?? []).map(mapLabel),
    assignees: (i.assignees ?? []).map(mapUser),
    headRef: '',
    baseRef: '',
    headSha: '',
    updatedAt: toEpoch(i.updated_at),
    createdAt: toEpoch(i.created_at),
    comments: i.comments ?? 0,
    htmlUrl: i.html_url ?? ''
  }
}

export function mapIssue(i: RawIssue): GhIssueSummary {
  return {
    number: i.number,
    title: i.title ?? '',
    state: i.state === 'closed' ? 'closed' : 'open',
    author: mapUser(i.user),
    labels: (i.labels ?? []).map(mapLabel),
    assignees: (i.assignees ?? []).map(mapUser),
    comments: i.comments ?? 0,
    updatedAt: toEpoch(i.updated_at),
    createdAt: toEpoch(i.created_at),
    htmlUrl: i.html_url ?? ''
  }
}

// ── 리뷰·체크 집계 ────────────────────────────────────

/**
 * 리뷰 요약 규칙(GitHub 의 머지 판정과 같은 의미):
 * - 사용자별로 APPROVED / CHANGES_REQUESTED / DISMISSED 중 가장 마지막 것이 그 사람의 판정이다
 *   (DISMISSED 는 판정 취소). 이후의 일반 댓글(COMMENTED)은 승인·변경 요청을 지우지 않는다.
 * - 누군가 CHANGES_REQUESTED → changes_requested, 아니면 APPROVED 가 있으면 approved,
 *   아니면 (임시 저장이 아닌) 리뷰가 하나라도 있으면 commented,
 *   아니면 리뷰 요청이 남아 있으면 pending, 그 외 none.
 */
export function aggregateReviews(reviews: RawReview[], requestedCount: number): GhReviewState {
  const verdict = new Map<string, string>()
  let anyReview = false
  const sorted = [...reviews].sort((a, b) => toEpoch(a.submitted_at) - toEpoch(b.submitted_at))
  for (const r of sorted) {
    const state = r.state?.toUpperCase()
    if (!state || state === 'PENDING') continue
    anyReview = true
    const who = r.user?.login ?? 'ghost'
    if (state === 'APPROVED' || state === 'CHANGES_REQUESTED') verdict.set(who, state)
    else if (state === 'DISMISSED') verdict.delete(who)
  }
  const states = [...verdict.values()]
  if (states.includes('CHANGES_REQUESTED')) return 'changes_requested'
  if (states.includes('APPROVED')) return 'approved'
  if (anyReview) return 'commented'
  return requestedCount > 0 ? 'pending' : 'none'
}

function mapReviewState(state: string | undefined): GhReviewState {
  switch (state?.toUpperCase()) {
    case 'APPROVED':
      return 'approved'
    case 'CHANGES_REQUESTED':
      return 'changes_requested'
    case 'COMMENTED':
      return 'commented'
    case 'PENDING':
      return 'pending'
    default:
      // DISMISSED 등
      return 'none'
  }
}

/**
 * 체크 한 건의 상태. 완료되지 않았으면 pending, 완료된 것은 결론으로:
 * success → success · neutral/skipped/stale → neutral · action_required → pending(사람 조치 대기) ·
 * 그 외(failure·cancelled·timed_out·startup_failure) → failure
 */
export function checkRunState(run: RawCheckRun): GhCheckState {
  if (run.status !== 'completed') return 'pending'
  switch (run.conclusion) {
    case 'success':
      return 'success'
    case 'neutral':
    case 'skipped':
    case 'stale':
      return 'neutral'
    case 'action_required':
      return 'pending'
    default:
      return 'failure'
  }
}

/** 커밋 상태(legacy status) 한 건: success / pending / failure·error */
export function commitStatusState(state: string | undefined): GhCheckState {
  if (state === 'success') return 'success'
  if (state === 'pending') return 'pending'
  return 'failure'
}

/**
 * 체크 집계: 하나라도 failure 면 failure, 아니면 pending 이 있으면 pending,
 * 하나도 없으면 none, 전부 neutral 이면 neutral, 그 외(success 와 neutral 만) success.
 */
export function aggregateChecks(states: GhCheckState[]): GhCheckState {
  if (states.length === 0) return 'none'
  if (states.includes('failure')) return 'failure'
  if (states.includes('pending')) return 'pending'
  return states.every((s) => s === 'neutral') ? 'neutral' : 'success'
}

// ── 조회 ──────────────────────────────────────────────

/** 동시 실행 수를 제한해 순서대로 결과를 모은다 */
async function mapLimit<T, R>(items: T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const out = new Array<R>(items.length)
  let next = 0
  const worker = async (): Promise<void> => {
    while (next < items.length) {
      const i = next++
      out[i] = await fn(items[i]!)
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker))
  return out
}

/** 보강 실패(권한·한도 등)는 목록을 막지 않는다. 취소만 그대로 전파한다 */
async function soft<T>(p: Promise<T>): Promise<T | undefined> {
  try {
    return await p
  } catch (err) {
    if (err instanceof AbortError) throw err
    return undefined
  }
}

async function fetchReviews(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<RawReview[]> {
  const all: RawReview[] = []
  for (let page = 1; page <= REVIEWS_MAX_PAGES; page++) {
    const res = await c.get<RawReview[]>(`${repoPath(r)}/pulls/${n}/reviews`, { query: { per_page: 100, page }, ...(signal ? { signal } : {}) })
    all.push(...res.data)
    if (!res.hasMore) break
  }
  return all
}

/** check-runs 와 legacy status 를 합친 개별 체크 목록 */
async function fetchChecks(c: GitHubClient, r: Slug, sha: string, signal?: AbortSignal): Promise<GhCheckRun[]> {
  const o = signal ? { signal } : {}
  const [runs, status] = await Promise.all([
    c.get<{ check_runs?: RawCheckRun[] }>(`${repoPath(r)}/commits/${enc(sha)}/check-runs`, { query: { per_page: 100 }, ...o }),
    c.get<RawStatus>(`${repoPath(r)}/commits/${enc(sha)}/status`, { query: { per_page: 100 }, ...o })
  ])
  const out: GhCheckRun[] = []
  for (const run of runs.data.check_runs ?? []) {
    out.push({ name: run.name ?? '', state: checkRunState(run), ...(run.html_url ? { url: run.html_url } : {}) })
  }
  for (const s of status.data.statuses ?? []) {
    out.push({ name: s.context ?? '', state: commitStatusState(s.state), ...(s.target_url ? { url: s.target_url } : {}) })
  }
  return out
}

/**
 * PR 목록. 라벨·담당자 필터가 없으면 pulls 엔드포인트(updated 내림차순, 브랜치·sha 포함)를 쓴다.
 * pulls 엔드포인트에는 라벨 필터가 없으므로 필터가 있으면 issues 엔드포인트(labels·assignee 파라미터)에서
 * pull_request 항목만 고른다. 검색 API 의 쿼리 언어를 쓰지 않으므로 라벨 이름에 든 따옴표 등으로
 * 다른 한정자(repo:… 등)를 끼워 넣는 인젝션이 원천적으로 없다. 결과가 issue 모양이라 브랜치·sha 가 없어
 * 리뷰·체크 보강은 하지 않는다(요약만). 필터된 페이지는 PR 이 아닌 항목을 빼서 짧을 수 있다(hasMore 는 Link 기준).
 * 로그인했다면 pulls 결과에만 리뷰·체크 요약을 동시 6개씩 보강한다(ETag 캐시로 재조회는 304).
 */
export async function listPulls(c: GitHubClient, r: Slug, q: GhListQuery, signal?: AbortSignal): Promise<GhListResult<GhPullSummary>> {
  const page = q.page ?? 1
  const o = signal ? { signal } : {}
  if (q.label || q.assignee) {
    const res = await c.get<RawIssue[]>(`${repoPath(r)}/issues`, {
      query: {
        state: q.state,
        ...(q.label ? { labels: q.label } : {}),
        ...(q.assignee ? { assignee: q.assignee } : {}),
        per_page: PAGE_SIZE,
        page,
        sort: 'updated',
        direction: 'desc'
      },
      ...o
    })
    return { items: res.data.filter((i) => i.pull_request).map(mapSearchPull), hasMore: res.hasMore }
  }

  const res = await c.get<RawPull[]>(`${repoPath(r)}/pulls`, {
    query: { state: q.state, per_page: PAGE_SIZE, page, sort: 'updated', direction: 'desc' },
    ...o
  })
  const items = res.data.map(mapPull)
  if (c.hasToken()) {
    await mapLimit(res.data.map((raw, i) => ({ raw, i })), ENRICH_CONCURRENCY, async ({ raw, i }) => {
      const item = items[i]!
      const [reviews, checks] = await Promise.all([soft(fetchReviews(c, r, raw.number, signal)), item.headSha ? soft(fetchChecks(c, r, item.headSha, signal)) : undefined])
      if (reviews) item.review = aggregateReviews(reviews, (raw.requested_reviewers?.length ?? 0) + (raw.requested_teams?.length ?? 0))
      if (checks) item.checks = aggregateChecks(checks.map((x) => x.state))
    })
  }
  return { items, hasMore: res.hasMore }
}

export async function getPull(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<GhPullDetail> {
  const o = signal ? { signal } : {}
  const pull = await c.get<RawPull>(`${repoPath(r)}/pulls/${n}`, { html: true, ...o })
  const raw = pull.data
  const summary = mapPull(raw)
  const [reviews, checkRuns] = await Promise.all([fetchReviews(c, r, n, signal), summary.headSha ? soft(fetchChecks(c, r, summary.headSha, signal)) : undefined])
  const requested = raw.requested_reviewers ?? []
  const detailReviews: GhReview[] = reviews
    .filter((x) => x.submitted_at && x.state?.toUpperCase() !== 'PENDING')
    .map((x) => ({ author: mapUser(x.user), state: mapReviewState(x.state), submittedAt: toEpoch(x.submitted_at) }))
  return {
    ...summary,
    review: aggregateReviews(reviews, requested.length + (raw.requested_teams?.length ?? 0)),
    ...(checkRuns ? { checks: aggregateChecks(checkRuns.map((x) => x.state)) } : {}),
    bodyHtml: raw.body_html ?? '',
    mergeable: raw.mergeable ?? null,
    additions: raw.additions ?? 0,
    deletions: raw.deletions ?? 0,
    changedFiles: raw.changed_files ?? 0,
    reviews: detailReviews,
    checkRuns: checkRuns ?? [],
    requestedReviewers: requested.map(mapUser)
  }
}

interface RawFile {
  filename: string
  previous_filename?: string
  status?: string
  additions?: number
  deletions?: number
  patch?: string
}

const FILE_STATUSES: readonly GhPullFile['status'][] = ['added', 'removed', 'modified', 'renamed', 'copied', 'changed', 'unchanged']

/** 변경 파일(최대 30쪽 × 100 = 3000개, GitHub 가 주는 상한과 같다) */
export async function getPullFiles(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<GhPullFile[]> {
  const out: GhPullFile[] = []
  for (let page = 1; page <= FILES_MAX_PAGES; page++) {
    const res = await c.get<RawFile[]>(`${repoPath(r)}/pulls/${n}/files`, { query: { per_page: 100, page }, ...(signal ? { signal } : {}) })
    for (const f of res.data) {
      const status = FILE_STATUSES.find((s) => s === f.status) ?? 'changed'
      out.push({
        path: f.filename,
        ...(f.previous_filename ? { oldPath: f.previous_filename } : {}),
        status,
        additions: f.additions ?? 0,
        deletions: f.deletions ?? 0,
        ...(f.patch !== undefined ? { patch: f.patch } : {})
      })
    }
    if (!res.hasMore) break
  }
  return out
}

interface RawCommit {
  sha: string
  commit?: { message?: string; author?: { name?: string; date?: string }; committer?: { date?: string } }
  author?: RawUser | null
}

/** PR 커밋(최대 250개 — GitHub 가 이 API 로 주는 상한) */
export async function getPullCommits(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<GhCommitSummary[]> {
  const out: GhCommitSummary[] = []
  for (let page = 1; out.length < COMMITS_MAX; page++) {
    const res = await c.get<RawCommit[]>(`${repoPath(r)}/pulls/${n}/commits`, { query: { per_page: 100, page }, ...(signal ? { signal } : {}) })
    for (const k of res.data) {
      out.push({
        sha: k.sha,
        message: k.commit?.message ?? '',
        authorName: k.commit?.author?.name ?? k.author?.login ?? '',
        ...(k.author ? { author: mapUser(k.author) } : {}),
        date: toEpoch(k.commit?.author?.date ?? k.commit?.committer?.date)
      })
    }
    if (!res.hasMore) break
  }
  return out.slice(0, COMMITS_MAX)
}

/**
 * 이슈 목록. issues 엔드포인트는 PR 도 섞어 돌려주므로 pull_request 가 있는 항목은 버린다.
 * 그래서 한 쪽이 30개보다 짧을 수 있으니 hasMore 는 항목 수가 아니라 Link 헤더로 판단한다.
 */
export async function listIssues(c: GitHubClient, r: Slug, q: GhListQuery, signal?: AbortSignal): Promise<GhListResult<GhIssueSummary>> {
  const res = await c.get<RawIssue[]>(`${repoPath(r)}/issues`, {
    query: { state: q.state, labels: q.label, assignee: q.assignee, per_page: PAGE_SIZE, page: q.page ?? 1, sort: 'updated', direction: 'desc' },
    ...(signal ? { signal } : {})
  })
  return { items: res.data.filter((i) => !i.pull_request).map(mapIssue), hasMore: res.hasMore }
}

export async function getIssue(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<GhIssueDetail> {
  const res = await c.get<RawIssue>(`${repoPath(r)}/issues/${n}`, { html: true, ...(signal ? { signal } : {}) })
  const closedAt = toEpoch(res.data.closed_at)
  return { ...mapIssue(res.data), bodyHtml: res.data.body_html ?? '', ...(closedAt ? { closedAt } : {}) }
}

interface RawComment {
  user?: RawUser | null
  body_html?: string | null
  created_at?: string
  html_url?: string
}

/** PR·이슈 공용 댓글(이슈 댓글 API). 최대 1000개 */
export async function getComments(c: GitHubClient, r: Slug, n: number, signal?: AbortSignal): Promise<GhComment[]> {
  const out: GhComment[] = []
  for (let page = 1; page <= COMMENTS_MAX_PAGES; page++) {
    const res = await c.get<RawComment[]>(`${repoPath(r)}/issues/${n}/comments`, { query: { per_page: 100, page }, html: true, ...(signal ? { signal } : {}) })
    for (const k of res.data) {
      out.push({ author: mapUser(k.user), bodyHtml: k.body_html ?? '', createdAt: toEpoch(k.created_at), htmlUrl: k.html_url ?? '' })
    }
    if (!res.hasMore) break
  }
  return out
}

/** 필터용 라벨·담당 가능 사용자(각 최대 100개) */
export async function getFilters(c: GitHubClient, r: Slug, signal?: AbortSignal): Promise<{ labels: GhLabel[]; assignees: GhUser[] }> {
  const o = signal ? { signal } : {}
  const [labels, assignees] = await Promise.all([
    c.get<RawLabel[]>(`${repoPath(r)}/labels`, { query: { per_page: 100 }, ...o }),
    c.get<RawUser[]>(`${repoPath(r)}/assignees`, { query: { per_page: 100 }, ...o })
  ])
  return { labels: labels.data.map(mapLabel), assignees: assignees.data.map(mapUser) }
}
