import { create } from 'zustand'
import type { CommitDetail, FileChange, FileDiff } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'

/** renderer 쪽 커밋 상세 캐시(커밋은 불변이라 해시만으로 충분하다). ↑↓ 로 오갈 때 즉시 보이게 한다 */
const detailCache = new Map<string, CommitDetail>()
const DETAIL_CACHE_MAX = 120

function remember(key: string, d: CommitDetail): void {
  detailCache.delete(key)
  detailCache.set(key, d)
  if (detailCache.size > DETAIL_CACHE_MAX) detailCache.delete(detailCache.keys().next().value!)
}

interface DetailState {
  repoId: string | null
  hash: string | null
  detail: CommitDetail | null
  loading: boolean
  error: string | null
  /** diff 를 보고 있는 파일(null 이면 중앙에 커밋 그래프) */
  file: FileChange | null
  diff: FileDiff | null
  diffLoading: boolean
  diffError: string | null
  pendingDetail: string | null
  pendingDiff: string | null

  load(repoId: string | null, hash: string | null): Promise<void>
  openFile(file: FileChange, full?: boolean): Promise<void>
  /** 목록에서 앞/뒤 파일로 */
  stepFile(dir: 1 | -1): void
  closeFile(): void
  reset(): void
}

const cancel = (id: string | null): void => {
  if (id) void api.invoke('req:cancel', id)
}

export const useDetailStore = create<DetailState>((set, get) => ({
  repoId: null,
  hash: null,
  detail: null,
  loading: false,
  error: null,
  file: null,
  diff: null,
  diffLoading: false,
  diffError: null,
  pendingDetail: null,
  pendingDiff: null,

  async load(repoId, hash) {
    const s = get()
    if (s.repoId === repoId && s.hash === hash) return
    cancel(s.pendingDetail)
    cancel(s.pendingDiff)
    // 다른 커밋으로 옮기면 열려 있던 diff 는 닫는다(그 커밋에 같은 파일이 없을 수 있다)
    set({ repoId, hash, file: null, diff: null, diffLoading: false, diffError: null, pendingDiff: null })
    if (!repoId || !hash) {
      set({ detail: null, loading: false, error: null, pendingDetail: null })
      return
    }
    const key = `${repoId}\0${hash}`
    const cached = detailCache.get(key)
    if (cached) {
      set({ detail: cached, loading: false, error: null, pendingDetail: null })
      return
    }
    const requestId = newRequestId('detail')
    set({ loading: true, error: null, pendingDetail: requestId })
    try {
      const detail = await api.invoke('git:commit', repoId, hash, requestId)
      remember(key, detail)
      if (get().hash !== hash || get().repoId !== repoId) return
      set({ detail, loading: false, pendingDetail: null })
    } catch (err) {
      if (get().hash !== hash || isAbort(err)) return
      set({ detail: null, loading: false, error: errorMessage(err), pendingDetail: null })
    }
  },

  async openFile(file, full = false) {
    const { repoId, detail, pendingDiff } = get()
    if (!repoId || !detail) return
    cancel(pendingDiff)
    const requestId = newRequestId('diff')
    set({ file, diffLoading: true, diffError: null, pendingDiff: requestId, ...(full ? {} : { diff: null }) })
    try {
      const diff = await api.invoke(
        'git:fileDiff',
        repoId,
        {
          hash: detail.hash,
          path: file.path,
          ...(file.oldPath ? { oldPath: file.oldPath } : {}),
          ...(detail.baseParent ? { parent: detail.baseParent } : {}),
          full
        },
        requestId
      )
      if (get().file !== file) return
      set({ diff, diffLoading: false, pendingDiff: null })
    } catch (err) {
      if (get().file !== file || isAbort(err)) return
      set({ diffLoading: false, diffError: errorMessage(err), pendingDiff: null })
    }
  },

  stepFile(dir) {
    const { detail, file } = get()
    if (!detail || !file) return
    const i = detail.files.indexOf(file)
    const next = detail.files[i + dir]
    if (next) void get().openFile(next)
  },

  closeFile() {
    cancel(get().pendingDiff)
    set({ file: null, diff: null, diffLoading: false, diffError: null, pendingDiff: null })
  },

  reset() {
    cancel(get().pendingDetail)
    cancel(get().pendingDiff)
    set({
      repoId: null,
      hash: null,
      detail: null,
      loading: false,
      error: null,
      file: null,
      diff: null,
      diffLoading: false,
      diffError: null,
      pendingDetail: null,
      pendingDiff: null
    })
  }
}))
