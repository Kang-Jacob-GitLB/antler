import type { Stash } from '@shared/types'
import { FIELD_SEP } from './log'
import { splitNul } from './nulRecords'

/** `git stash list` 인자. %gd 는 stash@{n}, %gs 는 reflog 제목("On main: 메시지") */
export const STASH_ARGS = ['stash', 'list', '-z', '--format=%H%x1f%gd%x1f%ct%x1f%gs']

export function parseStashes(buf: Buffer): Stash[] {
  const out: Stash[] = []
  for (const rec of splitNul(buf)) {
    const f = rec.replace(/^\n+/, '').split(FIELD_SEP)
    if (f.length < 4) continue
    const [hash, ref, ct] = f as [string, string, string]
    const m = /^stash@\{(\d+)\}$/.exec(ref)
    if (!m) continue
    out.push({ index: Number(m[1]), ref, hash, date: Number(ct), message: f.slice(3).join(FIELD_SEP) })
  }
  return out
}
