import type { FileHistoryEntry } from '@shared/types'
import { FIELD_SEP } from './log'

const FIELDS = ['%H', '%P', '%an', '%ae', '%at', '%s'] as const

/**
 * `git log --follow` 에 붙일 출력 형식 인자. `--name-status -z` 와 함께 쓰면 커밋마다
 * `\x01<필드들>\0\n<상태>\0<경로>[\0<새 경로>]\0` 가 나온다(이름 변경 추적 결과가 경로에 담긴다).
 */
export const FILE_HISTORY_FORMAT_ARGS = ['-z', '--name-status', '-M', `--format=%x01${FIELDS.join('%x1f')}`]

/** 커밋 레코드 시작 표지(SOH). 제목 등에 섞일 일은 거의 없지만 해시 모양이 뒤따를 때만 경계로 본다 */
const RECORD_START =/\x01(?=[0-9a-f]{40,64}\x1f)/

/**
 * `git log --follow -z --name-status` 출력을 커밋 목록으로 바꾼다.
 * fallbackPath: name-status 가 없는 레코드(예: 상태 줄이 생략된 병합)에 쓸 요청 경로.
 */
export function parseFileHistory(buf: Buffer, fallbackPath: string): FileHistoryEntry[] {
  const out: FileHistoryEntry[] = []
  for (const chunk of buf.toString('utf8').split(RECORD_START)) {
    if (!chunk) continue
    const nul = chunk.indexOf('\0')
    const head = nul < 0 ? chunk : chunk.slice(0, nul)
    const f = head.split(FIELD_SEP)
    if (f.length < FIELDS.length) continue
    const [hash, parents, authorName, authorEmail, at] = f as [string, string, string, string, string]
    const subject = f.slice(FIELDS.length - 1).join(FIELD_SEP)
    const rest = nul < 0 ? '' : chunk.slice(nul + 1).replace(/^\n+/, '')
    const parts = rest.split('\0')
    const status = parts[0]?.[0] ?? 'M'
    const renamed = status === 'R' || status === 'C'
    const path = (renamed ? parts[2] : parts[1]) || fallbackPath
    const oldPath = renamed ? parts[1] : undefined
    out.push({
      hash,
      parents: parents ? parents.split(' ') : [],
      authorName,
      authorEmail,
      authorDate: Number(at) || 0,
      subject,
      status,
      path,
      ...(oldPath ? { oldPath } : {})
    })
  }
  return out
}
