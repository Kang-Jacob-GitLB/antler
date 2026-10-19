import type { Locale } from '@shared/types'
import { authorInitials } from '../graph/nodeStyle'

const rtfCache = new Map<Locale, Intl.RelativeTimeFormat>()
const dtfCache = new Map<Locale, Intl.DateTimeFormat>()

function rtf(locale: Locale): Intl.RelativeTimeFormat {
  let f = rtfCache.get(locale)
  if (!f) rtfCache.set(locale, (f = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' })))
  return f
}

function dtf(locale: Locale): Intl.DateTimeFormat {
  let f = dtfCache.get(locale)
  if (!f) {
    f = new Intl.DateTimeFormat(locale, {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false
    })
    dtfCache.set(locale, f)
  }
  return f
}

/** 상대 시간으로 보여 줄 범위(초). 이보다 오래되면 날짜와 시각으로 */
const RELATIVE_MAX_SEC = 3 * 3600

/** 3시간 이내는 상대 시간(방금·n분 전·n시간 전), 그보다 오래되면 날짜와 시각 */
export function formatDate(epochSec: number, locale: Locale, now = Date.now()): string {
  const diff = (epochSec * 1000 - now) / 1000
  const abs = Math.abs(diff)
  if (abs >= RELATIVE_MAX_SEC) return dtf(locale).format(epochSec * 1000)
  if (abs < 60) return rtf(locale).format(Math.round(diff), 'second')
  if (abs < 3600) return rtf(locale).format(Math.round(diff / 60), 'minute')
  return rtf(locale).format(Math.round(diff / 3600), 'hour')
}

export function formatAbsolute(epochSec: number, locale: Locale): string {
  return dtf(locale).format(epochSec * 1000)
}

/** 아바타 이니셜: 한글 이름은 첫 글자, 영문은 단어 머리글자 2개 */
export function initials(name: string): string {
  // 그래프 노드와 같은 규칙을 쓴다
  return authorInitials(name)
}

/** 바이트 수를 B·KB·MB·GB 로 줄여 보여 준다(1024 단위, 소수 첫째 자리) */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '?'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = bytes / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(1)} ${units[i]}`
}
