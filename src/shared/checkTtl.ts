// 커밋 체크 상태의 수명과 캐시 키. main 캐시와 renderer 상태가 같은 값을 쓴다.
import type { GhCheckState, GhRepoRef } from './types'

/** 끝난 상태(성공·실패·중립)는 오래, 진행 중은 짧게, 체크 없음(아직 push 안 된 커밋 포함)은 중간 */
export const TTL_DONE_MS = 30 * 60_000
export const TTL_PENDING_MS = 30_000
export const TTL_NONE_MS = 2 * 60_000

export const ttlOf = (s: GhCheckState): number => (s === 'pending' ? TTL_PENDING_MS : s === 'none' ? TTL_NONE_MS : TTL_DONE_MS)

/** 받은 시각(at, epoch ms)에서 이 상태가 아직 신선한지(다시 묻지 않아도 되는지) */
export const isFresh = (e: { state: GhCheckState; at: number } | undefined, now = Date.now()): boolean => !!e && now - e.at < ttlOf(e.state)

/** GitHub 저장소 식별자(owner/repo, 소문자). 같은 저장소의 다른 clone 은 같은 데이터를 공유한다 */
export const scopeOf = (ref: Pick<GhRepoRef, 'owner' | 'repo'> | null | undefined): string | null => (ref ? `${ref.owner}/${ref.repo}`.toLowerCase() : null)
export const checkKey = (scope: string, sha: string): string => `${scope}|${sha.toLowerCase()}`
