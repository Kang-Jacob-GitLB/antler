import { describe, expect, it } from 'vitest'
import { DiffParser, parseDiff } from '../src/main/git/parsers/diff'

const buf = (s: string) => Buffer.from(s, 'utf8')

const MULTI = [
  'diff --git a/a.txt b/a.txt',
  'index 1111111..2222222 100644',
  '--- a/a.txt',
  '+++ b/a.txt',
  '@@ -1,3 +1,3 @@ function one()',
  ' keep',
  '-old',
  '+new',
  ' tail',
  '@@ -20,2 +20,3 @@',
  ' ctx',
  '+added',
  ' ctx2',
  '\\ No newline at end of file',
  ''
].join('\n')

describe('parseDiff', () => {
  it('여러 hunk 와 줄 번호', () => {
    const d = parseDiff(buf(MULTI), 'a.txt')
    expect(d.binary).toBe(false)
    expect(d.truncated).toBe(false)
    expect(d.hunks).toHaveLength(2)
    expect(d.hunks[0]).toMatchObject({ oldStart: 1, oldLines: 3, newStart: 1, newLines: 3, header: 'function one()' })
    expect(d.hunks[0]!.lines).toEqual([
      { t: ' ', text: 'keep', o: 1, n: 1 },
      { t: '-', text: 'old', o: 2 },
      { t: '+', text: 'new', n: 2 },
      { t: ' ', text: 'tail', o: 3, n: 3 }
    ])
    expect(d.hunks[1]!.lines).toEqual([
      { t: ' ', text: 'ctx', o: 20, n: 20 },
      { t: '+', text: 'added', n: 21 },
      { t: ' ', text: 'ctx2', o: 21, n: 22 },
      { t: '\\', text: 'No newline at end of file' }
    ])
    expect(d.totalLines).toBe(8)
    expect(d.meta).toEqual([])
  })

  it('개수가 생략된 hunk 헤더(@@ -1 +1 @@)는 1줄로 본다', () => {
    const d = parseDiff(buf('@@ -5 +7 @@\n-a\n+b\n'), 'x')
    expect(d.hunks[0]).toMatchObject({ oldStart: 5, oldLines: 1, newStart: 7, newLines: 1, header: '' })
  })

  it('new file mode / 삭제 / 모드 변경 / 이름 변경 meta', () => {
    const added = parseDiff(
      buf('diff --git a/r.sh b/r.sh\nnew file mode 100755\nindex 0000000..1234567\n--- /dev/null\n+++ b/r.sh\n@@ -0,0 +1 @@\n+#!/bin/sh\n'),
      'r.sh'
    )
    expect(added.meta).toEqual(['new file mode 100755'])
    expect(added.hunks[0]!.lines).toEqual([{ t: '+', text: '#!/bin/sh', n: 1 }])

    const removed = parseDiff(buf('diff --git a/r b/r\ndeleted file mode 100644\nindex 123..000\n--- a/r\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n'), 'r')
    expect(removed.meta).toEqual(['deleted file mode 100644'])

    const mode = parseDiff(buf('diff --git a/s b/s\nold mode 100644\nnew mode 100755\n'), 's')
    expect(mode.meta).toEqual(['old mode 100644', 'new mode 100755'])
    expect(mode.hunks).toEqual([])

    const rename = parseDiff(
      buf('diff --git a/a b/b\nsimilarity index 90%\nrename from a\nrename to b\nindex 1..2 100644\n--- a/a\n+++ b/b\n@@ -1 +1 @@\n-1\n+2\n'),
      'b',
      'a'
    )
    expect(rename.oldPath).toBe('a')
    expect(rename.meta).toEqual(['similarity index 90%', 'rename from a', 'rename to b'])
  })

  it('바이너리', () => {
    const d = parseDiff(buf('diff --git a/x.bin b/x.bin\nnew file mode 100644\nindex 0000000..1234567\nBinary files /dev/null and b/x.bin differ\n'), 'x.bin')
    expect(d.binary).toBe(true)
    expect(d.hunks).toEqual([])
    expect(d.meta).toEqual(['new file mode 100644'])
  })

  it('CRLF 내용은 \\r 만 떼고 줄을 유지한다(hunk 함수 문맥 포함)', () => {
    const d = parseDiff(buf('@@ -1,2 +1,2 @@ fn()\r\n a\r\n-b\r\n+c\r\n'), 'f')
    expect(d.hunks).toHaveLength(1)
    expect(d.hunks[0]!.header).toBe('fn()')
    expect(d.hunks[0]!.lines.map((l) => l.text)).toEqual(['a', 'b', 'c'])
  })

  it('hunk 안에서 "--"/"++" 로 시작하는 줄은 내용이다', () => {
    const d = parseDiff(
      buf('diff --git a/q b/q\nindex 1..2 100644\n--- a/q\n+++ b/q\n@@ -1,2 +1,2 @@\n--- not header\n+++ not header either\n ctx\n'),
      'q'
    )
    expect(d.hunks[0]!.lines).toEqual([
      { t: '-', text: '-- not header', o: 1 },
      { t: '+', text: '++ not header either', n: 1 },
      { t: ' ', text: 'ctx', o: 2, n: 2 }
    ])
  })

  it('한글 내용', () => {
    const d = parseDiff(buf('@@ -1 +1 @@ 함수명\n-안녕\n+안녕하세요 🙂\n'), 'k')
    expect(d.hunks[0]!.header).toBe('함수명')
    expect(d.hunks[0]!.lines[1]!.text).toBe('안녕하세요 🙂')
  })

  it('두 번째 파일 섹션은 무시한다', () => {
    const d = parseDiff(buf('diff --git a/a b/a\n@@ -1 +1 @@\n-1\n+2\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-x\n+y\n'), 'a')
    expect(d.hunks).toHaveLength(1)
    expect(d.totalLines).toBe(2)
  })

  it('개행 없이 끝난 마지막 줄도 담는다', () => {
    const d = parseDiff(buf('@@ -1 +1 @@\n-a\n+b'), 'x')
    expect(d.hunks[0]!.lines).toHaveLength(2)
  })
})

describe('줄 수 상한', () => {
  const big = (n: number) => '@@ -0,0 +1,' + n + ' @@\n' + Array.from({ length: n }, (_, i) => `+line ${i}`).join('\n') + '\n'

  it('maxLines 를 넘으면 truncated, totalLines 는 전체 줄 수', () => {
    const d = parseDiff(buf(big(100)), 'big', undefined, { maxLines: 10 })
    expect(d.truncated).toBe(true)
    expect(d.hunks[0]!.lines).toHaveLength(10)
    expect(d.totalLines).toBe(100)
  })

  it('정확히 maxLines 줄이면 잘리지 않는다', () => {
    const d = parseDiff(buf(big(10)), 'big', undefined, { maxLines: 10 })
    expect(d.truncated).toBe(false)
    expect(d.totalLines).toBe(10)
  })

  it('상한 뒤의 hunk 헤더는 줄 수에 넣지 않고 truncated 로 알린다', () => {
    const text = '@@ -1,2 +1,2 @@\n a\n b\n@@ -10,2 +10,2 @@\n c\n d\n'
    const d = parseDiff(buf(text), 'x', undefined, { maxLines: 2 })
    expect(d.hunks).toHaveLength(1)
    expect(d.truncated).toBe(true)
    expect(d.totalLines).toBe(4)
  })

  it('청크를 어떤 경계로 쪼개도 결과가 같다(UTF-8 다중 바이트 포함)', () => {
    const text = buf(MULTI + '@@ -50 +50 @@\n-한글\n+글자\n')
    const whole = parseDiff(text, 'a.txt', undefined, { maxLines: 7 })
    for (const size of [1, 2, 3, 7, 64]) {
      const p = new DiffParser('a.txt', undefined, { maxLines: 7 })
      for (let i = 0; i < text.length; i += size) p.push(text.subarray(i, i + size))
      expect(p.finish()).toEqual(whole)
    }
    expect(whole.truncated).toBe(true)
    expect(whole.totalLines).toBe(10)
  })

  it('countLimit 에서 세기를 멈추고 push 가 false 를 돌려준다', () => {
    const p = new DiffParser('x', undefined, { maxLines: 5, countLimit: 20 })
    const more = p.push(buf(big(1000)))
    expect(more).toBe(false)
    const d = p.finish()
    expect(d.truncated).toBe(true)
    expect(d.totalLines).toBe(25)
  })

  it('maxBytes 를 넘으면 거기까지만 담고 truncated', () => {
    const p = new DiffParser('x', undefined, { maxBytes: 200 })
    const text = buf(big(100))
    for (let i = 0; i < text.length; i += 50) p.push(text.subarray(i, i + 50))
    const d = p.finish()
    expect(d.truncated).toBe(true)
    expect(d.hunks[0]!.lines.length).toBeLessThan(100)
    expect(d.hunks[0]!.lines.length).toBeGreaterThan(0)
  })
})
