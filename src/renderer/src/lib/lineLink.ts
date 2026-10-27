// 코드 뷰어 줄 링크: GitHub permalink · 코드 복사 문자열 · #L10-L20 조각 해석(순수 함수)

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

/**
 * 코드 복사 문자열: `경로:10-20 @ 짧은해시` 머리글 + 코드 펜스(한 줄이면 `경로:10`).
 * lines 는 파일 전체 줄이고, 범위가 파일 끝을 넘으면 끝에서 자른다. 코드에 ``` 가 있으면 바깥 펜스를 더 길게 한다
 */
export function codeSnippet(o: { path: string; rev: string; range: LineRange; lines: string[]; lang?: string | null }): string {
  const start = Math.max(1, o.range.start)
  const end = Math.min(o.range.end, o.lines.length)
  const code = o.lines.slice(start - 1, Math.max(start - 1, end)).join('\n')
  const last = Math.max(start, end)
  const head = `${o.path}:${start === last ? start : `${start}-${last}`} @ ${o.rev.slice(0, 7)}`
  const longest = Math.max(0, ...(code.match(/`+/g) ?? []).map((r) => r.length))
  const fence = '`'.repeat(Math.max(3, longest + 1))
  return `${head}\n${fence}${o.lang ?? ''}\n${code}\n${fence}`
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
