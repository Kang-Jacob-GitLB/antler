import { create } from 'zustand'
import type { CompareResult } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'

interface CompareState {
  repoId: string | null
  /** 지금 불러온(또는 불러오는 중인) 비교의 방향: from(오래된 쪽) → to(새 쪽) */
  from: string | null
  to: string | null
  result: CompareResult | null
  loading: boolean
  error: string | null
  pending: string | null

  load(repoId: string, from: string, to: string): Promise<void>
  clear(): void
}

const cancel = (id: string | null): void => {
  if (id) void api.invoke('req:cancel', id)
}

const EMPTY = { repoId: null, from: null, to: null, result: null, loading: false, error: null, pending: null }

/** 두 커밋 비교 모드의 변경 파일 목록. result 가 있으면 비교 모드 — diff 를 열 때 이 쌍으로 연다 */
export const useCompareStore = create<CompareState>((set, get) => ({
  ...EMPTY,

  async load(repoId, from, to) {
    const s = get()
    if (s.repoId === repoId && s.from === from && s.to === to && (s.result || s.loading)) return
    cancel(s.pending)
    const requestId = newRequestId('compare')
    set({ repoId, from, to, result: null, loading: true, error: null, pending: requestId })
    try {
      const result = await api.invoke('git:compare', repoId, from, to, requestId)
      if (get().pending !== requestId) return
      set({ result, loading: false, pending: null })
    } catch (err) {
      if (get().pending !== requestId || isAbort(err)) return
      set({ loading: false, error: errorMessage(err), pending: null })
    }
  },

  clear() {
    cancel(get().pending)
    set(EMPTY)
  }
}))
