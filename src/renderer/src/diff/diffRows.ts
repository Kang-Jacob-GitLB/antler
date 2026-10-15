// diff hunk → 화면 행 변환. DOM 에 의존하지 않는 순수 모듈(테스트 대상).
//
// unified: hunk 머리 행 + 줄 행
// split:   hunk 머리 행 + (왼쪽=원래, 오른쪽=새) 짝 행. 연속된 삭제 묶음과 추가 묶음을 순서대로 짝짓고
//          남는 쪽은 반대편을 비운다.
// 짝지어진 삭제/추가 줄에는 공통 앞뒤를 뺀 "바뀐 구간"을 표시해 줄 안 변경을 강조한다.
import type { DiffHunk, DiffLine } from '@shared/types'

/** 줄 안에서 바뀐 구간 [start, end) — 문자 인덱스 */
export type Span = [start: number, end: number]

export interface UnifiedLineRow {
  kind: 'line'
  line: DiffLine
  span?: Span
}
export interface HunkRow {
  kind: 'hunk'
  hunk: DiffHunk
  index: number
}
export type UnifiedRow = UnifiedLineRow | HunkRow

export interface SplitCell {
  line: DiffLine
  span?: Span
}
export interface SplitLineRow {
  kind: 'pair'
  left?: SplitCell
  right?: SplitCell
}
export type SplitRow = SplitLineRow | HunkRow

/** 바뀐 비율이 이보다 크면 줄 전체가 바뀐 것으로 보고 구간 강조를 하지 않는다 */
const MAX_CHANGED_RATIO = 0.75

/**
 * 두 줄의 공통 접두·접미를 빼고 남는 바뀐 구간을 구한다.
 * 서로게이트 쌍을 가르지 않도록 코드 포인트 경계에서 멈춘다.
 */
export function changedSpans(a: string, b: string): [Span, Span] | null {
  if (a === b) return null
  const max = Math.min(a.length, b.length)
  let pre = 0
  while (pre < max && a.charCodeAt(pre) === b.charCodeAt(pre)) pre++
  // 서로게이트 쌍 중간에서 멈췄으면 한 칸 물린다
  if (pre > 0 && isHighSurrogate(a.charCodeAt(pre - 1))) pre--
  let suf = 0
  while (suf < max - pre && a.charCodeAt(a.length - 1 - suf) === b.charCodeAt(b.length - 1 - suf)) suf++
  if (suf > 0 && isLowSurrogate(a.charCodeAt(a.length - suf))) suf--
  const sa: Span = [pre, a.length - suf]
  const sb: Span = [pre, b.length - suf]
  const longest = Math.max(a.length, b.length)
  const changed = Math.max(sa[1] - sa[0], sb[1] - sb[0])
  if (longest === 0 || changed / longest > MAX_CHANGED_RATIO) return null
  return [sa, sb]
}

const isHighSurrogate = (c: number): boolean => c >= 0xd800 && c <= 0xdbff
const isLowSurrogate = (c: number): boolean => c >= 0xdc00 && c <= 0xdfff

/** 연속된 삭제 묶음 + 추가 묶음을 찾아 콜백한다. 나머지 줄은 단독으로 콜백한다 */
function walk(
  lines: DiffLine[],
  onSingle: (l: DiffLine) => void,
  onBlock: (dels: DiffLine[], adds: DiffLine[]) => void
): void {
  let i = 0
  while (i < lines.length) {
    const l = lines[i]!
    if (l.t !== '-' && l.t !== '+') {
      onSingle(l)
      i++
      continue
    }
    const dels: DiffLine[] = []
    const adds: DiffLine[] = []
    while (i < lines.length && lines[i]!.t === '-') dels.push(lines[i++]!)
    // 삭제 묶음 끝의 "No newline" 표시는 삭제 쪽에 붙인다
    while (i < lines.length && lines[i]!.t === '\\' && adds.length === 0) dels.push(lines[i++]!)
    while (i < lines.length && lines[i]!.t === '+') adds.push(lines[i++]!)
    while (i < lines.length && lines[i]!.t === '\\') adds.push(lines[i++]!)
    onBlock(dels, adds)
  }
}

/** 짝지을 실제 내용 줄만(‘\’ 표시 줄 제외) */
const content = (ls: DiffLine[]): DiffLine[] => ls.filter((l) => l.t !== '\\')

function spansFor(dels: DiffLine[], adds: DiffLine[]): Map<DiffLine, Span> {
  const map = new Map<DiffLine, Span>()
  const d = content(dels)
  const a = content(adds)
  const n = Math.min(d.length, a.length)
  for (let k = 0; k < n; k++) {
    const s = changedSpans(d[k]!.text, a[k]!.text)
    if (s) {
      map.set(d[k]!, s[0])
      map.set(a[k]!, s[1])
    }
  }
  return map
}

export function unifiedRows(hunks: DiffHunk[]): UnifiedRow[] {
  const rows: UnifiedRow[] = []
  hunks.forEach((hunk, index) => {
    rows.push({ kind: 'hunk', hunk, index })
    walk(
      hunk.lines,
      (line) => rows.push({ kind: 'line', line }),
      (dels, adds) => {
        const spans = spansFor(dels, adds)
        for (const line of [...dels, ...adds]) {
          const span = spans.get(line)
          rows.push(span ? { kind: 'line', line, span } : { kind: 'line', line })
        }
      }
    )
  })
  return rows
}

export function splitRows(hunks: DiffHunk[]): SplitRow[] {
  const rows: SplitRow[] = []
  hunks.forEach((hunk, index) => {
    rows.push({ kind: 'hunk', hunk, index })
    walk(
      hunk.lines,
      (line) => {
        // 문맥 줄은 양쪽에, 홀로 남은 "No newline" 은 양쪽에 같은 위치로
        rows.push({ kind: 'pair', left: { line }, right: { line } })
      },
      (dels, adds) => {
        const spans = spansFor(dels, adds)
        const n = Math.max(dels.length, adds.length)
        for (let k = 0; k < n; k++) {
          const l = dels[k]
          const r = adds[k]
          const row: SplitLineRow = { kind: 'pair' }
          if (l) row.left = spans.has(l) ? { line: l, span: spans.get(l)! } : { line: l }
          if (r) row.right = spans.has(r) ? { line: r, span: spans.get(r)! } : { line: r }
          rows.push(row)
        }
      }
    )
  })
  return rows
}
