import { describe, expect, it } from 'vitest'
import * as m from '../src/shared/checkTtl'

const sha = (n: number): string => n.toString(16).padStart(40, '0')

describe('renderer 체크 상태 수명·키', () => {
  it('상태별 수명이 지나면 신선하지 않다(진행 중 짧게, 없음 중간, 완료 길게)', () => {
    const at = 1_000
    expect(m.isFresh(undefined)).toBe(false)
    expect(m.isFresh({ state: 'pending', at }, at + m.TTL_PENDING_MS - 1)).toBe(true)
    expect(m.isFresh({ state: 'pending', at }, at + m.TTL_PENDING_MS)).toBe(false)
    expect(m.isFresh({ state: 'none', at }, at + m.TTL_NONE_MS - 1)).toBe(true)
    expect(m.isFresh({ state: 'none', at }, at + m.TTL_NONE_MS)).toBe(false)
    expect(m.isFresh({ state: 'success', at }, at + m.TTL_DONE_MS - 1)).toBe(true)
    expect(m.isFresh({ state: 'failure', at }, at + m.TTL_DONE_MS)).toBe(false)
  })

  it('키는 owner/repo 와 sha 로 만들어 저장소가 달라도 섞이지 않는다', () => {
    expect(m.scopeOf({ owner: 'Octo', repo: 'Hello' })).toBe('octo/hello')
    expect(m.scopeOf(null)).toBeNull()
    expect(m.checkKey('a/b', sha(1).toUpperCase())).toBe(`a/b|${sha(1)}`)
    expect(m.checkKey('a/b', sha(1))).not.toBe(m.checkKey('c/d', sha(1)))
  })
})
