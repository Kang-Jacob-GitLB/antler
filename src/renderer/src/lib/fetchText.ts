import type { FetchStatus } from '@shared/types'
import type { MessageKey } from '../i18n'

type T = (key: MessageKey, vars?: Record<string, string | number>) => string

/** 경과 시간을 "방금 / n분 전 / n시간 전" 으로 */
export function agoText(t: T, ms: number): string {
  const min = Math.floor(Math.max(0, ms) / 60_000)
  if (min < 1) return t('fetch.agoNow')
  return min < 60 ? t('fetch.agoMin', { n: min }) : t('fetch.agoHour', { n: Math.floor(min / 60) })
}

/** fetch 상태 한 줄 설명(새로고침 버튼 툴팁·상태 표시줄 공용) */
export function fetchDetail(t: T, s: FetchStatus | undefined, now: number): string {
  if (!s) return ''
  if (s.fetching) return t('fetch.fetching')
  if (s.blocked) return t(s.blocked.confirmable ? 'fetch.blocked' : 'fetch.blockedHard', { keys: s.blocked.keys.join(', ') })
  if (s.error) return t('fetch.failed', { msg: s.error }) + (s.needsManual ? `\n${t('fetch.needsManual')}` : '')
  if (s.authPaused) return t('fetch.authPaused')
  if (s.noRemote) return t('fetch.noRemote')
  if (s.lastOkAt) return t('fetch.last', { ago: agoText(t, now - s.lastOkAt) })
  return t('fetch.never')
}
