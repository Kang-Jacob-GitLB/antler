/**
 * 저장소 그룹 칩 색(hue).
 * 그룹이 처음 생기면 아직 다른 그룹이 쓰지 않는 색 하나를 고르고 설정(ui.groupColors)에 저장한다.
 * 그래서 그룹끼리 색이 겹치지 않고(팔레트가 남아 있는 동안), 한 번 정해진 색은 계속 유지된다.
 * 서로 구분이 잘 되는 색상만 고른 팔레트에서 뽑는다(채도·명도는 테마별로 CSS 가 정한다).
 */
export const GROUP_HUES = [8, 32, 50, 95, 145, 172, 195, 215, 250, 280, 310, 335] as const

export type GroupColors = Record<string, number>

/** FNV-1a 32비트 해시 */
function hash(s: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return h >>> 0
}

/**
 * 새 그룹의 색: 아직 안 쓰인 색 가운데 이름으로 하나를 고른다(다 쓰였으면 팔레트 전체에서).
 * 이름마다 흩어지지만 같은 상황이면 같은 결과라, 입력 중 미리보기 색과 저장되는 색이 같다.
 */
export function pickHue(name: string, used: Iterable<number>): number {
  const taken = new Set(used)
  const free = GROUP_HUES.filter((h) => !taken.has(h))
  const pool = free.length > 0 ? free : GROUP_HUES
  return pool[hash(name) % pool.length]!
}

/** 그룹의 색: 저장된 색이 있으면 그것, 없으면 지금 만든다면 받을 색 */
export function groupHue(name: string, colors: GroupColors | undefined): number {
  return colors?.[name] ?? pickHue(name, Object.values(colors ?? {}))
}

/**
 * 지금 있는 그룹 목록에 맞춰 색 표를 고친다: 색이 없는 그룹에 새 색을 주고, 없어진 그룹의 색은 지운다.
 * 바뀐 게 없으면 null.
 */
export function syncGroupColors(groups: string[], colors: GroupColors | undefined): GroupColors | null {
  const want = new Set(groups)
  const next: GroupColors = {}
  let changed = false
  for (const [g, h] of Object.entries(colors ?? {})) {
    if (want.has(g)) next[g] = h
    else changed = true
  }
  for (const g of [...want].sort()) {
    if (next[g] !== undefined) continue
    next[g] = pickHue(g, Object.values(next))
    changed = true
  }
  return changed ? next : null
}
