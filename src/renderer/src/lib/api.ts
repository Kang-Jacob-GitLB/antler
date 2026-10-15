import type { AntlerApi } from '@shared/ipc'

export const api: AntlerApi = window.antler

let seq = 0
export function newRequestId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${(seq++).toString(36)}`
}

/** preload 가 붙인 "[CODE] message" 형식에서 취소 여부를 판별한다 */
export function isAbort(err: unknown): boolean {
  return err instanceof Error && err.message.startsWith('[ABORTED]')
}

/** 페이지를 넘기는 사이 커밋 이력이 바뀌어 처음부터 다시 읽어야 하는 경우 */
export function isStale(err: unknown): boolean {
  return err instanceof Error && err.message.startsWith('[STALE]')
}

export function errorMessage(err: unknown): string {
  const msg = err instanceof Error ? err.message : String(err)
  return msg.replace(/^\[[A-Z]+\]\s*/, '')
}
