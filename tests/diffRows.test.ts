import { describe, expect, it } from 'vitest'
import type { DiffHunk, DiffLine } from '../src/shared/types'
import { changedSpans, splitRows, unifiedRows } from '../src/renderer/src/diff/diffRows'

const ctx = (text: string, o: number, n: number): DiffLine => ({ t: ' ', text, o, n })
const del = (text: string, o: number): DiffLine => ({ t: '-', text, o })
const add = (text: string, n: number): DiffLine => ({ t: '+', text, n })
const hunk = (lines: DiffLine[]): DiffHunk => ({ oldStart: 1, oldLines: 0, newStart: 1, newLines: 0, header: '', lines })

describe('changedSpans', () => {
  it('공통 앞뒤를 빼고 바뀐 구간만 남긴다', () => {
    expect(changedSpans('const a = 1', 'const a = 2')).toEqual([
      [10, 11],
      [10, 11]
    ])
    expect(changedSpans('안녕하세요 세계', '안녕하세요 우주')).toEqual([
      [6, 8],
      [6, 8]
    ])
  })
  it('같은 줄이거나 거의 전부 바뀌면 강조하지 않는다', () => {
    expect(changedSpans('abc', 'abc')).toBeNull()
    expect(changedSpans('hello world', 'xyz')).toBeNull()
  })
  it('삽입만 있으면 한쪽 구간은 비어 있다', () => {
    expect(changedSpans('foo(a)', 'foo(a, b)')).toEqual([
      [5, 5],
      [5, 8]
    ])
  })
  it('이모지 서로게이트 쌍을 가르지 않는다', () => {
    const s = changedSpans('x😀y', 'x😃y')!
    // 두 이모지는 첫 코드 유닛(\ud83d)이 같지만 구간은 쌍 전체를 감싼다
    expect(s[0]).toEqual([1, 3])
  })
})

describe('unifiedRows', () => {
  it('hunk 머리 행 뒤에 줄을 순서대로 두고, 짝지어진 수정 줄에 구간을 붙인다', () => {
    const rows = unifiedRows([hunk([ctx('a', 1, 1), del('let x = 1', 2), add('let x = 2', 2), ctx('b', 3, 3)])])
    expect(rows.map((r) => (r.kind === 'hunk' ? '@' : r.line.t))).toEqual(['@', ' ', '-', '+', ' '])
    const changed = rows.filter((r) => r.kind === 'line' && r.span)
    expect(changed).toHaveLength(2)
  })
})

describe('splitRows', () => {
  it('삭제·추가 묶음을 순서대로 짝짓고 남는 쪽은 비운다', () => {
    const rows = splitRows([hunk([del('a', 1), del('b', 2), del('c', 3), add('A', 1), ctx('z', 4, 2), add('new', 3)])])
    const pairs = rows.filter((r) => r.kind === 'pair')
    expect(pairs.map((p) => [p.left?.line.text ?? null, p.right?.line.text ?? null])).toEqual([
      ['a', 'A'],
      ['b', null],
      ['c', null],
      ['z', 'z'],
      [null, 'new']
    ])
  })

  it('"No newline" 표시는 자기 쪽 묶음에 붙는다', () => {
    const nl: DiffLine = { t: '\\', text: ' No newline at end of file' }
    const rows = splitRows([hunk([del('old', 1), nl, add('new', 1)])])
    const pairs = rows.filter((r) => r.kind === 'pair')
    expect(pairs[0]).toMatchObject({ left: { line: { text: 'old' } }, right: { line: { text: 'new' } } })
    expect(pairs[1]).toMatchObject({ left: { line: { t: '\\' } } })
    expect(pairs[1]!.kind === 'pair' && pairs[1]!.right).toBeUndefined()
  })
})
