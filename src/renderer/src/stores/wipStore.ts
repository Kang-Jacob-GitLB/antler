import { create } from 'zustand'
import type { FileChange, WipArea, WipStatus } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'

/** 저장소가 GitHub 원격 전용(작업 폴더 없음)인가 */
function isRemoteOnly(repoId: string): boolean {
  return !!useAppStore.getState().config?.repos.find((r) => r.id === repoId)?.github
}

/** 변경 파일 한 줄: 어느 영역(스테이지/작업 트리/추적 안 됨)의 파일인지 함께 든다 */
export interface WipEntry {
  area: WipArea
  file: FileChange
}

const entryCache = new WeakMap<WipStatus, WipEntry[]>()

/** 패널에 보이는 순서(스테이지 안 됨 → 추적 안 됨 → 스테이지됨)의 평탄한 목록. 상태 객체마다 한 번만 만든다 */
export function wipEntries(status: WipStatus | null): WipEntry[] {
  if (!status) return []
  let list = entryCache.get(status)
  if (!list) {
    list = [
      ...status.unstaged.map((file) => ({ area: 'unstaged' as const, file })),
      // 추적 안 되는 파일은 새 파일(A)처럼 보여 주되 영역으로 구분한다
      ...status.untracked.map((path) => ({
        area: 'untracked' as const,
        file: { status: 'A' as const, path, additions: null, deletions: null, binary: false }
      })),
      ...status.staged.map((file) => ({ area: 'staged' as const, file }))
    ]
    entryCache.set(status, list)
  }
  return list
}

/** WIP 행을 보여 줄 변경이 있는가 */
export const wipDirty = (s: WipStatus | null): boolean => !!s && s.counts.staged + s.counts.unstaged + s.counts.untracked > 0

/** 이 시간 넘게 응답이 없는 상태 조회는 멈춘 것으로 보고 새로 요청한다 */
const STUCK_MS = 30_000
let pendingSince = 0

interface WipState {
  repoId: string | null
  status: WipStatus | null
  error: string | null
  /** 목록에서 WIP 행을 선택했는가(커밋 선택과 배타적) */
  selected: boolean
  pending: string | null

  /** 상태를 다시 읽는다. 이미 읽는 중이면 그 요청을 기다리지 않고 건너뛴다 */
  refresh(): Promise<void>
  select(on: boolean): void
  /** 저장소가 바뀌었을 때 */
  reset(repoId: string | null): void
}

export const useWipStore = create<WipState>((set, get) => ({
  repoId: null,
  status: null,
  error: null,
  selected: false,
  pending: null,

  async refresh() {
    const { repoId, pending } = get()
    if (!repoId) return
    // GitHub 원격 전용 저장소는 작업 폴더가 없다 — 변경(WIP)을 읽지 않는다
    if (isRemoteOnly(repoId)) return
    if (pending) {
      // 멈춘 요청(응답 없는 원격)이 갱신을 영영 막지 않도록 오래되면 끊고 다시 읽는다
      if (Date.now() - pendingSince < STUCK_MS) return
      void api.invoke('req:cancel', pending)
    }
    const requestId = newRequestId('wip')
    pendingSince = Date.now()
    set({ pending: requestId })
    try {
      const next = await api.invoke('git:wipStatus', repoId, requestId)
      if (get().repoId !== repoId || get().pending !== requestId) return
      const prev = get().status
      // 바뀐 게 없으면 같은 객체를 유지해 화면이 다시 그려지지 않게 한다
      const same = prev && JSON.stringify(prev) === JSON.stringify(next)
      set({ status: same ? prev : next, error: null, pending: null })
    } catch (err) {
      if (get().repoId !== repoId || get().pending !== requestId) return
      // 일시적인 실패(원격 끊김 등)로 이전 상태를 지우지 않는다
      set({ error: isAbort(err) ? null : errorMessage(err), pending: null })
    }
  },

  select(on) {
    if (get().selected !== on) set({ selected: on })
  },

  reset(repoId) {
    const { pending } = get()
    if (pending) void api.invoke('req:cancel', pending)
    set({ repoId, status: null, error: null, selected: false, pending: null })
  }
}))
