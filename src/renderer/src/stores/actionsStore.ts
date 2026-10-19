import { create } from 'zustand'
import type { GhJob, GhRun } from '@shared/types'
import { api, errorMessage } from '../lib/api'

interface JobsState {
  loading: boolean
  error: string | null
  list: GhJob[] | null
}

interface ActionsState {
  repoId: string | null
  runs: GhRun[]
  hasMore: boolean
  /** "더 불러오기" 때 요청할 페이지 */
  nextPage: number
  loading: boolean
  error: string | null
  /** 잡 목록을 펼친 실행 */
  selected: number | null
  jobs: Record<number, JobsState>

  /** 첫 쪽(more=false)이나 다음 쪽을 받는다. 저장소가 바뀌었으면 비우고 처음부터 */
  load(repoId: string, more?: boolean): Promise<void>
  select(repoId: string, runId: number | null): void
  reset(): void
}

const initial = { repoId: null, runs: [], hasMore: false, nextPage: 2, loading: false, error: null, selected: null, jobs: {} }

/** 요청 순번 — 저장소 전환·새로고침 중 늦게 온 응답을 버린다 */
let seq = 0

export const useActionsStore = create<ActionsState>((set, get) => ({
  ...initial,

  async load(repoId, more = false) {
    const prev = get()
    const sameRepo = prev.repoId === repoId
    const mine = ++seq
    const page = more && sameRepo ? prev.nextPage : 1
    const reopen = !more && sameRepo ? prev.selected : null
    // 새로고침이면 잡 캐시도 버린다(진행 중이던 실행이 끝났을 수 있다). 펼쳐 둔 실행은 목록을 받은 뒤 다시 조회한다
    set({ ...(sameRepo ? {} : { ...initial }), repoId, loading: true, error: null, ...(more && sameRepo ? {} : { runs: [], hasMore: false, jobs: {} }) })
    try {
      const res = await api.invoke('gh:runs', repoId, page)
      if (mine !== seq) return
      set((s) => {
        // 쪽 경계에서 새 실행이 생기면 같은 실행이 두 쪽에 걸칠 수 있어 id 로 중복을 뺀다
        const seen = new Set(more && sameRepo ? s.runs.map((r) => r.id) : [])
        const fresh = res.items.filter((r) => !seen.has(r.id))
        return { runs: more && sameRepo ? [...s.runs, ...fresh] : fresh, hasMore: res.hasMore, nextPage: page + 1, loading: false }
      })
      if (reopen !== null && res.items.some((r) => r.id === reopen)) get().select(repoId, reopen)
    } catch (err) {
      if (mine === seq) set({ loading: false, error: errorMessage(err) })
    }
  },

  select(repoId, runId) {
    set({ selected: runId })
    if (runId === null) return
    const cur = get().jobs[runId]
    if (cur && (cur.list || cur.loading)) return
    set((s) => ({ jobs: { ...s.jobs, [runId]: { loading: true, error: null, list: null } } }))
    api.invoke('gh:runJobs', repoId, runId).then(
      (list) => get().repoId === repoId && set((s) => ({ jobs: { ...s.jobs, [runId]: { loading: false, error: null, list } } })),
      (err) => get().repoId === repoId && set((s) => ({ jobs: { ...s.jobs, [runId]: { loading: false, error: errorMessage(err), list: null } } }))
    )
  },

  reset() {
    seq++
    set({ ...initial })
  }
}))
