import { describe, expect, it } from 'vitest'
import { encodePath, githubPermalink, inRange, lineFragment, codeSnippet, normalizeRange, parseLineFragment } from '../src/renderer/src/lib/lineLink'

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

  it('코드 복사: 단일 줄·범위·언어', () => {
    const lines = ['a', 'b', 'c', 'd']
    expect(codeSnippet({ path: 'src/a.ts', rev: REV, range: { start: 2, end: 2 }, lines, lang: 'typescript' })).toBe('src/a.ts:2 @ aaaaaaa\n```typescript\nb\n```')
    expect(codeSnippet({ path: 'src/a.ts', rev: REV, range: { start: 2, end: 3 }, lines, lang: 'typescript' })).toBe('src/a.ts:2-3 @ aaaaaaa\n```typescript\nb\nc\n```')
  })

  it('코드 복사: 언어가 없으면 펜스 이름이 비고, 범위가 파일 끝을 넘으면 자른다', () => {
    expect(codeSnippet({ path: 'Makefile2', rev: REV, range: { start: 3, end: 99 }, lines: ['a', 'b', 'c', 'd'], lang: null })).toBe('Makefile2:3-4 @ aaaaaaa\n```\nc\nd\n```')
    expect(codeSnippet({ path: 'x', rev: REV, range: { start: 4, end: 9 }, lines: ['a', 'b', 'c', 'd'] })).toBe('x:4 @ aaaaaaa\n```\nd\n```')
  })

  it('코드 복사: 코드에 백틱 펜스가 있으면 바깥 펜스를 늘린다', () => {
    const lines = ['```js', 'x', '```']
    expect(codeSnippet({ path: 'a.md', rev: REV, range: { start: 1, end: 3 }, lines, lang: 'markdown' })).toBe('a.md:1-3 @ aaaaaaa\n````markdown\n```js\nx\n```\n````')
    expect(codeSnippet({ path: 'a.md', rev: REV, range: { start: 1, end: 1 }, lines: ['````'], lang: null })).toBe('a.md:1 @ aaaaaaa\n`````\n````\n`````')
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
