// 미니맵 클릭: 목록을 어떤 시점(epoch 초)의 커밋으로 옮긴다. 아직 불러오지 않았으면 페이지를 더 읽으며 찾아간다.
import { create } from 'zustand'
import type { Commit } from '@shared/types'
import { api } from '../lib/api'
import { useLogStore } from './logStore'

/** 찾아가며 한 번에 읽는 커밋 수 */
const JUMP_PAGE_SIZE = 5000
/** 이만큼 불러도 못 찾으면 멈춘다(메모리·레인 계산 상한) */
export const JUMP_MAX_COMMITS = 150_000

interface JumpState {
  jumping: boolean
}
export const useJumpStore = create<JumpState>(() => ({ jumping: false }))

let token = 0

/** time 이하의 작성 시각을 가진 첫 커밋의 위치(from 부터 탐색). 없으면 -1 */
export function findAtOrBefore(commits: readonly Pick<Commit, 'authorDate'>[], time: number, from = 0): number {
  // --date-order 는 엄밀한 시간순이 아니므로 이진 탐색 대신 선형으로 훑는다
  for (let i = from; i < commits.length; i++) if (commits[i]!.authorDate <= time) return i
  return -1
}

/** 진행 중인 찾기를 취소한다(읽던 페이지 요청도 끊는다) */
export function cancelJump(): void {
  // 찾는 중이 아니면 할 일이 없다(일반 스크롤 페이지 요청을 끊지 않는다)
  if (!useJumpStore.getState().jumping) return
  token++
  useJumpStore.setState({ jumping: false })
  const pending = useLogStore.getState().pendingRequest
  if (pending) void api.invoke('req:cancel', pending).catch(() => {})
}

/** 목록에 스크롤을 요청한다 */
function scrollTo(index: number): void {
  const s = useLogStore.getState()
  useLogStore.setState({ scrollRequest: { index, seq: (s.scrollRequest?.seq ?? 0) + 1 } })
}

/**
 * time 이전(이하)의 첫 커밋으로 이동한다. 끝까지 읽어도 더 오래된 커밋뿐이면 마지막 커밋으로 간다.
 * 상한(JUMP_MAX_COMMITS)에 걸리거나 취소·저장소 전환이 있으면 false.
 */
export async function jumpToTime(time: number): Promise<boolean> {
  const my = ++token
  const generation = useLogStore.getState().generation
  useJumpStore.setState({ jumping: true })
  let scanned = 0
  try {
    for (;;) {
      const s = useLogStore.getState()
      if (my !== token || s.generation !== generation) return false
      const i = findAtOrBefore(s.commits, time, scanned)
      if (i >= 0) {
        scrollTo(i)
        return true
      }
      scanned = s.commits.length
      if (!s.hasMore || s.error) {
        // 그 시점보다 오래된 커밋이 없다 — 가장 오래된 커밋으로
        if (s.commits.length) scrollTo(s.commits.length - 1)
        return s.commits.length > 0
      }
      if (s.commits.length >= JUMP_MAX_COMMITS) {
        scrollTo(s.commits.length - 1)
        return false
      }
      if (s.loading) {
        // 진행 중인 페이지(스크롤 프리페치 등)가 끝나기를 기다린다
        await new Promise<void>((resolve) => {
          const unsub = useLogStore.subscribe((st) => {
            if (!st.loading) {
              unsub()
              resolve()
            }
          })
        })
      } else {
        await s.loadMore(JUMP_PAGE_SIZE)
        // 읽다 취소됐으면(loadMore 는 취소를 삼킨다) 다음 반복의 token 검사에서 빠진다
      }
    }
  } finally {
    if (my === token) useJumpStore.setState({ jumping: false })
  }
}
