import type { BlameCommit, BlameGroup } from '@shared/types'

export interface ParsedBlame {
  commits: Record<string, BlameCommit>
  groups: BlameGroup[]
  lines: string[]
  /** 줄 안에 NUL 이 있었는지(바이너리) */
  binary: boolean
}

const HEADER_RE = /^([0-9a-f]{40,64}) \d+ \d+(?: \d+)?$/

/**
 * `git blame --porcelain` 출력 파서.
 * 줄마다 `<해시> <원래 줄> <최종 줄> [<묶음 줄 수>]` 머리 → (그 커밋을 처음 만났을 때만) 메타 줄들 → `\t<내용>` 순서다.
 * 같은 커밋이 연속으로 만든 줄은 한 묶음(group)으로 합친다(원래 줄이 떨어져 있어도 최종 줄이 이어지면 같은 묶음).
 */
export function parseBlame(buf: Buffer): ParsedBlame {
  const commits: Record<string, BlameCommit> = {}
  const groups: BlameGroup[] = []
  const lines: string[] = []
  let binary = false
  let cur: BlameCommit | null = null
  let awaiting: string | null = null

  for (const line of buf.toString('utf8').split('\n')) {
    if (awaiting === null) {
      const m = HEADER_RE.exec(line)
      if (!m) continue
      awaiting = m[1]!
      cur = commits[awaiting] ?? null
      if (!cur) {
        cur = { hash: awaiting, authorName: '', authorEmail: '', authorDate: 0, summary: '', boundary: false }
        commits[awaiting] = cur
      }
      continue
    }
    if (line.startsWith('\t')) {
      const text = line.slice(1)
      if (text.includes('\0')) binary = true
      const last = groups[groups.length - 1]
      if (last && last.hash === awaiting) last.count++
      else groups.push({ hash: awaiting, start: lines.length, count: 1 })
      lines.push(text)
      awaiting = null
      cur = null
      continue
    }
    if (!cur) continue
    const sp = line.indexOf(' ')
    const key = sp < 0 ? line : line.slice(0, sp)
    const value = sp < 0 ? '' : line.slice(sp + 1)
    if (key === 'author') cur.authorName = value
    else if (key === 'author-mail') cur.authorEmail = value.replace(/^<|>$/g, '')
    else if (key === 'author-time') cur.authorDate = Number(value) || 0
    else if (key === 'summary') cur.summary = value
    else if (key === 'boundary') cur.boundary = true
  }
  return { commits, groups, lines, binary }
}
