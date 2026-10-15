import { create } from 'zustand'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'
import { useLogStore } from './logStore'

interface SearchState {
  open: boolean
  query: string
  /** 마지막으로 실행한 질의(입력값과 다르면 Enter 는 다시 검색한다) */
  ranQuery: string
  /** 그래프 순서의 일치 커밋 해시 */
  hits: string[]
  hitSet: Set<string>
  cursor: number
  loading: boolean
  truncated: boolean
  error: string | null
  /** 늦게 도착한 이전 검색 결과를 버리는 데 쓴다 */
  seq: number
  /** Ctrl/Cmd+F 를 다시 누를 때 입력창에 포커스를 돌려주는 신호 */
  focusSeq: number
  pendingRequest: string | null

  show(): void
  close(): void
  setQuery(q: string): void
  run(): Promise<void>
  step(dir: 1 | -1): void
}

const EMPTY = { hits: [] as string[], hitSet: new Set<string>(), cursor: -1, truncated: false, error: null, ranQuery: '' }

export const useSearchStore = create<SearchState>((set, get) => {
  const cancelPending = (): void => {
    const id = get().pendingRequest
    if (id) void api.invoke('req:cancel', id)
  }

  return {
    open: false,
    query: '',
    ...EMPTY,
    loading: false,
    seq: 0,
    focusSeq: 0,
    pendingRequest: null,

    show() {
      set((s) => ({ open: true, focusSeq: s.focusSeq + 1 }))
    },

    close() {
      cancelPending()
      set((s) => ({ open: false, query: '', ...EMPTY, loading: false, seq: s.seq + 1, pendingRequest: null }))
    },

    setQuery(query) {
      set({ query })
    },

    async run() {
      const query = get().query.trim()
      const repoId = useAppStore.getState().activeRepoId
      const seq = get().seq + 1
      cancelPending()
      if (!query || !repoId) {
        set({ ...EMPTY, loading: false, seq, pendingRequest: null })
        return
      }
      const requestId = newRequestId('search')
      // 이전 결과는 바로 비운다 — 새 질의에 옛 일치 표시가 남지 않게
      set({ ...EMPTY, ranQuery: query, loading: true, seq, pendingRequest: requestId })
      try {
        const res = await api.invoke('git:search', repoId, query, useLogStore.getState().filter?.rev, requestId)
        if (get().seq !== seq) return
        set({ hits: res.hashes, hitSet: new Set(res.hashes), truncated: res.truncated, cursor: -1, loading: false, pendingRequest: null })
        if (res.hashes.length) get().step(1)
      } catch (err) {
        if (get().seq !== seq || isAbort(err)) return
        set({ ...EMPTY, ranQuery: query, loading: false, error: errorMessage(err), pendingRequest: null })
      }
    },

    step(dir) {
      const { hits, cursor } = get()
      if (!hits.length) return
      const next = (cursor + dir + hits.length) % hits.length
      set({ cursor: next })
      void useLogStore.getState().reveal(hits[next]!)
    }
  }
})
