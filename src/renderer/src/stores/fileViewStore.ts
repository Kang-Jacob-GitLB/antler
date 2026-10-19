import { create } from 'zustand'
import type { BlameResult, FileChange, FileDiff, FileHistoryEntry, WipArea } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'

/** 한 번에 받는 히스토리 항목 수 */
export const HISTORY_PAGE = 100

export type FileViewTab = 'history' | 'blame'

interface FileViewState {
  /** 열려 있는 화면(null 이면 닫힘) */
  tab: FileViewTab | null
  repoId: string | null
  /** 보고 있는 파일 경로(히스토리의 시작 경로이기도 하다) */
  path: string
  /** 시작 커밋. null 이면 HEAD(작업 트리 파일) */
  rev: string | null
  /** 작업 트리(WIP) 파일에서 열었는지(HEAD 기준 안내용) */
  fromWip: boolean

  entries: FileHistoryEntry[]
  hasMore: boolean
  historyLoading: boolean
  historyError: string | null
  /** 히스토리에서 고른 커밋 */
  selected: FileHistoryEntry | null
  diff: FileDiff | null
  diffLoading: boolean
  diffError: string | null
  diffFull: boolean

  /** blame 대상 커밋·경로(히스토리에서 "이 시점의 Blame" 으로 바뀔 수 있다). rev 가 null 이면 HEAD */
  blameRev: string | null
  blamePath: string
  blame: BlameResult | null
  blameLoading: boolean
  blameError: string | null

  pendingHistory: string | null
  pendingDiff: string | null
  pendingBlame: string | null

  /** 파일 화면을 연다. rev: 커밋 해시(커밋의 파일), null: HEAD(작업 트리 파일) */
  open(tab: FileViewTab, repoId: string, path: string, rev: string | null): void
  setTab(tab: FileViewTab): void
  loadMore(): Promise<void>
  select(entry: FileHistoryEntry, full?: boolean): Promise<void>
  /** 히스토리의 한 커밋 시점의 blame 으로 */
  blameAt(entry: FileHistoryEntry): void
  close(): void
}

const cancel = (id: string | null): void => {
  if (id) void api.invoke('req:cancel', id)
}

const CLOSED = {
  tab: null,
  repoId: null,
  path: '',
  rev: null,
  fromWip: false,
  entries: [],
  hasMore: false,
  historyLoading: false,
  historyError: null,
  selected: null,
  diff: null,
  diffLoading: false,
  diffError: null,
  diffFull: false,
  blameRev: null,
  blamePath: '',
  blame: null,
  blameLoading: false,
  blameError: null,
  pendingHistory: null,
  pendingDiff: null,
  pendingBlame: null
} satisfies Partial<FileViewState>

/**
 * 파일 행에서 히스토리·Blame 을 열 수 있는지. 커밋의 삭제된 파일은 그 시점에 내용이 없어 Blame 이 안 되고,
 * 작업 트리의 추적 안 됨·새로 추가된 파일은 HEAD 에 없어 둘 다 볼 이력이 없다.
 */
export function fileViewAvailability(file: FileChange, wipArea: WipArea | null): { history: boolean; blame: boolean } {
  if (wipArea) {
    const none = wipArea === 'untracked' || file.status === 'A'
    return { history: !none, blame: !none }
  }
  return { history: true, blame: file.status !== 'D' }
}

/** 파일 행의 히스토리·Blame 열기. rev 가 null 이면 작업 트리 파일이라 HEAD 에 있는 이름(이름 변경 전)으로 연다 */
export function openFileView(tab: FileViewTab, repoId: string, file: FileChange, rev: string | null): void {
  const path = rev === null && file.oldPath ? file.oldPath : file.path
  useFileViewStore.getState().open(tab, repoId, path, rev)
}

export const useFileViewStore = create<FileViewState>((set, get) => {
  const cancelAll = (): void => {
    const s = get()
    cancel(s.pendingHistory)
    cancel(s.pendingDiff)
    cancel(s.pendingBlame)
  }

  async function loadBlame(): Promise<void> {
    const { repoId, blameRev, blamePath, pendingBlame } = get()
    if (!repoId) return
    cancel(pendingBlame)
    const requestId = newRequestId('blame')
    set({ blame: null, blameLoading: true, blameError: null, pendingBlame: requestId })
    try {
      const blame = await api.invoke('git:blame', repoId, { path: blamePath, ...(blameRev ? { rev: blameRev } : {}) }, requestId)
      if (get().pendingBlame !== requestId) return
      set({ blame, blameLoading: false, pendingBlame: null })
    } catch (err) {
      if (get().pendingBlame !== requestId || isAbort(err)) return
      set({ blameLoading: false, blameError: errorMessage(err), pendingBlame: null })
    }
  }

  return {
    ...CLOSED,

    open(tab, repoId, path, rev) {
      cancelAll()
      set({ ...CLOSED, tab, repoId, path, rev, fromWip: rev === null, blameRev: rev, blamePath: path })
      if (tab === 'history') void get().loadMore()
      else void loadBlame()
    },

    setTab(tab) {
      const s = get()
      if (!s.tab || s.tab === tab) return
      set({ tab })
      if (tab === 'history' && s.entries.length === 0 && !s.historyLoading && !s.historyError) void get().loadMore()
      if (tab === 'blame' && !s.blame && !s.blameLoading && !s.blameError) void loadBlame()
    },

    async loadMore() {
      const { repoId, path, rev, entries, historyLoading, pendingHistory } = get()
      if (!repoId || historyLoading) return
      cancel(pendingHistory)
      const requestId = newRequestId('history')
      set({ historyLoading: true, historyError: null, pendingHistory: requestId })
      try {
        const page = await api.invoke('git:fileHistory', repoId, { path, skip: entries.length, count: HISTORY_PAGE, ...(rev ? { rev } : {}) }, requestId)
        if (get().pendingHistory !== requestId) return
        const seen = new Set(entries.map((e) => e.hash))
        const merged = [...entries, ...page.entries.filter((e) => !seen.has(e.hash))]
        set({ entries: merged, hasMore: page.hasMore, historyLoading: false, pendingHistory: null })
        // 처음 받은 페이지면 맨 위 커밋의 diff 를 바로 보여 준다
        if (!get().selected && merged[0]) void get().select(merged[0])
      } catch (err) {
        if (get().pendingHistory !== requestId || isAbort(err)) return
        set({ historyLoading: false, historyError: errorMessage(err), pendingHistory: null })
      }
    },

    async select(entry, full = false) {
      const { repoId, pendingDiff } = get()
      if (!repoId) return
      cancel(pendingDiff)
      const requestId = newRequestId('diff')
      // "전체 불러오기"는 받는 동안 이미 보이던 부분 diff 를 그대로 둔다
      set({ selected: entry, diffLoading: true, diffError: null, pendingDiff: requestId, ...(full ? {} : { diff: null, diffFull: false }) })
      try {
        const diff = await api.invoke(
          'git:fileDiff',
          repoId,
          { hash: entry.hash, path: entry.path, ...(entry.oldPath ? { oldPath: entry.oldPath } : {}), full },
          requestId
        )
        if (get().pendingDiff !== requestId) return
        set({ diff, diffFull: full, diffLoading: false, pendingDiff: null })
      } catch (err) {
        if (get().pendingDiff !== requestId || isAbort(err)) return
        set({ diffLoading: false, diffError: errorMessage(err), pendingDiff: null })
      }
    },

    blameAt(entry) {
      if (!get().tab) return
      set({ tab: 'blame', blameRev: entry.hash, blamePath: entry.path })
      void loadBlame()
    },

    close() {
      cancelAll()
      set({ ...CLOSED })
    }
  }
})
