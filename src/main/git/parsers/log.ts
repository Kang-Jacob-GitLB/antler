import type { Commit, RefLabel } from '@shared/types'
import { splitNul } from './nulRecords'

/** 필드 구분자 US(0x1f). 커밋 사이는 `-z` 의 NUL 로 구분된다. */
export const FIELD_SEP = '\x1f'

const FIELDS = ['%H', '%P', '%an', '%ae', '%at', '%ct', '%D', '%s'] as const
export const LOG_FORMAT = FIELDS.join('%x1f')

/** `git log` 에 붙일 출력 형식 인자 */
export const LOG_FORMAT_ARGS = ['-z', `--format=${LOG_FORMAT}`, '--decorate=full']

/**
 * `%D` + `--decorate=full` 출력을 ref 라벨로 변환한다.
 * 예: "HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1.0"
 */
export function parseDecorations(raw: string): RefLabel[] {
  if (!raw.trim()) return []
  const refs: RefLabel[] = []
  for (const part of raw.split(', ')) {
    const item = part.trim()
    if (!item) continue
    if (item === 'HEAD') {
      refs.push({ kind: 'head', name: 'HEAD' })
    } else if (item.startsWith('HEAD -> ')) {
      // head 라벨의 name 은 HEAD 가 가리키는 브랜치 이름(분리된 HEAD 면 'HEAD')
      const target = item.slice('HEAD -> '.length)
      refs.push({ kind: 'head', name: target.replace(/^refs\/heads\//, '') })
      pushRef(refs, target)
    } else if (item.startsWith('tag: ')) {
      pushRef(refs, item.slice('tag: '.length))
    } else {
      pushRef(refs, item)
    }
  }
  return refs
}

function pushRef(refs: RefLabel[], full: string): void {
  if (full.startsWith('refs/heads/')) refs.push({ kind: 'local', name: full.slice(11) })
  else if (full.startsWith('refs/remotes/')) {
    const name = full.slice(13)
    // origin/HEAD 같은 심볼릭 리모트 HEAD 는 라벨로 쓰지 않는다
    if (!name.endsWith('/HEAD')) refs.push({ kind: 'remote', name })
  } else if (full.startsWith('refs/tags/')) refs.push({ kind: 'tag', name: full.slice(10) })
  // refs/stash 등 나머지는 그래프 라벨로 쓰지 않는다
}

export function parseLogRecord(record: string): Commit | null {
  // 레코드 앞에 남은 개행(일부 git 버전의 구분자 차이)을 걷어낸다
  const rec = record.replace(/^\n+/, '')
  if (!rec) return null
  const f = rec.split(FIELD_SEP)
  if (f.length < FIELDS.length) return null
  const [hash, parents, authorName, authorEmail, at, ct, deco] = f as [string, string, string, string, string, string, string]
  // 제목에 US 문자가 들어간 극단적 경우를 대비해 나머지를 합친다
  const subject = f.slice(FIELDS.length - 1).join(FIELD_SEP)
  return {
    hash,
    parents: parents ? parents.split(' ') : [],
    authorName,
    authorEmail,
    authorDate: Number(at),
    commitDate: Number(ct),
    subject,
    refs: parseDecorations(deco)
  }
}

export function parseLog(buf: Buffer): Commit[] {
  const out: Commit[] = []
  for (const rec of splitNul(buf)) {
    const c = parseLogRecord(rec)
    if (c) out.push(c)
  }
  return out
}
