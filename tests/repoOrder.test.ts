import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
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

describe('ConfigStore 저장소 목록 순서·색 검증', () => {
  let dir: string
  let store: ConfigStore
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-order-'))
    store = new ConfigStore(join(dir, 'config.json'))
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  it('올바른 순서·색은 저장한다', () => {
    store.setUi({ sourceOrder: ['wsl:Ubuntu', 'local'], groupOrder: { local: ['회사', '개인'] }, repoOrder: ['r2', 'r1'], sourceColors: { local: 195 }, groupColors: { 회사: 8 } })
    const ui = JSON.parse(readFileSync(join(dir, 'config.json'), 'utf8')).ui
    expect(ui.sourceOrder).toEqual(['wsl:Ubuntu', 'local'])
    expect(ui.groupOrder).toEqual({ local: ['회사', '개인'] })
    expect(ui.sourceColors).toEqual({ local: 195 })
  })

  it.each([
    ['중복 id', { repoOrder: ['a', 'a'] }],
    ['문자열 아님', { sourceOrder: [1] }],
    ['배열 아님', { groupOrder: { local: 'x' } }],
    ['범위 밖 색', { sourceColors: { local: 360 } }],
    ['소수 색', { groupColors: { g: 1.5 } }],
    ['배열 색 표', { sourceColors: [1] }],
    ['너무 긴 이름', { groupOrder: { local: ['x'.repeat(201)] } }]
  ])('잘못된 값(%s)은 거부한다', (_why, patch) => {
    expect(() => store.setUi(patch as never)).toThrow('잘못된 저장소 목록 설정')
  })
})
