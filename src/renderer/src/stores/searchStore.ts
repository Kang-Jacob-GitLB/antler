import { create } from 'zustand'
import { api, errorMessage } from '../lib/api'
import { useAppStore } from './appStore'
import { useLogStore } from './logStore'

interface SearchState {
  open: boolean
  query: string
  /** 그래프 순서의 일치 커밋 해시 */
  hits: string[]
  hitSet: Set<string>
  cursor: number
  loading: boolean
  truncated: boolean
  error: string | null
  /** 늦게 도착한 이전 검색 결과를 버리는 데 쓴다 */
  seq: number

  show(): void
  close(): void
  setQuery(q: string): void
  run(): Promise<void>
  step(dir: 1 | -1): void
}

const EMPTY = { hits: [] as string[], hitSet: new Set<string>(), cursor: -1, truncated: false, error: null }

export const useSearchStore = create<SearchState>((set, get) => ({
  open: false,
  query: '',
  ...EMPTY,
  loading: false,
  seq: 0,

  show() {
    set({ open: true })
  },

  close() {
    set((s) => ({ open: false, query: '', ...EMPTY, loading: false, seq: s.seq + 1 }))
  },

  setQuery(query) {
    set({ query })
  },

  async run() {
    const query = get().query.trim()
    const repoId = useAppStore.getState().activeRepoId
    const seq = get().seq + 1
    if (!query || !repoId) {
      set({ ...EMPTY, loading: false, seq })
      return
    }
    set({ loading: true, seq, error: null })
    try {
      const res = await api.invoke('git:search', repoId, query, useLogStore.getState().filter?.rev)
      if (get().seq !== seq) return
      set({ hits: res.hashes, hitSet: new Set(res.hashes), truncated: res.truncated, cursor: -1, loading: false })
      if (res.hashes.length) get().step(1)
    } catch (err) {
      if (get().seq !== seq) return
      set({ ...EMPTY, loading: false, error: errorMessage(err) })
    }
  },

  step(dir) {
    const { hits, cursor } = get()
    if (!hits.length) return
    const next = (cursor + dir + hits.length) % hits.length
    set({ cursor: next })
    void useLogStore.getState().reveal(hits[next]!)
  }
}))
