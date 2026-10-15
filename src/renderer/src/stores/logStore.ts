import { create } from 'zustand'
import type { Commit } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'

/** 한 번에 가져오는 커밋 수. 보이는 영역 끝에서 PREFETCH 행 전에 다음 페이지를 요청한다. */
export const PAGE_SIZE = 1000
export const PREFETCH = 300

interface LogState {
  repoId: string | null
  commits: Commit[]
  hasMore: boolean
  loading: boolean
  error: string | null
  selected: number
  /** 저장소 전환·새로고침마다 증가. 늦게 도착한 이전 응답을 버리는 데 쓴다 */
  generation: number
  pendingRequest: string | null

  open(repoId: string | null): void
  reload(): void
  loadMore(): Promise<void>
  select(index: number): void
}

export const useLogStore = create<LogState>((set, get) => ({
  repoId: null,
  commits: [],
  hasMore: false,
  loading: false,
  error: null,
  selected: -1,
  generation: 0,
  pendingRequest: null,

  open(repoId) {
    const pending = get().pendingRequest
    if (pending) void api.invoke('req:cancel', pending)
    // 이전 저장소의 커밋은 즉시 버린다(비활성 저장소 메모리 해제)
    set((s) => ({
      repoId,
      commits: [],
      hasMore: repoId !== null,
      loading: false,
      error: null,
      selected: -1,
      generation: s.generation + 1,
      pendingRequest: null
    }))
    if (repoId) void get().loadMore()
  },

  reload() {
    get().open(get().repoId)
  },

  async loadMore() {
    const { repoId, loading, hasMore, commits, generation } = get()
    if (!repoId || loading || !hasMore) return
    const requestId = newRequestId('log')
    set({ loading: true, pendingRequest: requestId })
    try {
      const page = await api.invoke('git:log', repoId, { skip: commits.length, count: PAGE_SIZE }, requestId)
      if (get().generation !== generation) return
      set((s) => ({
        commits: s.commits.concat(page.commits),
        hasMore: page.hasMore,
        loading: false,
        pendingRequest: null,
        selected: s.selected < 0 && s.commits.length === 0 && page.commits.length ? 0 : s.selected
      }))
    } catch (err) {
      if (get().generation !== generation) return
      set({ loading: false, pendingRequest: null, error: isAbort(err) ? null : errorMessage(err), hasMore: false })
    }
  },

  select(index) {
    const n = get().commits.length
    if (!n) return
    set({ selected: Math.max(0, Math.min(n - 1, index)) })
  }
}))
