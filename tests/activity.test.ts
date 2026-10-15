import { describe, expect, it } from 'vitest'
import { bucketize, MAX_BUCKETS, TimeStreamParser } from '../src/main/git/activity'
import { authorInitials, contrastRatio, nodeColors } from '../src/renderer/src/graph/nodeStyle'

const DAY = 86400
const NOW = 1_800_000_000

describe('bucketize', () => {
  it('빈 입력은 빈 결과', () => {
    const r = bucketize([], 0, NOW)
    expect(r.buckets).toEqual([])
    expect(r.total).toBe(0)
  })

  it('커밋 하나는 구간 하나', () => {
    const r = bucketize([NOW - 5 * DAY], 1, NOW)
    expect(r.buckets).toHaveLength(1)
    expect(r.buckets[0]!.count).toBe(1)
    expect(r.first).toBe(r.last)
  })

  it('짧은 이력은 일 이하 단위, 구간 합 = total, 구간은 연속', () => {
    const times = [NOW - 100 * DAY, NOW - 100 * DAY + 60, NOW - 50 * DAY, NOW - 1 * DAY]
    const r = bucketize(times, times.length, NOW)
    expect(r.bucketSec).toBeLessThanOrEqual(DAY)
    expect(r.buckets.reduce((s, b) => s + b.count, 0)).toBe(4)
    expect(r.total).toBe(4)
    for (let i = 1; i < r.buckets.length; i++) expect(r.buckets[i]!.t - r.buckets[i - 1]!.t).toBe(r.bucketSec)
    expect(r.buckets[0]!.t).toBeLessThanOrEqual(r.first)
    expect(r.buckets.at(-1)!.t + r.bucketSec).toBeGreaterThan(r.last)
  })

  it('수년 이력은 주 이상 단위로 묶여 구간 수가 상한 이내', () => {
    const times = Array.from({ length: 5000 }, (_, i) => NOW - Math.floor((i / 5000) * 8 * 365 * DAY))
    const r = bucketize(times, times.length, NOW)
    expect(r.bucketSec).toBeGreaterThanOrEqual(7 * DAY)
    expect(r.buckets.length).toBeLessThanOrEqual(MAX_BUCKETS)
    expect(r.buckets.reduce((s, b) => s + b.count, 0)).toBe(5000)
    expect(r.last).toBe(NOW)
  })

  it('0·먼 미래 시각은 범위에서 빼되 total 에는 센다', () => {
    const r = bucketize([0, NOW - DAY, NOW + 400 * DAY], 3, NOW)
    expect(r.total).toBe(3)
    expect(r.buckets.reduce((s, b) => s + b.count, 0)).toBe(1)
    expect(r.first).toBe(NOW - DAY)
  })
})

describe('TimeStreamParser', () => {
  it('청크 경계에 걸친 숫자를 이어 붙인다', () => {
    const p = new TimeStreamParser()
    const enc = (s: string): Uint8Array => Buffer.from(s)
    p.push(enc('1700000'))
    p.push(enc('000\n17000'))
    p.push(enc('00001\r\n1\n'))
    p.push(enc('42'))
    p.end()
    expect([...p.values.slice(0, p.n)]).toEqual([1700000000, 1700000001, 1, 42])
  })

  it('큰 입력에서 버퍼가 늘어난다', () => {
    const p = new TimeStreamParser()
    p.push(Buffer.from(Array.from({ length: 50_000 }, (_, i) => i + 1).join('\n') + '\n'))
    p.end()
    expect(p.n).toBe(50_000)
    expect(p.values[49_999]).toBe(50_000)
  })
})

describe('authorInitials', () => {
  it('한글 이름은 첫 음절', () => expect(authorInitials('강민규')).toBe('강'))
  it('영문 두 단어는 머리글자 2개', () => expect(authorInitials('Jacob Kang')).toBe('JK'))
  it('한 단어는 한 글자', () => expect(authorInitials('octocat')).toBe('O'))
  it('구분자 . _ - 로도 나눈다', () => expect(authorInitials('jacob.kang')).toBe('JK'))
  it('세 단어 이상은 앞 둘만', () => expect(authorInitials('Mary Jane Watson')).toBe('MJ'))
  it('빈 문자열·기호뿐이면 ?', () => {
    expect(authorInitials('')).toBe('?')
    expect(authorInitials('  ')).toBe('?')
    expect(authorInitials('---')).toBe('?')
  })
  it('일본어·한자 이름은 첫 글자', () => {
    expect(authorInitials('山田 太郎')).toBe('山')
    expect(authorInitials('田中')).toBe('田')
  })
  it('서로게이트 쌍을 쪼개지 않는다', () => expect(authorInitials('𠮷野 家')).toBe('𠮷'))
  it('앞의 기호는 건너뛴다', () => expect(authorInitials('"Bob" Smith')).toBe('BS'))
})

describe('nodeColors', () => {
  const LANES = ['#D9A441', '#7FA06A', '#C4624A', '#6E9AA6', '#A5809F', '#C9B07A', '#8FB3A0', '#C98B7A', '#B8821C', '#6A8C55', '#B5543D', '#5A8794', '#92698C']
  it('모든 레인 색에서 글자 대비 AA(4.5) 이상', () => {
    for (const lane of LANES) {
      const c = nodeColors(lane)
      const h = (s: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16)) as [number, number, number]
      expect(contrastRatio(h(c.fill), h(c.text))).toBeGreaterThanOrEqual(4.5)
    }
  })
  it('대비가 충분하면 채움색을 바꾸지 않는다', () => {
    expect(nodeColors('#D9A441').fill).toBe('#D9A441')
  })
  it('해석할 수 없는 색은 그대로', () => {
    expect(nodeColors('red')).toEqual({ fill: 'red', text: '#ffffff' })
  })
})
