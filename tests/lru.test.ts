import { describe, expect, it } from 'vitest'
import { LruCache } from '../src/main/services/lru'

describe('LruCache', () => {
  it('항목 수 상한을 넘으면 가장 오래 쓰지 않은 항목부터 버린다', () => {
    const c = new LruCache<string, number>(2)
    c.set('a', 1)
    c.set('b', 2)
    expect(c.get('a')).toBe(1) // a 가 최근 사용
    c.set('c', 3)
    expect(c.has('b')).toBe(false)
    expect(c.get('a')).toBe(1)
    expect(c.get('c')).toBe(3)
  })

  it('총 비용 상한을 넘으면 비용이 찰 때까지 버린다', () => {
    const c = new LruCache<string, string>(100, 10)
    c.set('a', 'A', 4)
    c.set('b', 'B', 4)
    c.set('c', 'C', 4)
    expect(c.has('a')).toBe(false)
    expect(c.cost).toBe(8)
    expect(c.size).toBe(2)
  })

  it('혼자 상한을 넘는 항목은 저장하지 않고 기존 항목도 건드리지 않는다', () => {
    const c = new LruCache<string, string>(100, 10)
    c.set('a', 'A', 5)
    c.set('big', 'B', 11)
    expect(c.has('big')).toBe(false)
    expect(c.get('a')).toBe('A')
  })

  it('같은 키를 다시 set 하면 비용을 갱신한다', () => {
    const c = new LruCache<string, string>(100, 10)
    c.set('a', 'A', 6)
    c.set('a', 'A2', 3)
    expect(c.cost).toBe(3)
    expect(c.get('a')).toBe('A2')
    expect(c.delete('a')).toBe(true)
    expect(c.cost).toBe(0)
    expect(c.get('a')).toBeUndefined()
  })

  it('clear 는 모두 비운다', () => {
    const c = new LruCache<number, number>(5)
    c.set(1, 1)
    c.clear()
    expect(c.size).toBe(0)
    expect(c.cost).toBe(0)
  })
})
