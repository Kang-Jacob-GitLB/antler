// 워크로그 모델: 주 시작(일요일) · 월 격자 · 하루 표시 시각 · 점심 띠
import { describe, expect, it } from 'vitest'
import type { JiraWorklog } from '../src/shared/types'
import { hourRange, ISSUE_COLOR_COUNT, issueColorIndex, issueColorVar, isWeekend, layoutBlocks, lunchBand, rangeFor, weekdayIndex, weekStart } from '../src/renderer/src/components/jira/worklog/worklogModel'

// 2026-10-21 은 수요일
const log = (started: string, seconds: number): JiraWorklog => ({ id: started, issueKey: 'A-1', issueSummary: 's', issueType: 'Task', authorId: 'u', author: 'u', started, seconds, comment: '' }) as JiraWorklog
const blocksAt = (h: number, m: number, sec: number) => layoutBlocks([log(new Date(2026, 9, 21, h, m).toISOString(), sec)])

describe('주 시작은 일요일', () => {
  it('weekStart·weekdayIndex·isWeekend', () => {
    expect(weekStart('2026-10-21')).toBe('2026-10-18')
    expect(weekStart('2026-10-18')).toBe('2026-10-18')
    expect(weekStart('2026-10-24')).toBe('2026-10-18')
    expect(weekdayIndex('2026-10-18')).toBe(0)
    expect(weekdayIndex('2026-10-24')).toBe(6)
    expect(isWeekend('2026-10-18')).toBe(true)
    expect(isWeekend('2026-10-24')).toBe(true)
    expect(isWeekend('2026-10-19')).toBe(false)
  })

  it('rangeFor week 는 일~토', () => {
    const r = rangeFor('week', '2026-10-21')
    expect(r.from).toBe('2026-10-18')
    expect(r.to).toBe('2026-10-24')
    expect(r.gridDays).toHaveLength(7)
  })

  it('월 격자는 일요일에 시작해 토요일에 끝난다', () => {
    const r = rangeFor('month', '2026-10-21')
    expect(r.from).toBe('2026-09-27')
    expect(r.to).toBe('2026-10-31')
    expect(r.gridDays).toHaveLength(35)
    expect(weekdayIndex(r.gridDays[0]!)).toBe(0)
    expect(weekdayIndex(r.gridDays[r.gridDays.length - 1]!)).toBe(6)
    expect(r.days[0]).toBe('2026-10-01')
    expect(r.days).toHaveLength(31)
  })
})

describe('hourRange', () => {
  it('기본 7–21', () => expect(hourRange([])).toEqual([7, 21]))
  it('설정한 범위를 쓴다', () => expect(hourRange([], [9, 18])).toEqual([9, 18]))
  it('범위 밖 항목이 있으면 넓힌다', () => {
    expect(hourRange(blocksAt(6, 30, 3600), [9, 18])).toEqual([6, 18])
    expect(hourRange(blocksAt(20, 0, 3 * 3600), [9, 18])).toEqual([9, 23])
  })
  it('범위 안 항목은 그대로', () => expect(hourRange(blocksAt(10, 0, 3600), [9, 18])).toEqual([9, 18]))
})

describe('lunchBand', () => {
  it('평일 12–13시', () => expect(lunchBand('2026-10-21', 7, 21)).toEqual({ start: 720, end: 780 }))
  it('주말은 없다', () => {
    expect(lunchBand('2026-10-18', 7, 21)).toBeNull()
    expect(lunchBand('2026-10-24', 7, 21)).toBeNull()
  })
  it('보이는 범위 밖이면 없고, 걸치면 보이는 만큼만', () => {
    expect(lunchBand('2026-10-21', 13, 21)).toBeNull()
    expect(lunchBand('2026-10-21', 7, 12)).toBeNull()
    expect(lunchBand('2026-10-21', 7, 13)).toEqual({ start: 720, end: 780 })
    expect(lunchBand('2026-10-21', 12, 21)).toEqual({ start: 720, end: 780 })
  })
})

describe('이슈 키 색', () => {
  it('같은 키는 항상 같은 색 번호, 번호는 팔레트 범위 안', () => {
    expect(issueColorIndex('PROJ-123')).toBe(issueColorIndex('PROJ-123'))
    for (const k of ['A-1', 'PROJ-123', 'ZZ_9-99999', '']) {
      const i = issueColorIndex(k)
      expect(Number.isInteger(i) && i >= 0 && i < ISSUE_COLOR_COUNT).toBe(true)
      expect(issueColorVar(k)).toBe(`var(--wl-issue-${i})`)
    }
  })
  it('값이 고정돼 있다(세션·버전이 바뀌어도 색이 바뀌지 않는다)', () => {
    expect(['PROJ-1', 'PROJ-2', 'PROJ-3'].map(issueColorIndex)).toEqual([2, 9, 0])
  })
  it('연속된 키도 팔레트 대부분에 흩어진다', () => {
    const used = new Set(Array.from({ length: 60 }, (_, n) => issueColorIndex(`PROJ-${n + 1}`)))
    expect(used.size).toBeGreaterThanOrEqual(8)
  })
})
