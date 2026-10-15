// 커밋 제목에서 PR 번호를 읽는다(네트워크 없이 제목만 본다).
// 인정하는 형태: 제목 끝의 "(#123)"(squash 머지, 여러 개면 마지막), 제목 처음의 "Merge pull request #123 from …".
// 문장 중간의 "#123"·"issue #5" 처럼 괄호가 없는 언급은 PR 이라고 단정할 수 없어 무시한다.
const MERGE_RE = /^Merge pull request #(\d{1,9})(?=\s|$)/
const SQUASH_RE = /\(#(\d{1,9})\)\s*$/

/** 제목의 PR 번호. 없으면 null */
export function parsePrNumber(subject: string): number | null {
  const m = SQUASH_RE.exec(subject) ?? MERGE_RE.exec(subject)
  if (!m) return null
  const n = Number(m[1])
  return n > 0 ? n : null
}
