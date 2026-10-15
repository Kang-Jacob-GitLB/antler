import { create } from 'zustand'
import type { CommitDetail, FileChange, FileDiff } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'

/** renderer 쪽 커밋 상세 캐시(커밋은 불변이라 해시만으로 충분하다). ↑↓ 로 오갈 때 즉시 보이게 한다 */
const detailCache = new Map<string, CommitDetail>()
const DETAIL_CACHE_MAX = 120
/** 캐시에 담긴 변경 파일 수 합계 상한(파일이 수천 개인 merge 가 쌓여 메모리를 잡아먹지 않게) */
const DETAIL_CACHE_FILES = 40_000
let cachedFiles = 0

function remember(key: string, d: CommitDetail): void {
  const prev = detailCache.get(key)
  if (prev) {
    cachedFiles -= prev.files.length
    detailCache.delete(key)
  }
  detailCache.set(key, d)
  cachedFiles += d.files.length
  while (detailCache.size > 1 && (detailCache.size > DETAIL_CACHE_MAX || cachedFiles > DETAIL_CACHE_FILES)) {
    const oldest = detailCache.keys().next().value!
    cachedFiles -= detailCache.get(oldest)!.files.length
    detailCache.delete(oldest)
  }
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
  /** 지금 diff 가 full 요청으로 받은 것인지(그래도 잘렸으면 더 받을 수 없다) */
  diffFull: boolean
  /** 전체 불러오기 실패(부분 diff 는 유지) */
  fullError: string | null
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
  diffFull: false,
  fullError: null,
  pendingDetail: null,
  pendingDiff: null,

  async load(repoId, hash) {
    const s = get()
    if (s.repoId === repoId && s.hash === hash) return
    cancel(s.pendingDetail)
    cancel(s.pendingDiff)
    // 다른 커밋으로 옮기면 열려 있던 diff 는 닫는다(그 커밋에 같은 파일이 없을 수 있다)
    set({ repoId, hash, file: null, diff: null, diffLoading: false, diffError: null, diffFull: false, fullError: null, pendingDiff: null })
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
    // "전체 불러오기"는 받는 동안 이미 보이던 부분 diff 를 그대로 둔다
    set({ file, diffLoading: true, diffError: null, fullError: null, pendingDiff: requestId, ...(full ? {} : { diff: null, diffFull: false }) })
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
      // 늦게 도착한 이전 요청의 응답은 버린다(같은 파일을 다시 연 경우도 요청 ID 로 구분)
      if (get().pendingDiff !== requestId) return
      set({ diff, diffFull: full, diffLoading: false, pendingDiff: null })
    } catch (err) {
      if (get().pendingDiff !== requestId || isAbort(err)) return
      // 전체 불러오기 실패는 부분 diff 를 지우지 않고 안내만 한다
      if (full && get().diff) set({ diffLoading: false, fullError: errorMessage(err), pendingDiff: null })
      else set({ diffLoading: false, diffError: errorMessage(err), pendingDiff: null })
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
    set({ file: null, diff: null, diffLoading: false, diffError: null, diffFull: false, fullError: null, pendingDiff: null })
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
      diffFull: false,
      fullError: null,
      pendingDetail: null,
      pendingDiff: null
    })
  }
}))
