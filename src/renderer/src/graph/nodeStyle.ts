// 그래프 노드(아바타) 표시용 순수 함수: 작성자 이니셜과 레인 색 위의 글자색.

/** 한글·한자·가나는 글자 하나가 이름의 첫 단위라 첫 글자만 쓴다 */
const CJK_RE = /^[\p{Script=Hangul}\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]/u
const ALNUM_RE = /^[\p{L}\p{N}]/u
const SPLIT_RE = /[\s._\-/\\<>()[\]]+/

/**
 * 작성자 이름의 이니셜(1~2글자). CJK 이름은 첫 글자(한글은 첫 음절), 그 밖에는 앞 두 단어의 머리글자를 대문자로.
 * 이름이 비었거나 글자가 없으면 '?'. 서로게이트 쌍(이모지 등)도 한 글자로 센다.
 */
export function authorInitials(name: string): string {
  const parts = name
    .trim()
    .split(SPLIT_RE)
    // 단어 앞의 기호(따옴표·#·@ 등)는 건너뛰고 첫 글자·숫자를 찾는다
    .map((p) => p.replace(/^[^\p{L}\p{N}]+/u, ''))
    .filter(Boolean)
  const first = parts[0]
  if (!first) return '?'
  if (CJK_RE.test(first)) return Array.from(first)[0]!
  const a = Array.from(first)[0]!
  const b = parts[1] && ALNUM_RE.test(parts[1]) ? Array.from(parts[1])[0]! : ''
  return (a + b).toUpperCase()
}

function parseHex(hex: string): [number, number, number] | null {
  const m = /^#([0-9a-f]{6})/i.exec(hex.trim())
  if (!m) return null
  const n = parseInt(m[1]!, 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

const toLinear = (v: number): number => {
  const s = v / 255
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
}

/** WCAG 상대 휘도 */
export function luminance([r, g, b]: [number, number, number]): number {
  return 0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b)
}

export function contrastRatio(a: [number, number, number], b: [number, number, number]): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number]
  return (hi + 0.05) / (lo + 0.05)
}

const hex2 = (n: number): string => Math.round(n).toString(16).padStart(2, '0')
const toHex = ([r, g, b]: [number, number, number]): string => `#${hex2(r)}${hex2(g)}${hex2(b)}`

const DARK: [number, number, number] = [0x12, 0x0f, 0x0d]
const WHITE: [number, number, number] = [255, 255, 255]

export interface NodeColors {
  /** 노드 원 채움색 */
  fill: string
  /** 이니셜 글자색 */
  text: string
}

/**
 * 레인 색(#rrggbb) 위에 올릴 글자색을 고른다. 어두운 글자·흰 글자 중 대비가 큰 쪽을 쓰되,
 * 둘 다 AA(4.5) 미만이면 원의 채움색을 글자 반대 방향으로 조금씩 밀어 4.5 를 맞춘다(색상은 유지).
 * 해석할 수 없는 색이면 입력을 그대로 두고 흰 글자를 쓴다.
 */
export function nodeColors(laneHex: string, min = 4.5): NodeColors {
  const rgb = parseHex(laneHex)
  if (!rgb) return { fill: laneHex, text: '#ffffff' }
  const onDark = contrastRatio(rgb, DARK)
  const onWhite = contrastRatio(rgb, WHITE)
  const textRgb = onDark >= onWhite ? DARK : WHITE
  if (Math.max(onDark, onWhite) >= min) return { fill: laneHex, text: toHex(textRgb) }
  // 글자가 어두우면 채움을 밝게, 글자가 희면 채움을 어둡게
  const target = textRgb === DARK ? WHITE : DARK
  for (let k = 1; k <= 20; k++) {
    const f = k / 20
    const mixed: [number, number, number] = [
      rgb[0] + (target[0] - rgb[0]) * f,
      rgb[1] + (target[1] - rgb[1]) * f,
      rgb[2] + (target[2] - rgb[2]) * f
    ]
    if (contrastRatio(mixed, textRgb) >= min) return { fill: toHex(mixed), text: toHex(textRgb) }
  }
  return { fill: laneHex, text: toHex(textRgb) }
}
