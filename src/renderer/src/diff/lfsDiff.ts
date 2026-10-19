import { pointerFromLines, type LfsPointer } from '@shared/lfs'
import type { FileDiff } from '@shared/types'

export interface LfsDiff {
  /** 변경 전 포인터(새 파일이거나 LFS 전환 전이면 null) */
  before: LfsPointer | null
  /** 변경 후 포인터(삭제거나 LFS 해제 후면 null) */
  after: LfsPointer | null
  /** 한쪽만 포인터인 전환 커밋: to = 일반 → LFS, from = LFS → 일반. 이때는 실제 diff 도 함께 보여 준다 */
  converted?: 'to' | 'from'
}

/**
 * diff 가 LFS 포인터 파일의 변경이면 양쪽 포인터를 돌려준다. 실제 LFS 객체는 받지 않고 diff 텍스트만 해석한다.
 * 잘려서 일부만 담긴 diff 는 포인터로 보지 않는다(포인터는 몇 줄뿐이라 정상이면 잘리지 않는다).
 */
export function lfsFromDiff(diff: FileDiff | null): LfsDiff | null {
  if (!diff || diff.binary || diff.truncated || diff.hunks.length === 0 || diff.hunks.length > 2) return null
  const before: string[] = []
  const after: string[] = []
  for (const h of diff.hunks) {
    for (const l of h.lines) {
      if (l.t === '\\') continue
      if (l.t !== '+') before.push(l.text)
      if (l.t !== '-') after.push(l.text)
    }
  }
  const b = pointerFromLines(before)
  const a = pointerFromLines(after)
  if (!b && !a) return null
  // 한쪽이 포인터가 아닌 실제 내용이면(LFS 도입·해제) 추가·삭제가 아니라 전환이다. 빈 쪽은 새 파일·삭제
  if (!b && before.length > 0) return { before: b, after: a, converted: 'to' }
  if (!a && after.length > 0) return { before: b, after: a, converted: 'from' }
  return { before: b, after: a }
}
