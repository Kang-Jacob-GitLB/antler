/**
 * 두 커밋 비교의 방향. 커밋 목록은 최신이 위(인덱스가 작음)이므로 아래쪽(오래된) 커밋이 from 이다.
 * 목록에 아직 불러오지 않은 커밋은 더 아래(오래된 쪽)로 본다. 둘 다 모르면 a 가 from.
 * swapped 면 방향을 뒤집는다(사용자가 "방향 바꾸기"를 누른 경우).
 */
export function orderCompare(a: string, b: string, index: Map<string, number>, swapped: boolean): { from: string; to: string } {
  const ia = index.get(a) ?? Infinity
  const ib = index.get(b) ?? Infinity
  const aOlder = ia >= ib
  const [from, to] = aOlder ? [a, b] : [b, a]
  return swapped ? { from: to, to: from } : { from, to }
}
