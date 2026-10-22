import { describe, expect, it } from 'vitest'
import { formatAbsolute, formatCalendarDate, formatDate, isBeforeToday } from '../src/renderer/src/lib/format'

const NOW = Date.UTC(2026, 9, 16, 12, 0, 0)
const ago = (sec: number): number => NOW / 1000 - sec

describe('formatDate', () => {
  it('3시간 이내는 상대 시간', () => {
    expect(formatDate(ago(30), 'en', NOW)).toMatch(/sec/)
    expect(formatDate(ago(5 * 60), 'en', NOW)).toMatch(/5 min/)
    expect(formatDate(ago(2 * 3600), 'en', NOW)).toMatch(/2 hr/)
    expect(formatDate(ago(3 * 3600 - 1), 'ko', NOW)).toMatch(/시간 전/)
  })

  it('3시간이 넘으면 날짜와 시각', () => {
    for (const sec of [3 * 3600, 5 * 3600, 2 * 86400, 30 * 86400]) {
      expect(formatDate(ago(sec), 'ko', NOW)).toBe(formatAbsolute(ago(sec), 'ko'))
    }
  })
})

describe('formatCalendarDate / isBeforeToday', () => {
  it('YYYY-MM-DD 를 로컬 날짜로 보여 준다(하루가 밀리지 않는다)', () => {
    expect(formatCalendarDate('2026-10-22', 'en')).toBe('10/22/2026')
    expect(formatCalendarDate('2026-10-22', 'ko')).toBe('2026. 10. 22.')
    expect(formatCalendarDate('2026-01-01', 'ko')).toBe('2026. 01. 01.')
  })
  it('만든 날짜(formatAbsolute)의 날짜 부분과 같은 모양이다', () => {
    const local = new Date(2026, 0, 1, 9, 0, 0).getTime() / 1000
    for (const l of ['ko', 'en'] as const) expect(formatAbsolute(local, l).startsWith(formatCalendarDate('2026-01-01', l))).toBe(true)
  })
  it('없거나 형식이 다르거나 없는 날짜면 빈 문자열', () => {
    for (const v of ['', '2026-10-22T00:00:00Z', '10/22/2026', '2026-02-30', '2026-13-01']) expect(formatCalendarDate(v, 'en')).toBe('')
  })
  it('오늘 당일은 지난 것이 아니고, 어제부터 지났다', () => {
    const now = new Date(2026, 9, 22, 0, 30)
    expect(isBeforeToday('2026-10-22', now)).toBe(false)
    expect(isBeforeToday('2026-10-23', now)).toBe(false)
    expect(isBeforeToday('2026-10-21', now)).toBe(true)
    expect(isBeforeToday('', now)).toBe(false)
  })
})
