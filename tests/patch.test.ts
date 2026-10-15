import { describe, expect, it } from 'vitest'
import { fileDiffFromPatch, parsePatch } from '../src/renderer/src/diff/patch'

describe('parsePatch (GitHub PR 파일 patch)', () => {
  it('여러 hunk 와 줄 번호', () => {
    const hunks = parsePatch('@@ -1,3 +1,3 @@ function a()\n ctx\n-old\n+new\n ctx2\n@@ -10 +10,2 @@\n x\n+y')
    expect(hunks).toHaveLength(2)
    expect(hunks[0]!.header).toBe('function a()')
    expect(hunks[0]!.lines.map((l) => [l.t, l.o, l.n])).toEqual([
      [' ', 1, 1],
      ['-', 2, undefined],
      ['+', undefined, 2],
      [' ', 3, 3]
    ])
    expect(hunks[1]).toMatchObject({ oldStart: 10, oldLines: 1, newStart: 10, newLines: 2 })
    expect(hunks[1]!.lines.at(-1)).toEqual({ t: '+', text: 'y', n: 11 })
  })

  it('"No newline" 표시, CRLF, 한글, -- 로 시작하는 내용 줄', () => {
    const hunks = parsePatch('@@ -1 +1 @@\r\n-한글 이전\r\n\\ No newline at end of file\r\n+-- 주석 같은 내용\r\n')
    expect(hunks[0]!.lines).toEqual([
      { t: '-', text: '한글 이전', o: 1 },
      { t: '\\', text: 'No newline at end of file' },
      { t: '+', text: '-- 주석 같은 내용', n: 1 }
    ])
  })

  it('patch 가 없으면 바이너리·대형·이름만 변경을 구분한다', () => {
    // 줄 수 변화가 없는 수정 = 바이너리
    expect(fileDiffFromPatch('a.png', undefined, undefined, { additions: 0, deletions: 0, status: 'modified' })).toMatchObject({ binary: true, meta: [] })
    // 줄 수 변화가 있는데 patch 가 없음 = 너무 큰 텍스트 diff
    expect(fileDiffFromPatch('big.json', undefined, undefined, { additions: 9000, deletions: 3, status: 'modified' })).toMatchObject({ binary: false, meta: ['too-large'] })
    // 내용 없는 이름 변경
    expect(fileDiffFromPatch('b.ts', undefined, 'a.ts', { additions: 0, deletions: 0, status: 'renamed' })).toMatchObject({ binary: false, meta: ['rename-only'] })
    // 통계를 모르면 예전처럼 바이너리로 본다
    expect(fileDiffFromPatch('x', undefined)).toMatchObject({ binary: true })
    expect(fileDiffFromPatch('a.ts', '@@ -0,0 +1 @@\n+x', 'old.ts')).toMatchObject({ binary: false, oldPath: 'old.ts', totalLines: 1 })
  })
})
