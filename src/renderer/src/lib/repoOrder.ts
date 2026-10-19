/**
 * 사이드바 저장소 목록(환경 카드 · 그룹 · 저장소)의 사용자 순서.
 * 순서 목록(ui.sourceOrder · groupOrder · repoOrder)에 있는 것은 그 순서로, 없는 것(새로 추가된 저장소·그룹)은 기본 정렬로 뒤에 붙는다.
 * 없어진 id 는 목록에 남아 있어도 무시되고, 다음에 순서를 바꿀 때 지금 있는 것만으로 다시 써서 정리된다.
 */

export function sortByOrder<T>(items: readonly T[], key: (t: T) => string, order: readonly string[] | undefined, fallback: (a: T, b: T) => number): T[] {
  const rank = new Map((order ?? []).map((id, i) => [id, i]))
  return [...items].sort((a, b) => {
    const ra = rank.get(key(a))
    const rb = rank.get(key(b))
    if (ra !== undefined && rb !== undefined) return ra - rb
    if (ra !== undefined) return -1
    if (rb !== undefined) return 1
    return fallback(a, b)
  })
}

/** list 에서 from 을 target 바로 앞(after=false)이나 뒤로 옮긴 새 목록. 둘 중 하나라도 없거나 같으면 그대로 */
export function moveItem(list: readonly string[], from: string, target: string, after: boolean): string[] {
  if (from === target || !list.includes(from) || !list.includes(target)) return [...list]
  const rest = list.filter((x) => x !== from)
  const at = rest.indexOf(target) + (after ? 1 : 0)
  return [...rest.slice(0, at), from, ...rest.slice(at)]
}

/**
 * 전체 순서 all 에서 subset 이 차지하던 자리들을 subset 의 새 순서로 채운다(나머지는 제자리).
 * 화면에 보이는 일부(한 그룹 안 저장소, 숨긴 그룹을 뺀 목록)만 바꿔도 전체 순서가 어긋나지 않는다.
 */
export function permuteSlots(all: readonly string[], subset: readonly string[]): string[] {
  const inAll = new Set(all)
  const sub = subset.filter((id) => inAll.has(id))
  const members = new Set(sub)
  let i = 0
  return all.map((id) => (members.has(id) ? sub[i++]! : id))
}
