// Launchpad 화면의 순수 로직: 검색 결과·Jira 이슈를 한 목록 모델로 합치고, 필터·핀·스누즈를 적용해 정렬한다.
import type { GhCheckState, GhLabel, GhLaunchpadItem, GhUser, JiraIssueSummary, Repo } from '@shared/types'
import { ghItemKey, jiraItemKey, MAX_LAUNCHPAD_PINS, MAX_LAUNCHPAD_SNOOZES } from '@shared/launchpad'

export type LpSection = 'authored' | 'review' | 'issues' | 'jira'
export const LP_SECTIONS: readonly LpSection[] = ['authored', 'review', 'issues', 'jira']

/** 화면 목록 한 줄 */
export interface LpEntry {
  /** 핀·스누즈 키(`owner/repo#번호` 또는 `jira:KEY`) */
  key: string
  section: LpSection
  kind: 'pull' | 'issue' | 'jira'
  title: string
  /** GitHub 항목 */
  repoId?: string
  slug?: string
  repoName?: string
  group?: string
  number?: number
  state?: 'open' | 'closed' | 'merged'
  draft: boolean
  author?: GhUser
  labels: GhLabel[]
  checks?: GhCheckState
  /** Jira 항목 */
  jiraKey?: string
  status?: string
  statusCategory?: string
  assignee?: string
  /** epoch seconds */
  updatedAt: number
}

/** 범위: 전체 저장소 / 그룹 / 저장소 하나 */
export type LpScope = { kind: 'all' } | { kind: 'group'; name: string } | { kind: 'repo'; id: string }

export const scopeKey = (s: LpScope): string => (s.kind === 'all' ? 'all' : s.kind === 'group' ? `group:${s.name}` : `repo:${s.id}`)

/** 범위에 드는 저장소(GitHub remote 가 있는지는 main 이 걸러 낸다) */
export function scopeRepos(scope: LpScope, repos: Repo[]): Repo[] {
  if (scope.kind === 'all') return repos
  if (scope.kind === 'group') return repos.filter((r) => r.group === scope.name)
  return repos.filter((r) => r.id === scope.id)
}

/**
 * 검색 결과 항목 → 화면 항목. 범위(scope)에 드는 저장소를 remote 로 가진 항목만 남기고(전체면 등록된 저장소 전부),
 * 저장소 이름·그룹은 그중 첫 저장소에서 가져온다. 이미 목록에서 빠진 저장소의 항목은 나오지 않는다.
 */
export function toEntries(items: GhLaunchpadItem[], repos: Repo[], scope: LpScope = { kind: 'all' }): LpEntry[] {
  const byId = new Map(scopeRepos(scope, repos).map((r) => [r.id, r]))
  const out: LpEntry[] = []
  for (const it of items) {
    const repo = it.repoIds.map((id) => byId.get(id)).find((r) => r !== undefined)
    if (!repo) continue
    out.push({
      key: ghItemKey(it.owner, it.repo, it.number),
      section: it.section,
      kind: it.kind,
      title: it.title,
      repoId: repo.id,
      slug: `${it.owner}/${it.repo}`,
      repoName: repo.name,
      ...(repo.group ? { group: repo.group } : {}),
      number: it.number,
      state: it.state,
      draft: it.draft,
      author: it.author,
      labels: it.labels,
      ...(it.checks ? { checks: it.checks } : {}),
      updatedAt: it.updatedAt
    })
  }
  return out
}

/** Jira 이슈 → 화면 항목(updated 는 ISO 문자열) */
export function jiraEntries(items: JiraIssueSummary[]): LpEntry[] {
  return items.map((it) => {
    const t = Date.parse(it.updated)
    return {
      key: jiraItemKey(it.key),
      section: 'jira' as const,
      kind: 'jira' as const,
      title: it.summary,
      jiraKey: it.key,
      status: it.status,
      statusCategory: it.statusCategory,
      assignee: it.assignee,
      draft: false,
      labels: [],
      updatedAt: Number.isNaN(t) ? 0 : Math.floor(t / 1000)
    }
  })
}

export interface LpFilter {
  text: string
  /** GitHub 항목을 이 저장소(앱의 저장소 ID)로 한정. null 이면 전부 */
  repoId: string | null
  hideDrafts: boolean
  /** 스누즈한 항목도 보인다 */
  showSnoozed: boolean
}

export const emptyFilter: LpFilter = { text: '', repoId: null, hideDrafts: false, showSnoozed: false }

function matchesText(e: LpEntry, q: string): boolean {
  const hay = [e.title, e.slug, e.repoName, e.group, e.jiraKey, e.author?.login, e.assignee, e.status, e.number !== undefined ? `#${e.number}` : '', ...e.labels.map((l) => l.name)]
  return hay.some((h) => !!h && h.toLowerCase().includes(q))
}

export const isSnoozed = (key: string, snoozes: Record<string, number>, now: number): boolean => (snoozes[key] ?? 0) > now

export interface LpView {
  items: LpEntry[]
  /** 스누즈 때문에 가려진 항목 수 */
  snoozed: number
}

/**
 * 한 섹션에 보일 항목: 필터를 적용하고 핀을 맨 위(핀끼리·나머지끼리는 최근 업데이트 순)로 정렬한다.
 * 스누즈 기간이 끝나지 않은 항목은 숨긴다(showSnoozed 면 보인다). 저장소 필터와 초안 숨기기는 GitHub 항목에만 적용된다.
 */
export function visibleEntries(entries: LpEntry[], section: LpSection, f: LpFilter, pins: string[], snoozes: Record<string, number>, now: number): LpView {
  const q = f.text.trim().toLowerCase()
  const pinned = new Set(pins)
  let snoozed = 0
  const items = entries.filter((e) => {
    if (e.section !== section) return false
    if (f.repoId && e.kind !== 'jira' && e.repoId !== f.repoId) return false
    if (f.hideDrafts && e.draft) return false
    if (q && !matchesText(e, q)) return false
    if (isSnoozed(e.key, snoozes, now)) {
      snoozed++
      return f.showSnoozed
    }
    return true
  })
  items.sort((a, b) => Number(pinned.has(b.key)) - Number(pinned.has(a.key)) || b.updatedAt - a.updatedAt || a.key.localeCompare(b.key))
  return { items, snoozed }
}

/** 핀 토글. 상한을 넘으면 오래된 핀부터 버린다 */
export function togglePin(pins: string[], key: string): string[] {
  return pins.includes(key) ? pins.filter((k) => k !== key) : [...pins, key].slice(-MAX_LAUNCHPAD_PINS)
}

/** 기간이 끝난 스누즈를 지운다. 바뀐 게 없으면 같은 객체를 돌려준다 */
export function pruneSnoozes(snoozes: Record<string, number>, now: number): Record<string, number> {
  const live = Object.entries(snoozes).filter(([, until]) => until > now)
  return live.length === Object.keys(snoozes).length ? snoozes : Object.fromEntries(live)
}

/** 스누즈 지정(until: epoch ms) 또는 해제(null). 끝난 항목은 정리하고 상한을 넘으면 먼저 끝나는 것부터 버린다 */
export function setSnooze(snoozes: Record<string, number>, key: string, until: number | null, now: number): Record<string, number> {
  const next = { ...pruneSnoozes(snoozes, now) }
  delete next[key]
  if (until !== null && until > now) next[key] = until
  const entries = Object.entries(next)
  if (entries.length <= MAX_LAUNCHPAD_SNOOZES) return next
  return Object.fromEntries(entries.sort((a, b) => b[1] - a[1]).slice(0, MAX_LAUNCHPAD_SNOOZES))
}

export type SnoozePreset = '1h' | '4h' | 'tomorrow' | '1w'

/** 스누즈 프리셋의 끝 시각(epoch ms). tomorrow 는 다음 날 오전 9시(로컬) */
export function snoozeUntil(preset: SnoozePreset, now: number): number {
  const H = 3_600_000
  if (preset === '1h') return now + H
  if (preset === '4h') return now + 4 * H
  if (preset === '1w') return now + 7 * 24 * H
  const d = new Date(now)
  d.setDate(d.getDate() + 1)
  d.setHours(9, 0, 0, 0)
  return d.getTime()
}
