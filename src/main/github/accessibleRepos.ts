// 로그인한 사용자가 접근할 수 있는 GitHub 저장소(본인·협업·소속 org) 목록. 저장소 추가 창의 원격 탭이 쓴다.
// GET /user/repos 를 쪽별로 읽는다. 쪽 수 상한을 넘으면 거기까지만 돌려주고 truncated 로 알린다.
import type { GhAccessibleRepo, GhAccessibleReposResult } from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
import type { GitHubClient } from './client'

const PER_PAGE = 100
/** 읽는 쪽 수 상한(저장소 2000개). 넘으면 truncated */
export const MAX_ACCESSIBLE_PAGES = 20

interface RawRepo {
  name?: unknown
  full_name?: unknown
  private?: unknown
  fork?: unknown
  archived?: unknown
  default_branch?: unknown
  description?: unknown
  pushed_at?: unknown
  html_url?: unknown
  owner?: { login?: unknown; type?: unknown } | null
}

const str = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null)

/** 응답 항목 하나를 매핑한다. 필수 필드가 없거나 형식이 이상하면 null(건너뛴다) */
export function mapAccessibleRepo(raw: unknown): GhAccessibleRepo | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as RawRepo
  const owner = str(r.owner?.login)
  const repo = str(r.name)
  const htmlUrl = str(r.html_url)
  if (!owner || !repo || !htmlUrl) return null
  const pushed = typeof r.pushed_at === 'string' ? Date.parse(r.pushed_at) : NaN
  return {
    owner,
    repo,
    fullName: str(r.full_name) ?? `${owner}/${repo}`,
    ownerType: r.owner?.type === 'Organization' ? 'Organization' : 'User',
    private: r.private === true,
    fork: r.fork === true,
    archived: r.archived === true,
    defaultBranch: str(r.default_branch) ?? 'main',
    description: str(r.description),
    pushedAt: Number.isFinite(pushed) ? pushed : 0,
    htmlUrl
  }
}

/** 접근 가능한 저장소를 모두(상한까지) 읽는다. pushedAt 내림차순. 취소는 AbortError 로 던진다 */
export async function fetchAccessibleRepos(client: GitHubClient, signal: AbortSignal): Promise<GhAccessibleReposResult> {
  const repos: GhAccessibleRepo[] = []
  let truncated = false
  for (let page = 1; ; page++) {
    if (signal.aborted) throw new AbortError()
    const { data, hasMore } = await client.get<unknown>('/user/repos', {
      query: { affiliation: 'owner,collaborator,organization_member', sort: 'pushed', per_page: PER_PAGE, page },
      signal
    })
    if (Array.isArray(data)) {
      for (const item of data) {
        const mapped = mapAccessibleRepo(item)
        if (mapped) repos.push(mapped)
      }
    }
    if (!hasMore) break
    if (page >= MAX_ACCESSIBLE_PAGES) {
      truncated = true
      break
    }
  }
  repos.sort((a, b) => b.pushedAt - a.pushedAt)
  return { repos, needsLogin: false, truncated, fetchedAt: Date.now() }
}

export function accessibleLoginRequired(): GhAccessibleReposResult {
  return { repos: [], needsLogin: true, truncated: false, fetchedAt: Date.now() }
}
