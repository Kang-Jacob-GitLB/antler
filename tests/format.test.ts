import { describe, expect, it } from 'vitest'
import { formatAbsolute, formatDate } from '../src/renderer/src/lib/format'

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
