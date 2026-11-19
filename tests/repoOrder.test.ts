import { describe, expect, it } from 'vitest'
import { moveItem, permuteSlots, sortByOrder } from '../src/renderer/src/lib/repoOrder'

const byStr = (a: string, b: string): number => a.localeCompare(b)
const id = (x: string): string => x

describe('sortByOrder', () => {
  it('순서 목록에 있는 것은 그 순서로, 없는 것은 기본 정렬로 뒤에 붙인다', () => {
    expect(sortByOrder(['d', 'b', 'a', 'c'], id, ['c', 'a'], byStr)).toEqual(['c', 'a', 'b', 'd'])
  })
  it('목록이 없거나 없어진 id 만 있으면 기본 정렬', () => {
    expect(sortByOrder(['b', 'a'], id, undefined, byStr)).toEqual(['a', 'b'])
    expect(sortByOrder(['b', 'a'], id, ['gone'], byStr)).toEqual(['a', 'b'])
  })
})

describe('moveItem', () => {
  it('대상 앞·뒤로 옮긴다', () => {
    expect(moveItem(['a', 'b', 'c', 'd'], 'd', 'b', false)).toEqual(['a', 'd', 'b', 'c'])
    expect(moveItem(['a', 'b', 'c', 'd'], 'a', 'c', true)).toEqual(['b', 'c', 'a', 'd'])
    expect(moveItem(['a', 'b', 'c'], 'a', 'c', true)).toEqual(['b', 'c', 'a'])
  })
  it('같은 항목이거나 목록에 없으면 그대로', () => {
    expect(moveItem(['a', 'b'], 'a', 'a', true)).toEqual(['a', 'b'])
    expect(moveItem(['a', 'b'], 'x', 'a', false)).toEqual(['a', 'b'])
  })
})

describe('permuteSlots', () => {
  it('일부가 차지한 자리 안에서만 순서를 바꾸고 나머지는 제자리', () => {
    // 그룹 X = a, c, e 를 e, a, c 로 — b, d 는 그대로
    expect(permuteSlots(['a', 'b', 'c', 'd', 'e'], ['e', 'a', 'c'])).toEqual(['e', 'b', 'a', 'd', 'c'])
  })
  it('전체에 없는 id 는 무시한다', () => {
    expect(permuteSlots(['a', 'b'], ['zz', 'b', 'a'])).toEqual(['b', 'a'])
  })
})
