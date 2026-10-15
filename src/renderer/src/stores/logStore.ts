import { create } from 'zustand'
import type { Commit } from '@shared/types'
import { graphEngine } from '../graph/graphEngine'
import { api, errorMessage, isAbort, isStale, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'

/** 한 번에 가져오는 커밋 수. 보이는 영역 끝에서 PREFETCH 행 전에 다음 페이지를 요청한다. */
export const PAGE_SIZE = 1000
export const PREFETCH = 300
/** 멀리 있는 커밋을 찾아갈 때 쓰는 큰 페이지 */
const REVEAL_PAGE_SIZE = 5000
let revealToken = 0

/** 그래프를 특정 ref 기준으로만 보는 필터 */
export interface LogFilter {
  /** git 에 넘길 rev(refs/heads/x, 커밋 해시, stash@{0}) */
  rev: string
  /** 화면 표시용 이름 */
  label: string
}

interface LogState {
  repoId: string | null
  filter: LogFilter | null
  commits: Commit[]
  /** hash → index */
  index: Map<string, number>
  hasMore: boolean
  loading: boolean
  error: string | null
  selected: number
  /** 저장소 전환·새로고침마다 증가. 늦게 도착한 이전 응답을 버리는 데 쓴다 */
  generation: number
  pendingRequest: string | null
  /** 목록이 이 행으로 스크롤해야 함을 알리는 요청(같은 값 재요청도 감지하도록 seq 포함) */
  scrollRequest: { index: number; seq: number } | null

  open(repoId: string | null, filter?: LogFilter | null): void
  setFilter(filter: LogFilter | null): void
  reload(): void
  loadMore(count?: number): Promise<void>
  select(index: number): void
  /** 해시의 커밋을 (필요하면 더 불러와서) 선택하고 보이게 한다. 찾으면 true */
  reveal(hash: string): Promise<boolean>
}

export const useLogStore = create<LogState>((set, get) => ({
  repoId: null,
  filter: null,
  commits: [],
  index: new Map(),
  hasMore: false,
  loading: false,
  error: null,
  selected: -1,
  generation: 0,
  pendingRequest: null,
  scrollRequest: null,

  open(repoId, filter = null) {
    const pending = get().pendingRequest
    if (pending) void api.invoke('req:cancel', pending)
    const generation = get().generation + 1
    graphEngine.reset(generation)
    // 이전 저장소의 커밋은 즉시 버린다(비활성 저장소 메모리 해제)
    set({
      repoId,
      filter,
      commits: [],
      index: new Map(),
      hasMore: repoId !== null,
      loading: false,
      error: null,
      selected: -1,
      generation,
      pendingRequest: null,
      scrollRequest: null
    })
    if (repoId) void get().loadMore()
  },

  setFilter(filter) {
    get().open(get().repoId, filter)
  },

  reload() {
    get().open(get().repoId, get().filter)
  },

  async loadMore(count = PAGE_SIZE) {
    const { repoId, loading, hasMore, commits, generation, filter } = get()
    if (!repoId || loading || !hasMore) return
    const requestId = newRequestId('log')
    set({ loading: true, pendingRequest: requestId })
    try {
      const start = commits.length
      const page = await api.invoke('git:log', repoId, { skip: start, count, ref: filter?.rev }, requestId)
      if (get().generation !== generation) return
      graphEngine.append(generation, start, page.commits)
      set((s) => {
        const index = s.index
        page.commits.forEach((c, i) => index.set(c.hash, start + i))
        return {
          commits: s.commits.concat(page.commits),
          index,
          hasMore: page.hasMore,
          loading: false,
          pendingRequest: null,
          selected: s.selected < 0 && start === 0 && page.commits.length ? 0 : s.selected
        }
      })
    } catch (err) {
      if (get().generation !== generation) return
      if (isStale(err)) {
        // 페이지를 넘기는 사이 이력이 바뀌었다 — 이어 붙이면 중복·누락이 생기므로 처음부터 다시 읽는다.
        // ref 도 움직였을 것이므로 사이드바 스냅샷도 갱신한다
        get().reload()
        void useAppStore.getState().refreshRefs()
        return
      }
      // 취소는 오류가 아니다. 다음 스크롤에서 다시 요청할 수 있게 hasMore 를 유지한다
      if (isAbort(err)) set({ loading: false, pendingRequest: null })
      else set({ loading: false, pendingRequest: null, error: errorMessage(err), hasMore: false })
    }
  },

  select(index) {
    const n = get().commits.length
    if (!n) return
    set({ selected: Math.max(0, Math.min(n - 1, index)) })
  },

  async reveal(hash) {
    const generation = get().generation
    // 더 나중에 요청된 reveal 이 있으면 이 루프는 물러난다(먼 커밋을 찾는 동안 다른 항목을 클릭한 경우)
    const token = ++revealToken
    for (;;) {
      const s = get()
      if (s.generation !== generation || token !== revealToken) return false
      const i = s.index.get(hash)
      if (i !== undefined) {
        set({ selected: i, scrollRequest: { index: i, seq: (s.scrollRequest?.seq ?? 0) + 1 } })
        return true
      }
      if (!s.hasMore || s.error) return false
      if (s.loading) {
        // 진행 중인 페이지가 끝나기를 기다린다
        await new Promise<void>((resolve) => {
          const unsub = useLogStore.subscribe((st) => {
            if (!st.loading) {
              unsub()
              resolve()
            }
          })
        })
      } else {
        await s.loadMore(REVEAL_PAGE_SIZE)
      }
    }
  }
}))
