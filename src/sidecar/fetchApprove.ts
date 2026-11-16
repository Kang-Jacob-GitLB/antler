// Rust 가 보낸 `fetchApprove` 메시지를 ConfigStore 에 옮긴다. 프로토콜은 protocol.ts.
//
// 승인(fetchApprovals·trustedConfigs)을 쓰는 길은 이것 하나다. renderer 가 보낸 `req` 는 여기로 오지 않는다(채널 허용 목록에 없다) —
// 메시지의 `t` 는 Rust 가 정하고 renderer 의 값은 `req` 의 `args` 안에만 들어간다. 그래도 사이드카는 모양을 다시 검사한다.
import { FETCH_APPROVE_KINDS, type FetchApproveKind } from './protocol'

/** 처리에 필요한 것만 — 테스트가 ConfigStore 전체를 만들지 않아도 된다 */
export interface ApproveTarget {
  repo(repoId: string): unknown
  setFetchApproval(repoId: string, fingerprint: string | null): void
  setTrustedConfig(repoId: string, fingerprint: string): void
}

/** sha256 지문(소문자 16진수 64자). `$` 는 JS 에서 맨 끝만이라 끝 개행이 붙은 값은 통과하지 못한다 */
const FINGERPRINT = /^[0-9a-f]{64}$/

/** 허용 목록의 kind·존재하는 저장소·지문 모양이 모두 맞을 때만 저장한다(해제(null)는 받지 않는다). 처리했으면 true */
export function handleFetchApprove(target: ApproveTarget, msg: { kind?: unknown; repoId?: unknown; fingerprint?: unknown }): boolean {
  const { kind, repoId, fingerprint } = msg
  if (typeof repoId !== 'string' || typeof fingerprint !== 'string' || !FINGERPRINT.test(fingerprint)) return false
  if (!FETCH_APPROVE_KINDS.includes(kind as FetchApproveKind) || !target.repo(repoId)) return false
  if (kind === 'approval') target.setFetchApproval(repoId, fingerprint)
  else target.setTrustedConfig(repoId, fingerprint)
  return true
}
