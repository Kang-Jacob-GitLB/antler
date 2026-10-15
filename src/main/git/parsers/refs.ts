import type { Branch, RemoteGroup, Tag } from '@shared/types'

/** `git for-each-ref` 에 붙일 출력 형식. 필드는 NUL(%00), ref 하나는 한 줄 */
export const REFS_FORMAT =
  '%(refname)%00%(objectname)%00%(*objectname)%00%(HEAD)%00%(upstream:short)%00%(upstream:track,nobracket)' +
  '%00%(symref)%00%(objecttype)%00%(*objecttype)'

/** `git for-each-ref` 전체 인자 */
export const REFS_ARGS = ['for-each-ref', `--format=${REFS_FORMAT}`, 'refs/heads', 'refs/remotes', 'refs/tags']

export interface ParsedRefs {
  local: Branch[]
  remotes: RemoteGroup[]
  tags: Tag[]
}

const naturalCompare = (a: string, b: string): number => a.localeCompare(b, undefined, { numeric: true })

/** `%(upstream:track,nobracket)` 출력("ahead 1, behind 2" / "ahead 3" / "behind 1" / "gone") 해석 */
export function parseTrack(track: string): { ahead?: number; behind?: number; gone?: boolean } {
  const t = track.trim()
  if (!t) return {}
  if (t === 'gone') return { gone: true }
  const out: { ahead?: number; behind?: number } = {}
  const ahead = /ahead (\d+)/.exec(t)
  const behind = /behind (\d+)/.exec(t)
  if (ahead) out.ahead = Number(ahead[1])
  if (behind) out.behind = Number(behind[1])
  return out
}

/** `git remote` 출력(줄 단위)에서 리모트 이름 목록을 뽑는다. */
export function parseRemoteNames(buf: Buffer | string): string[] {
  return buf
    .toString('utf8')
    .split('\n')
    .map((s) => s.replace(/\r$/, ''))
    .filter((s) => s.length > 0)
}

/**
 * for-each-ref 출력을 로컬 브랜치·리모트 그룹·태그로 나눈다.
 * 리모트 이름에 '/' 가 들어갈 수 있어(team/upstream) 이름 목록과 가장 긴 접두 일치로 그룹을 정한다.
 * 목록에 없는 리모트(방금 지워진 경우 등)는 첫 '/' 앞을 이름으로 본다.
 */
export function parseRefs(buf: Buffer | string, remoteNames: string[]): ParsedRefs {
  const local: Branch[] = []
  const tags: Tag[] = []
  const groups = new Map<string, Branch[]>()
  // 긴 이름부터 비교해 'team' 과 'team/upstream' 이 같이 있어도 올바르게 가른다
  const names = [...remoteNames].sort((a, b) => b.length - a.length)

  for (const line of buf.toString('utf8').split('\n')) {
    if (!line) continue
    const f = line.split('\0')
    if (f.length < 4) continue
    const [fullName, hash, peeled, headMark, upstream = '', track = '', symref = '', type = '', peeledType = ''] = f as [
      string,
      string,
      string,
      string,
      string?,
      string?,
      string?,
      string?,
      string?
    ]

    if (fullName.startsWith('refs/heads/')) {
      const b: Branch = { name: fullName.slice(11), fullName, hash, isHead: headMark === '*' }
      if (upstream) {
        b.upstream = upstream
        const t = parseTrack(track)
        if (t.gone) b.upstreamGone = true
        if (t.ahead !== undefined) b.ahead = t.ahead
        if (t.behind !== undefined) b.behind = t.behind
      }
      local.push(b)
    } else if (fullName.startsWith('refs/remotes/')) {
      const short = fullName.slice(13)
      // origin/HEAD 같은 심볼릭 ref 는 목록에서 뺀다(이름이 우연히 x/HEAD 인 실제 브랜치는 남긴다)
      if (symref) continue
      const slash = short.indexOf('/')
      const remote = names.find((n) => short.startsWith(`${n}/`)) ?? (slash === -1 ? short : short.slice(0, slash))
      let list = groups.get(remote)
      if (!list) groups.set(remote, (list = []))
      list.push({ name: short, fullName, hash, isHead: false })
    } else if (fullName.startsWith('refs/tags/')) {
      // annotated 태그는 %(*objectname) 이 가리키는 커밋 해시를 쓴다
      // 트리·블롭을 가리키거나(v2.6.11-tree) 태그를 가리키는 태그는 그래프에서 찾아갈 커밋이 없다
      const targetsCommit = peeled ? peeledType === 'commit' : type === 'commit'
      tags.push({ name: fullName.slice(10), fullName, hash: peeled || hash, annotated: peeled !== '', targetsCommit })
    }
  }

  const byName = (a: { name: string }, b: { name: string }) => naturalCompare(a.name, b.name)
  local.sort(byName)
  tags.sort(byName)
  const remotes: RemoteGroup[] = [...groups.entries()]
    .map(([name, branches]) => ({ name, branches: branches.sort(byName) }))
    .sort(byName)
  return { local, remotes, tags }
}
