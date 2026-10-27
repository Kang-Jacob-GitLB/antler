import { describe, expect, it } from 'vitest'
import { encodePath, githubPermalink, inRange, lineFragment, lineReference, normalizeRange, parseLineFragment } from '../src/renderer/src/lib/lineLink'

const REV = 'a'.repeat(40)

describe('줄 링크 순수 함수', () => {
  it('범위를 정규화한다', () => {
    expect(normalizeRange(20, 10)).toEqual({ start: 10, end: 20 })
    expect(normalizeRange(3, 3)).toEqual({ start: 3, end: 3 })
    expect(inRange({ start: 2, end: 4 }, 4)).toBe(true)
    expect(inRange({ start: 2, end: 4 }, 5)).toBe(false)
    expect(inRange(null, 1)).toBe(false)
  })

  it('조각: 한 줄은 L10, 범위는 L10-L20', () => {
    expect(lineFragment({ start: 10, end: 10 })).toBe('L10')
    expect(lineFragment({ start: 10, end: 20 })).toBe('L10-L20')
  })

  it('경로는 세그먼트마다 인코딩한다(공백·한글·#·?)', () => {
    expect(encodePath('src/a b/한글.ts')).toBe(`src/a%20b/${encodeURIComponent('한글.ts')}`)
    expect(encodePath('docs/#1 what?.md')).toBe('docs/%231%20what%3F.md')
    expect(encodePath('x%y.txt')).toBe('x%25y.txt')
  })

  it('permalink', () => {
    expect(githubPermalink({ owner: 'o', repo: 'r', rev: REV, path: 'a b/#c?.ts', range: { start: 5, end: 5 } })).toBe(`https://github.com/o/r/blob/${REV}/a%20b/%23c%3F.ts#L5`)
    expect(githubPermalink({ owner: 'o', repo: 'r', rev: REV, path: 'x.ts', range: normalizeRange(20, 10) })).toBe(`https://github.com/o/r/blob/${REV}/x.ts#L10-L20`)
  })

  it('참조 문자열', () => {
    expect(lineReference({ path: 'src/a.ts', rev: REV, range: { start: 10, end: 20 } })).toBe('src/a.ts:10-20 @ aaaaaaa')
    expect(lineReference({ path: 'src/a.ts', rev: REV, range: { start: 7, end: 7 } })).toBe('src/a.ts:7 @ aaaaaaa')
  })

  it('#L 조각 해석', () => {
    expect(parseLineFragment('#L10')).toEqual({ start: 10, end: 10 })
    expect(parseLineFragment('L10-L20')).toEqual({ start: 10, end: 20 })
    expect(parseLineFragment('#L20-L10')).toEqual({ start: 10, end: 20 })
    expect(parseLineFragment('#L3C2-L5C9')).toEqual({ start: 3, end: 5 })
    expect(parseLineFragment('#L0')).toBeNull()
    expect(parseLineFragment('#intro')).toBeNull()
    expect(parseLineFragment('#L1-')).toBeNull()
    expect(parseLineFragment('')).toBeNull()
    expect(parseLineFragment(null)).toBeNull()
  })
})
