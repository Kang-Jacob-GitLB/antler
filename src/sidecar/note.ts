// Rust 가 보낸 `note` 메시지를 RepoService 에 옮긴다. 프로토콜은 protocol.ts.
import { NOTE_OPS, type NoteOp } from './protocol'

/** 처리에 필요한 것만 — 테스트가 RepoService 전체를 만들지 않아도 된다 */
export interface NoteTarget {
  noteLog(op: NoteOp, repoId: string): void
}

/** 허용 목록의 op 와 문자열 repoId 만 통과시킨다(모양이 틀린 메시지는 버린다). 처리했으면 true */
export function handleNote(target: NoteTarget, msg: { op?: unknown; repoId?: unknown }): boolean {
  const { op, repoId } = msg
  if (typeof repoId !== 'string' || !NOTE_OPS.includes(op as NoteOp)) return false
  target.noteLog(op as NoteOp, repoId)
  return true
}
