import type { RefsDetails, RefsSnapshot } from '@shared/types'

/**
 * 업스트림 ahead/behind·서브모듈 같은 미룬 부가 정보를 스냅샷에 합친다(원본은 바꾸지 않는다).
 * tracking 은 변화 없는 브랜치를 빼고 오므로, 업스트림이 있는데 없는 브랜치는 동기화된 것이다 — 이전 값을 지운다.
 */
export function withDetails(refs: RefsSnapshot, d: RefsDetails): RefsSnapshot {
  return {
    ...refs,
    submodules: d.submodules,
    local: refs.local.map((b) => {
      if (!b.upstream) return b
      const { ahead: _a, behind: _b, upstreamGone: _g, ...rest } = b
      return { ...rest, ...d.tracking[b.fullName] }
    })
  }
}

/** 새 스냅샷이 오는 동안 이전 부가 정보를 그대로 보여 준다(새 정보가 도착할 때까지 배지가 깜빡이지 않게) */
export function carryDetails(next: RefsSnapshot, prev: RefsSnapshot | null): RefsSnapshot {
  if (!prev) return next
  const tracking: RefsDetails['tracking'] = {}
  for (const b of prev.local) {
    if (b.ahead !== undefined || b.behind !== undefined || b.upstreamGone) {
      tracking[b.fullName] = {
        ...(b.ahead !== undefined ? { ahead: b.ahead } : {}),
        ...(b.behind !== undefined ? { behind: b.behind } : {}),
        ...(b.upstreamGone ? { upstreamGone: true } : {})
      }
    }
  }
  return withDetails(next, { tracking, submodules: prev.submodules })
}
