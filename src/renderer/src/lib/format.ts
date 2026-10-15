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

/** 일주일 이내는 상대 시간, 그 이전은 절대 시각 */
export function formatDate(epochSec: number, locale: Locale, now = Date.now()): string {
  const diff = (epochSec * 1000 - now) / 1000
  const abs = Math.abs(diff)
  if (abs < 60) return rtf(locale).format(Math.round(diff), 'second')
  if (abs < 3600) return rtf(locale).format(Math.round(diff / 60), 'minute')
  if (abs < 86400) return rtf(locale).format(Math.round(diff / 3600), 'hour')
  if (abs < 7 * 86400) return rtf(locale).format(Math.round(diff / 86400), 'day')
  return dtf(locale).format(epochSec * 1000)
}

export function formatAbsolute(epochSec: number, locale: Locale): string {
  return dtf(locale).format(epochSec * 1000)
}

/** 아바타 이니셜: 한글 이름은 첫 글자, 영문은 단어 머리글자 2개 */
export function initials(name: string): string {
  // 그래프 노드와 같은 규칙을 쓴다
  return authorInitials(name)
}
