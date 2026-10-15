import type { FileChange, FileStatus } from '@shared/types'

interface StatusEntry {
  status: FileStatus
  path: string
  oldPath?: string
  similarity?: number
}

interface NumstatEntry {
  path: string
  additions: number | null
  deletions: number | null
  binary: boolean
}

const KNOWN_STATUS = new Set<string>(['A', 'M', 'D', 'R', 'C', 'T', 'U'])

/** git 이 알려 주는 알 수 없는 상태 문자(X, B 등)는 수정으로 취급한다 */
function toStatus(letter: string): FileStatus {
  return KNOWN_STATUS.has(letter) ? (letter as FileStatus) : 'M'
}

function splitNulTokens(buf: Buffer): string[] {
  const s = buf.toString('utf8').split('\0')
  // -z 출력은 NUL 로 끝나므로 마지막 빈 조각을 버린다
  if (s.length && s[s.length - 1] === '') s.pop()
  return s
}

/**
 * `--name-status -z` 출력. "M\0path\0", "R100\0old\0new\0", "C075\0old\0new\0".
 */
export function parseNameStatus(buf: Buffer): StatusEntry[] {
  const t = splitNulTokens(buf)
  const out: StatusEntry[] = []
  for (let i = 0; i < t.length; ) {
    const code = t[i++]!
    const status = toStatus(code.charAt(0))
    if (status === 'R' || status === 'C') {
      const oldPath = t[i++]
      const path = t[i++]
      if (oldPath === undefined || path === undefined) break
      const sim = code.length > 1 ? Number(code.slice(1)) : NaN
      out.push({ status, path, oldPath, ...(Number.isNaN(sim) ? {} : { similarity: sim }) })
    } else {
      const path = t[i++]
      if (path === undefined) break
      out.push({ status, path })
    }
  }
  return out
}

/**
 * `--numstat -z` 출력. "1\t2\tpath\0", 이름 변경은 "1\t2\t\0old\0new\0", 바이너리는 "-\t-\t".
 */
export function parseNumstat(buf: Buffer): NumstatEntry[] {
  const t = splitNulTokens(buf)
  const out: NumstatEntry[] = []
  for (let i = 0; i < t.length; ) {
    const tok = t[i++]!
    const a = tok.indexOf('\t')
    const b = a < 0 ? -1 : tok.indexOf('\t', a + 1)
    if (b < 0) continue
    const add = tok.slice(0, a)
    const del = tok.slice(a + 1, b)
    let path = tok.slice(b + 1)
    if (path === '') {
      // 이름 변경·복사: 다음 두 토큰이 old, new
      i++
      path = t[i++] ?? ''
      if (path === '') continue
    }
    const binary = add === '-' && del === '-'
    out.push({ path, additions: binary ? null : Number(add), deletions: binary ? null : Number(del), binary })
  }
  return out
}

/** name-status 순서를 유지하며 numstat 통계를 변경 후 경로로 합친다. */
export function parseChanges(nameStatus: Buffer, numstat: Buffer): FileChange[] {
  const stats = new Map<string, NumstatEntry>()
  for (const e of parseNumstat(numstat)) stats.set(e.path, e)
  return parseNameStatus(nameStatus).map((e) => {
    const s = stats.get(e.path)
    return {
      status: e.status,
      path: e.path,
      ...(e.oldPath !== undefined ? { oldPath: e.oldPath } : {}),
      ...(e.similarity !== undefined ? { similarity: e.similarity } : {}),
      additions: s ? s.additions : null,
      deletions: s ? s.deletions : null,
      binary: s?.binary ?? false
    }
  })
}
