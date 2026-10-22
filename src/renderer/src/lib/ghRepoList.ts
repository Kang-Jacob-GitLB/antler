// 저장소 추가 창의 GitHub 탭이 쓰는 순수 함수: 목록 거르기·소유자 뽑기·이미 추가됨 판정·상대 시각·첫 fetch 상태.
// 스토어·DOM 을 import 하지 않아 단위 테스트가 가능하다.
import type { FetchStatus, GhAccessibleRepo, Locale, Repo } from '@shared/types'

/** 소유자 거르기: 전체 / 개인 계정(User 소유) / 조직 하나 */
export type OwnerFilter = { kind: 'all' } | { kind: 'user' } | { kind: 'org'; owner: string }

/** select 의 value 로 쓰는 문자열 표기 */
export function ownerFilterValue(f: OwnerFilter): string {
  return f.kind === 'org' ? `org:${f.owner}` : f.kind
}

export function parseOwnerFilter(value: string): OwnerFilter {
  if (value.startsWith('org:')) return { kind: 'org', owner: value.slice(4) }
  return value === 'user' ? { kind: 'user' } : { kind: 'all' }
}

/** 목록에 나오는 조직(owner 이름순, 대소문자 무시)과 개인 계정 저장소가 있는지 */
export function listOwners(repos: readonly GhAccessibleRepo[]): { orgs: string[]; hasUser: boolean } {
  const orgs = new Map<string, string>()
  let hasUser = false
  for (const r of repos) {
    if (r.ownerType === 'Organization') orgs.set(r.owner.toLowerCase(), r.owner)
    else hasUser = true
  }
  return { orgs: [...orgs.values()].sort((a, b) => a.localeCompare(b)), hasUser }
}

/** 이름·owner·설명에 질의(공백으로 나눈 낱말 모두, 대소문자 무시)가 들어 있는 저장소를 소유자 조건과 함께 거른다. 순서는 그대로 */
export function filterGhRepos(repos: readonly GhAccessibleRepo[], query: string, owner: OwnerFilter): GhAccessibleRepo[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  return repos.filter((r) => {
    if (owner.kind === 'user' && r.ownerType !== 'User') return false
    if (owner.kind === 'org' && (r.ownerType !== 'Organization' || r.owner.toLowerCase() !== owner.owner.toLowerCase())) return false
    if (words.length === 0) return true
    const hay = `${r.owner}/${r.repo} ${r.description ?? ''}`.toLowerCase()
    return words.every((w) => hay.includes(w))
  })
}

/** 대소문자를 가리지 않는 owner/repo 키 */
export function ghKey(owner: string, repo: string): string {
  return `${owner}/${repo}`.toLowerCase()
}

/** 이미 목록에 있는 원격 전용 저장소의 키 집합 */
export function registeredGhKeys(repos: readonly Repo[]): Set<string> {
  const set = new Set<string>()
  for (const r of repos) if (r.github) set.add(ghKey(r.github.owner, r.github.repo))
  return set
}

const rtfCache = new Map<Locale, Intl.RelativeTimeFormat>()

/** 마지막 push 시각의 상대 표기(초~년). formatDate 는 3시간이 넘으면 날짜로 바뀌지만 여기서는 오래된 저장소도 "2년 전" 처럼 짧게 보인다 */
export function formatPushed(epochMs: number, locale: Locale, now = Date.now()): string {
  let f = rtfCache.get(locale)
  if (!f) rtfCache.set(locale, (f = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' })))
  const sec = Math.round((epochMs - now) / 1000)
  const abs = Math.abs(sec)
  if (abs < 60) return f.format(0, 'second')
  if (abs < 3600) return f.format(Math.round(sec / 60), 'minute')
  if (abs < 86400) return f.format(Math.round(sec / 3600), 'hour')
  if (abs < 86400 * 30) return f.format(Math.round(sec / 86400), 'day')
  if (abs < 86400 * 365) return f.format(Math.round(sec / (86400 * 30)), 'month')
  return f.format(Math.round(sec / (86400 * 365)), 'year')
}

/**
 * 원격 전용 저장소의 빈 커밋 목록에 무엇을 보일지.
 * fetching: 아직 받는 중(상태가 오기 전 포함) · failed: 받지 못했다 · empty: 받았는데 커밋이 없다
 */
export function githubEmptyState(status: FetchStatus | undefined): 'fetching' | 'failed' | 'empty' {
  if (!status) return 'fetching'
  if (status.fetching) return 'fetching'
  if (status.error || status.blocked || status.authPaused || status.needsManual) return 'failed'
  return status.lastOkAt === null ? 'fetching' : 'empty'
}

/** 원격 전용 저장소의 GitHub 주소 */
export function githubUrl(link: { owner: string; repo: string }): string {
  return `https://github.com/${link.owner}/${link.repo}`
}

/** fetch 진행 상태를 저장소와 함께 기억한 값 */
export interface FetchSeen {
  repoId: string | null
  fetching: boolean
}

/**
 * 원격 전용 저장소에서 fetch 가 끝났을 때 로그를 처음부터 다시 읽을지.
 * 같은 저장소에서 fetching 이 true → false 로 바뀌었고 목록이 비어 있을 때만(첫 이력 도착·빈 저장소 오류 해소).
 * 이력이 이미 있으면 repo:changed 의 softReload 가 선택·스크롤을 지킨 채 갱신하므로 건드리지 않는다.
 */
export function shouldReloadAfterFetch(prev: FetchSeen | null, cur: FetchSeen, commitCount: number): boolean {
  return !!cur.repoId && !!prev && prev.repoId === cur.repoId && prev.fetching && !cur.fetching && commitCount === 0
}
