import { create } from 'zustand'
import type { GhLaunchpadResult } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { emptyFilter, setSnooze, togglePin, type LpEntry, type LpFilter, type LpScope, type LpSection } from '../lib/launchpad'
import { useAppStore } from './appStore'
import { useGithubStore } from './githubStore'
import { useJiraStore } from './jiraStore'

/** 이 시간이 지난 결과는 화면을 열 때 다시 받는다(그 안에서는 새로고침 버튼으로만) */
const STALE_MS = 5 * 60_000

interface LaunchpadState {
  open: boolean
  scope: LpScope
  section: LpSection
  filter: LpFilter
  /**
   * 마지막 검색 결과 하나. 검색은 사용자 기준이고 범위(전체·그룹·저장소)는 이 결과를 거르기만 하므로 범위를 바꿔도 다시 받지 않는다
   */
  result: GhLaunchpadResult | null
  /** 이 결과를 받을 때의 저장소 목록 서명(저장소를 추가·제거하면 다시 받는다) */
  repos: string
  loading: boolean
  /** 호출 자체가 실패했다(한도 같은 부분 실패는 result.errors) */
  error: string | null

  show(open?: boolean): void
  toggle(): void
  setScope(scope: LpScope): void
  setSection(section: LpSection): void
  setFilter(patch: Partial<LpFilter>): void
  /** 검색 결과를 불러온다. force 가 아니면 최근 결과가 있을 때 다시 받지 않는다 */
  load(force?: boolean): Promise<void>
  /** 항목을 연다: GitHub 항목은 그 저장소로 전환해 PR·이슈 화면을, Jira 항목은 Jira 상세를 */
  openEntry(entry: LpEntry): void
  togglePin(key: string): void
  snooze(key: string, until: number | null): void
}

let seq = 0
let reqId: string | null = null

const repoSignature = (): string => (useAppStore.getState().config?.repos ?? []).map((r) => r.id).join(',')

export const useLaunchpadStore = create<LaunchpadState>((set, get) => ({
  open: false,
  scope: { kind: 'all' },
  section: 'authored',
  filter: emptyFilter,
  result: null,
  repos: '',
  loading: false,
  error: null,

  show(open = true) {
    if (open === get().open) return
    set({ open })
    if (open) {
      void get().load(false)
      // Jira 는 연결해 있으면 같은 시점에 새로 고친다(오래된 목록을 보여 주지 않도록)
      const jira = useJiraStore.getState()
      if (jira.auth.connected && !jira.mine.loading) void jira.loadMine()
    } else if (reqId) {
      void api.invoke('req:cancel', reqId).catch(() => {})
      reqId = null
      seq++
      set({ loading: false })
    }
  },

  toggle() {
    get().show(!get().open)
  },

  setScope(scope) {
    // 범위가 바뀌면 옛 범위의 저장소 필터는 의미가 없다. 결과는 그대로 두고 거르기만 한다
    set({ scope, filter: { ...get().filter, repoId: null } })
  },

  setSection(section) {
    set({ section })
  },

  setFilter(patch) {
    set({ filter: { ...get().filter, ...patch } })
  },

  async load(force = false) {
    const sig = repoSignature()
    const hit = get().result
    if (!force && hit && !hit.needsLogin && get().repos === sig && Date.now() - hit.fetchedAt < STALE_MS) return
    if (reqId) void api.invoke('req:cancel', reqId).catch(() => {})
    const my = ++seq
    const id = newRequestId('launchpad')
    reqId = id
    set({ loading: true, error: null })
    try {
      const result = await api.invoke('gh:launchpad', id, force)
      if (my !== seq) return
      set({ result, repos: sig, loading: false })
      // 남은 호출 수 등 인증 상태가 바뀌었을 수 있다
      void useGithubStore.getState().refreshAuth()
    } catch (err) {
      if (my !== seq || isAbort(err)) return
      set({ loading: false, error: errorMessage(err) })
    } finally {
      if (my === seq) reqId = null
    }
  },

  openEntry(entry) {
    if (entry.kind === 'jira' && entry.jiraKey) {
      // Jira 화면이 Launchpad 위를 덮는다 — 뒤로가기로 Launchpad 에 돌아온다
      useJiraStore.getState().openIssue(entry.jiraKey)
      return
    }
    // 목록에서 이미 빠진 저장소는 열지 않는다
    if (!entry.repoId || entry.number === undefined || !useAppStore.getState().config?.repos.some((r) => r.id === entry.repoId)) return
    get().show(false)
    useGithubStore.getState().openAt(entry.repoId, entry.kind === 'pull' ? { kind: 'pull', number: entry.number, tab: 'conversation' } : { kind: 'issue', number: entry.number })
  },

  togglePin(key) {
    const app = useAppStore.getState()
    void app.setUi({ launchpadPins: togglePin(app.config?.ui.launchpadPins ?? [], key) })
  },

  snooze(key, until) {
    const app = useAppStore.getState()
    const next = setSnooze(app.config?.ui.launchpadSnoozes ?? {}, key, until, Date.now())
    void app.setUi({ launchpadSnoozes: Object.keys(next).length ? next : undefined })
  }
}))

// 다른 화면(사이드바의 PR·이슈, Jira, 저장소 전환)이 중앙을 차지하면 Launchpad 는 닫는다
useGithubStore.subscribe((s, prev) => {
  if (s.view && s.view !== prev.view) useLaunchpadStore.getState().show(false)
})
useJiraStore.subscribe((s, prev) => {
  if (s.view && s.view !== prev.view) useLaunchpadStore.getState().show(false)
})
useAppStore.subscribe((s, prev) => {
  if (s.activeRepoId !== prev.activeRepoId) useLaunchpadStore.getState().show(false)
})
// 로그인·로그아웃하면 열려 있는 Launchpad 를 새로 받는다
useGithubStore.subscribe((s, prev) => {
  if (s.auth.signedIn !== prev.auth.signedIn && useLaunchpadStore.getState().open) void useLaunchpadStore.getState().load(true)
})
