import { create } from 'zustand'
import type { JiraUser, JiraWorklogResult } from '@shared/types'
import { rangeFor, shiftAnchor, todayYmd, type WorklogGroup, type WorklogMode } from '../components/jira/worklog/worklogModel'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useJiraStore } from './jiraStore'

export type WorklogTab = 'calendar' | 'report'

/** 같은 범위·사용자의 결과를 이 시간 동안은 다시 묻지 않는다(탭 전환·이슈를 보고 돌아올 때). 새로고침은 건너뛴다 */
const CACHE_TTL_MS = 2 * 60_000
const CACHE_MAX = 24

interface WorklogState {
  tab: WorklogTab
  mode: WorklogMode
  /** 기준일(YYYY-MM-DD, 로컬) */
  anchor: string
  /** '나'를 포함하는지. 팀원을 하나라도 고르면 끌 수 있고, 아무도 안 남기는 선택은 허용하지 않는다 */
  includeMe: boolean
  /** 고른 팀원 id */
  selectedTeam: string[]
  /** 마지막으로 받은 결과와 그 요청 키(지금 범위와 다르면 새 응답을 기다리는 중) */
  data: JiraWorklogResult | null
  dataKey: string | null
  /** 응답에서 알게 된 연결 계정 */
  me: JiraUser | null
  loading: boolean
  error: string | null
  /** 리포트 묶음 기준과 접힌 행(이슈를 보고 돌아와도 유지) */
  reportGroup: WorklogGroup
  collapsed: string[]

  setTab(tab: WorklogTab): void
  setMode(mode: WorklogMode): void
  shift(dir: -1 | 1): void
  goToday(): void
  /** 그 날의 일간 보기로 */
  openDay(ymd: string): void
  toggleMe(): void
  setReportGroup(g: WorklogGroup): void
  toggleCollapsed(id: string): void
  setCollapsed(ids: string[]): void
  toggleTeam(id: string): void
  /** 팀 목록에서 사라진 사람을 선택에서 뺀다 */
  pruneTeam(ids: string[]): void
  load(force?: boolean): Promise<void>
  reset(): void
}

let seq = 0
let reqId: string | null = null
/** 진행 중인 요청의 키(같은 키로 다시 부르면 취소하지 않고 그대로 기다린다) */
let inflightKey: string | null = null
const cache = new Map<string, { at: number; result: JiraWorklogResult }>()

function cancelReq(): void {
  if (reqId) void api.invoke('req:cancel', reqId).catch(() => {})
  reqId = null
  inflightKey = null
}

/** 요청에 실을 팀원 id(내 id 는 renderer 가 모른다 — '나'는 includeMe 로 main 에 맡긴다) */
export function queryUsers(s: Pick<WorklogState, 'selectedTeam'>): string[] {
  return [...s.selectedTeam]
}

export function requestKey(from: string, to: string, users: string[], includeMe: boolean): string {
  // 팀원이 없으면 항상 '나만'이라 includeMe 는 키에 영향이 없다
  return `${from}|${to}|${[...users].sort().join(',')}|${users.length === 0 || includeMe ? 'me' : '-'}`
}

const initial = (): Pick<WorklogState, 'includeMe' | 'selectedTeam' | 'data' | 'dataKey' | 'me' | 'loading' | 'error' | 'reportGroup' | 'collapsed'> => ({
  includeMe: true,
  selectedTeam: [],
  data: null,
  dataKey: null,
  me: null,
  loading: false,
  error: null,
  reportGroup: 'user',
  collapsed: []
})

export const useWorklogStore = create<WorklogState>((set, get) => {
  const change = (patch: Partial<WorklogState>): void => {
    set(patch)
    void get().load()
  }

  return {
    tab: 'calendar',
    mode: 'month',
    anchor: todayYmd(),
    ...initial(),

    setTab: (tab) => set({ tab }),
    setMode: (mode) => change({ mode }),
    shift: (dir) => change({ anchor: shiftAnchor(get().mode, get().anchor, dir) }),
    goToday: () => change({ anchor: todayYmd() }),
    openDay: (ymd) => change({ mode: 'day', anchor: ymd }),

    setReportGroup: (reportGroup) => set({ reportGroup }),
    toggleCollapsed: (id) => set((s) => ({ collapsed: s.collapsed.includes(id) ? s.collapsed.filter((x) => x !== id) : [...s.collapsed, id] })),
    setCollapsed: (collapsed) => set({ collapsed }),

    toggleMe() {
      const s = get()
      // 나를 끄려면 팀원이 하나는 남아 있어야 한다
      if (s.includeMe && s.selectedTeam.length === 0) return
      change({ includeMe: !s.includeMe })
    },

    toggleTeam(id) {
      const s = get()
      if (s.selectedTeam.includes(id)) {
        const selectedTeam = s.selectedTeam.filter((x) => x !== id)
        // 마지막 팀원을 빼면 '나'로 돌아간다
        change({ selectedTeam, includeMe: selectedTeam.length === 0 ? true : s.includeMe })
      } else {
        change({ selectedTeam: [...s.selectedTeam, id] })
      }
    },

    pruneTeam(ids) {
      const s = get()
      const keep = s.selectedTeam.filter((id) => ids.includes(id))
      if (keep.length !== s.selectedTeam.length) change({ selectedTeam: keep, includeMe: keep.length === 0 ? true : s.includeMe })
    },

    async load(force = false) {
      if (!useJiraStore.getState().auth.connected) return
      const s = get()
      const { from, to } = rangeFor(s.mode, s.anchor)
      const users = queryUsers(s)
      const includeMe = users.length === 0 || s.includeMe
      const key = requestKey(from, to, users, includeMe)
      // 같은 요청이 이미 가는 중이면 취소하지 않고 그 응답을 기다린다(이슈를 보고 돌아온 경우)
      if (!force && reqId && inflightKey === key) {
        set({ loading: true })
        return
      }
      const mine = ++seq
      cancelReq()

      const hit = force ? undefined : cache.get(key)
      if (hit && Date.now() - hit.at < CACHE_TTL_MS) {
        set({ data: hit.result, dataKey: key, me: hit.result.me, loading: false, error: null })
        return
      }
      const id = newRequestId('jira')
      reqId = id
      inflightKey = key
      set({ loading: true, error: null })
      try {
        const result = await api.invoke('jira:worklogs', { from, to, users, includeMe }, id)
        if (mine !== seq) return
        reqId = null
        inflightKey = null
        if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value as string)
        cache.set(key, { at: Date.now(), result })
        set({ data: result, dataKey: key, me: result.me, loading: false })
      } catch (err) {
        if (mine !== seq || isAbort(err)) return
        reqId = null
        inflightKey = null
        set({ loading: false, error: errorMessage(err), data: null, dataKey: null })
      }
    },

    reset() {
      seq++
      cancelReq()
      cache.clear()
      set(initial())
    }
  }
})

// 연결을 끊거나 다른 사이트로 바꾸면 받아 둔 결과·선택을 비운다(다른 사이트의 사람 id 로 조회하지 않게)
useJiraStore.subscribe((s, prev) => {
  if (prev.auth.connected && (!s.auth.connected || s.auth.siteUrl !== prev.auth.siteUrl)) useWorklogStore.getState().reset()
})
