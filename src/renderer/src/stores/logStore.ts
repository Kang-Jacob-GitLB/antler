import { create } from 'zustand'
import type { Commit } from '@shared/types'
import { graphEngine } from '../graph/graphEngine'
import { api, errorMessage, isAbort, isStale, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'
import { useCompareStore } from './compareStore'
import { useDetailStore } from './detailStore'
import { closeGithubView } from './githubStore'

/** 한 번에 가져오는 커밋 수. 보이는 영역 끝에서 PREFETCH 행 전에 다음 페이지를 요청한다. */
export const PAGE_SIZE = 1000
export const PREFETCH = 300
/** 멀리 있는 커밋을 찾아갈 때 쓰는 큰 페이지 */
const REVEAL_PAGE_SIZE = 5000
/** 부드러운 다시 읽기에서 한 번에 읽는 최대 커밋 수 */
const SOFT_RELOAD_MAX = 5000
let revealToken = 0
/** 다시 읽은 첫 페이지에서 선택을 이어 갈 커밋 */
let keepAfterReload: string | null = null
function firstSelection(commits: Commit[]): number {
  const i = keepAfterReload ? commits.findIndex((c) => c.hash === keepAfterReload) : -1
  keepAfterReload = null
  return i >= 0 ? i : 0
}

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
  /** 두 커밋 비교 모드: 선택한 커밋과 비교할 다른 커밋의 해시(null 이면 비교 중이 아님) */
  compareHash: string | null
  /** 비교를 시작한 때 선택돼 있던 커밋. 선택이 이 커밋에서 벗어나면(찾아가기·다시 읽기 등) 비교를 끝낸다 */
  compareAnchor: string | null
  /** 비교 방향을 사용자가 뒤집었는지(기본은 목록에서 아래쪽 = 오래된 커밋이 from) */
  compareSwapped: boolean
  /** 저장소 전환·새로고침마다 증가. 늦게 도착한 이전 응답을 버리는 데 쓴다 */
  generation: number
  pendingRequest: string | null
  /** 목록이 이 행으로 스크롤해야 함을 알리는 요청(같은 값 재요청도 감지하도록 seq 포함) */
  scrollRequest: { index: number; seq: number } | null

  /** keepHash: 첫 페이지에 그 커밋이 있으면 선택을 유지한다(외부 변경으로 다시 읽을 때) */
  open(repoId: string | null, filter?: LogFilter | null, keepHash?: string): void
  setFilter(filter: LogFilter | null): void
  reload(keepSelection?: boolean): void
  /**
   * 외부 변경(커밋·fetch) 반영용 부드러운 다시 읽기: 목록을 비우지 않고 첫 구간을 새로 읽어 한 번에 바꾼다.
   * 선택한 커밋이 그 구간에 있으면 선택(과 열린 diff)을 그대로 둔다.
   */
  softReload(): Promise<void>
  loadMore(count?: number): Promise<void>
  select(index: number): void
  /** 선택한 커밋과 index 의 커밋을 비교 모드로 연다(Ctrl/⌘·Shift+클릭) */
  compareWith(index: number): void
  /** 해시의 커밋을 선택하고 other 와 비교 모드로 연다(사이드바 "현재 HEAD 와 비교"). 성공하면 true */
  compareHashes(hash: string, other: string): Promise<boolean>
  swapCompare(): void
  /** 비교 모드를 끝낸다(비교로 열었던 diff 도 닫는다) */
  clearCompare(): void
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
  compareHash: null,
  compareAnchor: null,
  compareSwapped: false,
  generation: 0,
  pendingRequest: null,
  scrollRequest: null,

  open(repoId, filter = null, keepHash) {
    keepAfterReload = keepHash ?? null
    useCompareStore.getState().clear()
    const pending = get().pendingRequest
    if (pending) void api.invoke('req:cancel', pending)
    const generation = get().generation + 1
    // 저장소가 바뀌면 이전 저장소의 기본 브랜치 tip 도 버린다
    if (repoId !== get().repoId) graphEngine.clearDefaultTip()
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
      compareHash: null,
      compareAnchor: null,
      compareSwapped: false,
      generation,
      pendingRequest: null,
      scrollRequest: null
    })
    if (repoId) void get().loadMore()
  },

  setFilter(filter) {
    // ref 필터는 그래프에 적용되므로 GitHub 화면을 보고 있었다면 닫아 그래프가 보이게 한다
    closeGithubView()
    get().open(get().repoId, filter)
  },

  reload(keepSelection = false) {
    const s = get()
    const keep = keepSelection && s.selected >= 0 ? s.commits[s.selected]?.hash : undefined
    get().open(s.repoId, s.filter, keep)
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
          selected: s.selected < 0 && start === 0 && page.commits.length ? firstSelection(page.commits) : s.selected
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

  async softReload() {
    const s = get()
    if (!s.repoId) return
    // 페이지를 읽는 중이면 그 결과와 섞이지 않도록 일반 다시 읽기로 대신한다
    if (s.loading) return get().reload(true)
    const keep = s.selected >= 0 ? s.commits[s.selected]?.hash : undefined
    // 지금까지 불러온 만큼(최대 SOFT_RELOAD_MAX) 다시 읽어 스크롤 위치 근처가 그대로 보이게 한다
    const count = Math.min(Math.max(PAGE_SIZE, s.commits.length), SOFT_RELOAD_MAX)
    const myGen = s.generation
    const requestId = newRequestId('log')
    set({ loading: true, pendingRequest: requestId })
    try {
      const page = await api.invoke('git:log', s.repoId, { skip: 0, count, ref: s.filter?.rev }, requestId)
      if (get().generation !== myGen) return
      const generation = myGen + 1
      graphEngine.reset(generation)
      graphEngine.append(generation, 0, page.commits)
      const index = new Map<string, number>()
      page.commits.forEach((c, i) => index.set(c.hash, i))
      const kept = keep !== undefined ? index.get(keep) : undefined
      set({
        generation,
        commits: page.commits,
        index,
        hasMore: page.hasMore,
        loading: false,
        pendingRequest: null,
        error: null,
        selected: kept ?? (page.commits.length ? 0 : -1)
      })
    } catch (err) {
      if (get().generation !== myGen) return
      set({ loading: false, pendingRequest: null })
      if (isStale(err)) get().reload(true)
      else if (!isAbort(err)) set({ error: errorMessage(err) })
    }
  },

  select(index) {
    const n = get().commits.length
    if (!n) return
    get().clearCompare()
    set({ selected: Math.max(0, Math.min(n - 1, index)) })
  },

  compareWith(index) {
    const { commits, selected } = get()
    const other = commits[index]
    if (!other || selected < 0 || index === selected) return
    // 열려 있던 diff 는 이전 비교(또는 커밋)의 것이다
    useDetailStore.getState().closeFile()
    set({ compareHash: other.hash, compareAnchor: commits[selected]?.hash ?? null, compareSwapped: false })
  },

  async compareHashes(hash, other) {
    if (hash === other) return false
    if (!(await get().reveal(hash))) return false
    set({ compareHash: other, compareAnchor: hash, compareSwapped: false })
    return true
  },

  swapCompare() {
    useDetailStore.getState().closeFile()
    set((s) => ({ compareSwapped: !s.compareSwapped }))
  },

  clearCompare() {
    if (!get().compareHash) return
    useDetailStore.getState().closeFile()
    useCompareStore.getState().clear()
    set({ compareHash: null, compareAnchor: null, compareSwapped: false })
  },

  async reveal(hash) {
    // 그래프에서 찾아가는 것이므로 diff 를 보고 있었다면 그래프로 돌아온다(숨겨진 목록을 스크롤하지 않게)
    useDetailStore.getState().closeFile()
    closeGithubView()
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

// 선택이 비교의 기준 커밋에서 벗어나면 비교를 끝낸다. reveal·softReload 등 select() 를 거치지 않고 selected 를 바꾸는 경로를 모두 덮는다
useLogStore.subscribe((s) => {
  if (s.compareHash && s.commits[s.selected]?.hash !== s.compareAnchor) s.clearCompare()
})
