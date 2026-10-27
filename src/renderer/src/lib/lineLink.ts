// 코드 뷰어 줄 링크: GitHub permalink · 참조 문자열 · #L10-L20 조각 해석(순수 함수)

/** 1부터 세는 줄 범위(양끝 포함, start <= end) */
export interface LineRange {
  start: number
  end: number
}

/** 두 줄 번호를 작은 쪽이 앞이 되게 정리한다 */
export function normalizeRange(a: number, b: number): LineRange {
  return a <= b ? { start: a, end: b } : { start: b, end: a }
}

export const inRange = (r: LineRange | null, line: number): boolean => !!r && line >= r.start && line <= r.end

/** 경로의 각 세그먼트를 encodeURIComponent 로 인코딩한다('/' 는 유지) */
export function encodePath(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/')
}

/** `L10` 또는 `L10-L20` */
export function lineFragment(r: LineRange): string {
  return r.start === r.end ? `L${r.start}` : `L${r.start}-L${r.end}`
}

/** 전체 커밋 해시를 담은 GitHub blob permalink */
export function githubPermalink(o: { owner: string; repo: string; rev: string; path: string; range: LineRange }): string {
  return `https://github.com/${encodeURIComponent(o.owner)}/${encodeURIComponent(o.repo)}/blob/${o.rev}/${encodePath(o.path)}#${lineFragment(o.range)}`
}

/** `경로:10-20 @ 짧은해시` (한 줄이면 `경로:10`) */
export function lineReference(o: { path: string; rev: string; range: LineRange }): string {
  const r = o.range
  return `${o.path}:${r.start === r.end ? r.start : `${r.start}-${r.end}`} @ ${o.rev.slice(0, 7)}`
}

/** `#L10` · `#L10-L20` · `L20-L10`(뒤집힘) · GitHub 의 `L10C2-L12C5`(열은 무시)를 줄 범위로. 아니면 null */
export function parseLineFragment(hash: string | null | undefined): LineRange | null {
  if (!hash) return null
  const m = /^#?L(\d+)(?:C\d+)?(?:-L?(\d+)(?:C\d+)?)?$/.exec(hash)
  if (!m) return null
  const a = Number(m[1])
  const b = m[2] === undefined ? a : Number(m[2])
  if (!Number.isSafeInteger(a) || !Number.isSafeInteger(b) || a < 1 || b < 1) return null
  return normalizeRange(a, b)
}
