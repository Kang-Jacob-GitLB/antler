// 브랜치 → 레인 색 인덱스. 같은 브랜치는 세션이 바뀌어도 같은 색이 되도록 이름 해시로 정한다.
// 그래프 worker(M2)도 같은 함수를 쓴다.

export const LANE_COUNT = 8
const MAIN_NAMES = new Set(['main', 'master', 'trunk'])

/** FNV-1a 32bit */
export function fnv1a(s: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return h >>> 0
}

/** origin/feature/x → feature/x. 로컬과 리모트가 같은 색을 쓰게 한다. */
export function branchKey(name: string, isRemote: boolean): string {
  if (!isRemote) return name
  const i = name.indexOf('/')
  return i >= 0 ? name.slice(i + 1) : name
}

/** main/master 는 0(앰버) 고정, 나머지는 1..7 */
export function laneIndexFor(branch: string): number {
  if (MAIN_NAMES.has(branch)) return 0
  return 1 + (fnv1a(branch) % (LANE_COUNT - 1))
}

export const laneVar = (i: number): string => `var(--lane-${i % LANE_COUNT})`
