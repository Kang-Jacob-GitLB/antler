import type { CommitDetail } from '@shared/types'

/** 필드 구분자는 NUL(%x00) — 본문(%b)에 들어갈 수 없는 유일한 바이트다. 본문은 마지막에 둔다. */
const FIELDS = ['%H', '%P', '%an', '%ae', '%at', '%cn', '%ce', '%ct', '%s', '%b'] as const

/** `git log -1` 에 붙일 출력 형식 인자. `-z` 라 레코드 끝에 NUL 이 하나 더 붙는다. */
export const COMMIT_FORMAT_ARGS = ['-z', '--no-notes', `--format=${FIELDS.join('%x00')}`]

/** 변경 파일 목록(files·통계)은 별도 명령에서 채운다 */
export type CommitMeta = Omit<CommitDetail, 'baseParent' | 'files' | 'additions' | 'deletions' | 'truncated'>

export function parseCommitMeta(buf: Buffer): CommitMeta {
  const f = buf.toString('utf8').split('\0')
  // 본문은 마지막 필드 + 종결 NUL 뒤의 빈 조각이므로 최소 FIELDS.length 개가 있어야 한다
  if (f.length < FIELDS.length) throw new Error('unexpected commit metadata output')
  const [hash, parents, authorName, authorEmail, at, committerName, committerEmail, ct, subject, body] = f as [
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string,
    string
  ]
  return {
    hash,
    parents: parents ? parents.split(' ') : [],
    authorName,
    authorEmail,
    authorDate: Number(at),
    committerName,
    committerEmail,
    commitDate: Number(ct),
    subject,
    body: body.trim()
  }
}
