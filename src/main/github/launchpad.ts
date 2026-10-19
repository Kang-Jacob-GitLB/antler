// Launchpad: 내 "PR · 리뷰 요청 · 할당 이슈" 를 GitHub 검색 API 로 모은다.
// 검색은 저장소를 한정하지 않고 사용자 기준으로 섹션마다 한 번씩만 보낸다(`repo:` 묶음을 쓰면 접근할 수 없는 저장소 하나가
// 검색 전체를 422 로 만들고, 저장소 수만큼 검색어가 늘어난다). 결과는 main 에서 앱에 등록된 저장소의 GitHub remote 와 맞춰 거른다.
// 그래서 범위(전체·그룹·저장소)를 바꿔도 다시 검색할 필요가 없다. 검색 API 는 core 와 별도 한도(인증 시 분당 30회)다.
import type { GhLaunchpadItem, GhLaunchpadResult, GhLaunchpadSection } from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
import { mapIssue, mapSearchPull, type RawIssue, type Slug } from './api'
import { GitHubApiError, type GitHubClient } from './client'

const PER_PAGE = 100
/** 섹션 하나에서 읽는 쪽 수 상한(검색 API 가 한 검색에 주는 최대 1000건의 절반) */
export const MAX_PAGES_PER_SECTION = 5
/** GitHub 검색이 한 검색어에 돌려줄 수 있는 최대 건수 */
const SEARCH_RESULT_CAP = 1000
/** 저장소 하나의 remote 를 읽는 제한 시간. 꺼진 SSH 호스트가 전체를 붙잡지 않게 한다 */
export const RESOLVE_TIMEOUT_MS = 3_000
const RESOLVE_CONCURRENCY = 8

interface SectionDef {
  section: GhLaunchpadSection
  kind: 'pull' | 'issue'
  /** 검색어. 열린 항목만, 보관된 저장소는 제외 */
  q: string
}

export const SECTIONS: readonly SectionDef[] = [
  { section: 'authored', kind: 'pull', q: 'is:pr is:open author:@me archived:false' },
  { section: 'review', kind: 'pull', q: 'is:pr is:open review-requested:@me archived:false' },
  { section: 'issues', kind: 'issue', q: 'is:issue is:open assignee:@me archived:false' }
]

const slugKey = (s: Slug): string => `${s.owner}/${s.repo}`.toLowerCase()

/** 검색 결과 항목의 repository_url 에서 owner/repo 를 뽑는다. 형식이 다르거나 해석할 수 없으면 null */
export function slugFromRepositoryUrl(url: string | undefined): Slug | null {
  const m = /^https:\/\/api\.github\.com\/repos\/([^/]+)\/([^/]+)$/.exec(url ?? '')
  if (!m) return null
  try {
    return { owner: decodeURIComponent(m[1]!), repo: decodeURIComponent(m[2]!) }
  } catch {
    return null
  }
}

interface SearchResponse {
  items?: RawIssue[]
  incomplete_results?: boolean
  total_count?: number
}

async function mapLimit<T>(items: T[], limit: number, fn: (item: T) => Promise<void>): Promise<void> {
  let next = 0
  const worker = async (): Promise<void> => {
    while (next < items.length) await fn(items[next++]!)
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker))
}

/** 저장소 ID → 그 저장소의 모든 GitHub remote. signal 이 끊기면 읽기를 멈춘다 */
export type RemoteResolver = (repoId: string, signal: AbortSignal) => Promise<Slug[]>

interface Resolved {
  /** 소문자 owner/repo → 그 저장소를 remote 로 가진 앱 저장소 ID(등록 순서) */
  byKey: Map<string, string[]>
  /** GitHub remote 가 없는 저장소 수 */
  skipped: number
  /** 시간 초과·오류로 remote 를 읽지 못한 저장소 수 */
  unresolved: number
}

/** 저장소별 제한 시간 안에 병렬로 remote 를 읽는다. 실패한 저장소는 건너뛴다(취소는 그대로 던진다) */
export async function resolveRemotes(repoIds: string[], resolve: RemoteResolver, signal: AbortSignal): Promise<Resolved> {
  const slugs: (Slug[] | null)[] = repoIds.map(() => null)
  await mapLimit(
    repoIds.map((id, i) => ({ id, i })),
    RESOLVE_CONCURRENCY,
    async ({ id, i }) => {
      if (signal.aborted) throw new AbortError()
      const sig = AbortSignal.any([signal, AbortSignal.timeout(RESOLVE_TIMEOUT_MS)])
      const gone = new Promise<never>((_, reject) => {
        const stop = (): void => reject(signal.aborted ? new AbortError() : new Error('timeout'))
        if (sig.aborted) stop()
        else sig.addEventListener('abort', stop, { once: true })
      })
      gone.catch(() => {})
      const work = resolve(id, sig)
      work.catch(() => {})
      try {
        slugs[i] = await Promise.race([work, gone])
      } catch (err) {
        if (err instanceof AbortError || signal.aborted) throw new AbortError()
      }
    }
  )
  const byKey = new Map<string, string[]>()
  let skipped = 0
  let unresolved = 0
  slugs.forEach((list, i) => {
    if (list === null) return void unresolved++
    if (list.length === 0) return void skipped++
    for (const s of list) {
      const k = slugKey(s)
      byKey.set(k, [...(byKey.get(k) ?? []), repoIds[i]!])
    }
  })
  return { byKey, skipped, unresolved }
}

interface SectionResult {
  def: SectionDef
  items: RawIssue[]
  incomplete: boolean
}

/** 섹션 하나의 검색. 실패는 errors 에 담고(로그인 만료·취소는 던진다) 읽은 데까지를 돌려준다 */
async function searchSection(client: GitHubClient, def: SectionDef, errors: string[], signal: AbortSignal): Promise<SectionResult> {
  const out: SectionResult = { def, items: [], incomplete: false }
  for (let page = 1; page <= MAX_PAGES_PER_SECTION; page++) {
    let res
    try {
      res = await client.get<SearchResponse>('/search/issues', { query: { q: def.q, sort: 'updated', order: 'desc', per_page: PER_PAGE, page }, signal })
    } catch (err) {
      if (err instanceof AbortError) throw err
      if (err instanceof GitHubApiError && err.code !== 'GH_AUTH') {
        if (!errors.includes(err.message)) errors.push(err.message)
        out.incomplete = true
        return out
      }
      throw err
    }
    out.items.push(...(res.data.items ?? []))
    if (res.data.incomplete_results || (res.data.total_count ?? 0) > SEARCH_RESULT_CAP) out.incomplete = true
    if (!res.hasMore) return out
    if (page === MAX_PAGES_PER_SECTION) out.incomplete = true
  }
  return out
}

/**
 * Launchpad 조회. 섹션 검색 3개와 저장소 remote 읽기를 동시에 시작하고, 둘이 끝나면 등록된 저장소에 속한 항목만 남긴다.
 * 일부 검색이 실패해도(한도 초과 등) 나머지는 돌려주고 이유를 errors 에 담는다. 로그인 만료(GH_AUTH)와 취소는 그대로 던진다.
 */
export async function fetchLaunchpad(client: GitHubClient, repoIds: string[], resolve: RemoteResolver, signal: AbortSignal): Promise<GhLaunchpadResult> {
  const errors: string[] = []
  const [sections, resolved] = await Promise.all([
    Promise.all(SECTIONS.map((def) => searchSection(client, def, errors, signal))),
    resolveRemotes(repoIds, resolve, signal)
  ])
  const items = new Map<string, GhLaunchpadItem>()
  let incomplete = false
  for (const { def, items: raws, incomplete: inc } of sections) {
    incomplete ||= inc
    for (const raw of raws) {
      const slug = slugFromRepositoryUrl(raw.repository_url)
      const repoIds = slug ? resolved.byKey.get(slugKey(slug)) : undefined
      if (!slug || !repoIds) continue
      const key = `${def.section}:${slugKey(slug)}#${raw.number}`
      if (!items.has(key)) items.set(key, toItem(def, raw, repoIds, slug))
    }
  }
  return {
    items: [...items.values()].sort((a, b) => b.updatedAt - a.updatedAt),
    fetchedAt: Date.now(),
    needsLogin: false,
    incomplete,
    skipped: resolved.skipped,
    unresolved: resolved.unresolved,
    errors
  }
}

function toItem(def: SectionDef, raw: RawIssue, repoIds: string[], slug: Slug): GhLaunchpadItem {
  const base = { section: def.section, kind: def.kind, repoIds, owner: slug.owner, repo: slug.repo } as const
  if (def.kind === 'pull') {
    const p = mapSearchPull(raw)
    return { ...base, number: p.number, title: p.title, state: p.state, draft: p.draft, author: p.author, labels: p.labels, comments: p.comments, updatedAt: p.updatedAt, createdAt: p.createdAt, htmlUrl: p.htmlUrl }
  }
  const i = mapIssue(raw)
  return { ...base, number: i.number, title: i.title, state: i.state, draft: false, author: i.author, labels: i.labels, comments: i.comments, updatedAt: i.updatedAt, createdAt: i.createdAt, htmlUrl: i.htmlUrl }
}

/** 로그인하지 않은 상태의 결과(@me 검색은 로그인이 있어야 한다) */
export function loginRequired(): GhLaunchpadResult {
  return { items: [], fetchedAt: Date.now(), needsLogin: true, incomplete: false, skipped: 0, unresolved: 0, errors: [] }
}
