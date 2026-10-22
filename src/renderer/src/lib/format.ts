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
export const RELATIVE_MAX_SEC = 3 * 3600

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

const YMD_RE = /^(\d{4})-(\d{2})-(\d{2})$/

/** 'YYYY-MM-DD' → 로컬 달력 날짜(UTC 로 해석하면 음수 시간대에서 하루 밀린다). 형식이 다르거나 없는 날짜면 null */
function parseYmd(ymd: string): Date | null {
  const m = YMD_RE.exec(ymd)
  if (!m) return null
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])]
  const date = new Date(y, mo - 1, d)
  return date.getFullYear() === y && date.getMonth() === mo - 1 && date.getDate() === d ? date : null
}

/** 시간대 없는 달력 날짜('YYYY-MM-DD')를 로케일 형식으로(ko: 2026. 10. 22., en: Oct 22, 2026). 해석할 수 없으면 빈 문자열 */
export function formatCalendarDate(ymd: string, locale: Locale): string {
  const date = parseYmd(ymd)
  // formatAbsolute 의 날짜 부분과 같은 모양(2026. 10. 01. / 10/01/2026)이라 만든 날짜·수정한 날짜 줄과 나란히 맞는다
  return date ? new Intl.DateTimeFormat(locale, { year: 'numeric', month: '2-digit', day: '2-digit' }).format(date) : ''
}

/** 달력 날짜가 로컬 오늘보다 이전인가(오늘 당일은 지난 것이 아니다) */
export function isBeforeToday(ymd: string, now = new Date()): boolean {
  const date = parseYmd(ymd)
  return date !== null && date.getTime() < new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
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

/** 체크·잡 소요 시간(초) → 언어 중립 표기: "45s" / "3m 05s" / "1h 02m" */
export function formatDuration(sec: number): string {
  const s = Math.max(0, Math.round(sec))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`
}
