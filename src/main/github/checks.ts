// GitHub Actions(체크) 상태 조회: 커밋 목록용 일괄 요약(GraphQL), 커밋 상세의 체크 목록, 워크플로 실행·잡 목록. 모두 읽기 전용이다.
import type { GhCheckDetail, GhCheckState, GhJob, GhListResult, GhRun } from '@shared/types'
import { ttlOf } from '@shared/checkTtl'
import { checkRunState, commitStatusState, repoPath, toEpoch, type Slug } from './api'
import type { GitHubClient } from './client'

export { TTL_DONE_MS, TTL_NONE_MS, TTL_PENDING_MS } from '@shared/checkTtl'

/** 한 번의 GraphQL 호출에 싣는 커밋 수 */
export const CHECKS_BATCH = 50
const RUNS_PER_PAGE = 30
const JOBS_PER_PAGE = 100
const CACHE_MAX = 5000
const SHA_RE = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/

/** (저장소, sha) → 체크 상태 캐시. 상태에 따라 수명이 다르다 */
export class CheckStateCache {
  private readonly map = new Map<string, { state: GhCheckState; at: number }>()

  constructor(private readonly now: () => number = Date.now) {}

  get(key: string): GhCheckState | undefined {
    const e = this.map.get(key)
    if (!e) return undefined
    if (this.now() - e.at >= ttlOf(e.state)) {
      this.map.delete(key)
      return undefined
    }
    return e.state
  }

  set(key: string, state: GhCheckState): void {
    // 이미 있는 키를 갱신할 때는 다른 항목을 밀어내지 않는다(맨 뒤로 옮기기만)
    if (!this.map.delete(key) && this.map.size >= CACHE_MAX) this.map.delete(this.map.keys().next().value as string)
    this.map.set(key, { state, at: this.now() })
  }

  clear(): void {
    this.map.clear()
  }
}

/** GraphQL StatusState → 공유 상태 */
export function rollupState(state: string | undefined | null): GhCheckState {
  switch (state) {
    case 'SUCCESS':
      return 'success'
    case 'FAILURE':
    case 'ERROR':
      return 'failure'
    case 'PENDING':
    case 'EXPECTED':
      return 'pending'
    default:
      return 'none'
  }
}

type RollupData = { repository?: Record<string, { statusCheckRollup?: { state?: string } | null } | null> | null }

/** 별칭(c0…)별로 object(oid:) 를 묻는 쿼리. sha 는 변수로 넘겨 쿼리 문자열에 끼어들지 않는다 */
export function rollupQuery(n: number): string {
  const vars = Array.from({ length: n }, (_, i) => `$o${i}:GitObjectID!`).join(',')
  const fields = Array.from({ length: n }, (_, i) => `c${i}:object(oid:$o${i}){...on Commit{statusCheckRollup{state}}}`).join(' ')
  return `query($owner:String!,$name:String!,${vars}){repository(owner:$owner,name:$name){${fields}}}`
}

/**
 * 여러 커밋의 체크 요약을 한 번에 구한다. 캐시에 있으면 묻지 않고, 나머지는 CHECKS_BATCH 개씩 GraphQL 한 번으로 묻는다.
 * GitHub 가 모르는 커밋(아직 push 안 됨)·체크가 없는 커밋은 'none'. 저장소에 접근할 수 없으면 그 sha 들은 결과에서 빠진다.
 */
export async function fetchCommitChecks(
  c: GitHubClient,
  r: Slug,
  shas: string[],
  cache: CheckStateCache,
  signal?: AbortSignal
): Promise<Record<string, GhCheckState>> {
  const out: Record<string, GhCheckState> = {}
  const scope = `${r.owner}/${r.repo}`.toLowerCase()
  const missing: string[] = []
  for (const raw of new Set(shas.map((s) => s.toLowerCase()))) {
    if (!SHA_RE.test(raw)) continue
    const hit = cache.get(`${scope}@${raw}`)
    if (hit) out[raw] = hit
    else missing.push(raw)
  }
  for (let i = 0; i < missing.length; i += CHECKS_BATCH) {
    const chunk = missing.slice(i, i + CHECKS_BATCH)
    const variables: Record<string, string> = { owner: r.owner, name: r.repo }
    chunk.forEach((sha, k) => (variables[`o${k}`] = sha))
    const data = await c.graphql<RollupData>(rollupQuery(chunk.length), variables, signal ? { signal } : {})
    const repo = data.repository
    if (!repo) continue
    chunk.forEach((sha, k) => {
      const state = rollupState(repo[`c${k}`]?.statusCheckRollup?.state)
      cache.set(`${scope}@${sha}`, state)
      out[sha] = state
    })
  }
  return out
}

// ── 커밋 상세 ─────────────────────────────────────────

interface RawRun {
  name?: string
  status?: string
  conclusion?: string | null
  html_url?: string | null
  started_at?: string | null
  completed_at?: string | null
}
interface RawStatus {
  statuses?: { context?: string; state?: string; target_url?: string | null }[]
}

function durationOf(start: string | null | undefined, end: string | null | undefined): number | undefined {
  const a = toEpoch(start)
  const b = toEpoch(end)
  return a && b && b >= a ? b - a : undefined
}

/** 한 커밋의 체크 목록: check-run(소요 시간·결론 포함) + legacy status */
export async function fetchCheckDetails(c: GitHubClient, r: Slug, sha: string, signal?: AbortSignal): Promise<GhCheckDetail[]> {
  if (!SHA_RE.test(sha.toLowerCase())) throw new Error('잘못된 커밋 해시입니다')
  const o = signal ? { signal } : {}
  const base = `${repoPath(r)}/commits/${encodeURIComponent(sha.toLowerCase())}`
  const [runs, status] = await Promise.all([
    c.get<{ check_runs?: RawRun[] }>(`${base}/check-runs`, { query: { per_page: 100 }, ...o }),
    c.get<RawStatus>(`${base}/status`, { query: { per_page: 100 }, ...o })
  ])
  const out: GhCheckDetail[] = []
  for (const run of runs.data.check_runs ?? []) {
    const d = durationOf(run.started_at, run.completed_at)
    out.push({
      name: run.name ?? '',
      state: checkRunState(run),
      ...(run.conclusion ? { conclusion: run.conclusion } : {}),
      ...(d !== undefined ? { durationSec: d } : {}),
      ...(run.html_url ? { url: run.html_url } : {})
    })
  }
  for (const s of status.data.statuses ?? []) {
    out.push({ name: s.context ?? '', state: commitStatusState(s.state), ...(s.target_url ? { url: s.target_url } : {}) })
  }
  return out
}

// ── 워크플로 실행 ──────────────────────────────────────

interface RawWorkflowRun {
  id: number
  name?: string | null
  display_title?: string
  head_branch?: string | null
  head_sha?: string
  event?: string
  status?: string | null
  conclusion?: string | null
  run_number?: number
  created_at?: string
  updated_at?: string
  html_url?: string
}
interface RawJob {
  id: number
  name?: string
  status?: string
  conclusion?: string | null
  started_at?: string | null
  completed_at?: string | null
  html_url?: string | null
}

export function mapRun(x: RawWorkflowRun): GhRun {
  return {
    id: x.id,
    name: x.name ?? '',
    title: x.display_title ?? '',
    branch: x.head_branch ?? '',
    event: x.event ?? '',
    state: checkRunState({ ...(x.status ? { status: x.status } : {}), conclusion: x.conclusion ?? null }),
    status: x.status ?? '',
    ...(x.conclusion ? { conclusion: x.conclusion } : {}),
    number: x.run_number ?? 0,
    sha: x.head_sha ?? '',
    createdAt: toEpoch(x.created_at),
    updatedAt: toEpoch(x.updated_at),
    htmlUrl: x.html_url ?? ''
  }
}

export async function listRuns(c: GitHubClient, r: Slug, page: number, signal?: AbortSignal): Promise<GhListResult<GhRun>> {
  const res = await c.get<{ workflow_runs?: RawWorkflowRun[] }>(`${repoPath(r)}/actions/runs`, {
    query: { per_page: RUNS_PER_PAGE, page },
    ...(signal ? { signal } : {})
  })
  return { items: (res.data.workflow_runs ?? []).map(mapRun), hasMore: res.hasMore }
}

export async function listJobs(c: GitHubClient, r: Slug, runId: number, signal?: AbortSignal): Promise<GhJob[]> {
  const res = await c.get<{ jobs?: RawJob[] }>(`${repoPath(r)}/actions/runs/${runId}/jobs`, {
    query: { per_page: JOBS_PER_PAGE },
    ...(signal ? { signal } : {})
  })
  return (res.data.jobs ?? []).map((j) => {
    const d = durationOf(j.started_at, j.completed_at)
    return {
      id: j.id,
      name: j.name ?? '',
      state: checkRunState({ ...(j.status ? { status: j.status } : {}), conclusion: j.conclusion ?? null }),
      status: j.status ?? '',
      ...(j.conclusion ? { conclusion: j.conclusion } : {}),
      ...(d !== undefined ? { durationSec: d } : {}),
      htmlUrl: j.html_url ?? ''
    }
  })
}
