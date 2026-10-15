import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseChanges, parseNameStatus, parseNumstat } from '../src/main/git/parsers/changes'
import { parseCommitMeta } from '../src/main/git/parsers/commit'

const fx = (name: string) => readFileSync(resolve(__dirname, 'fixtures', name))

describe('parseChanges (fixture)', () => {
  it('이름 변경: 유사도와 numstat 을 변경 후 경로로 합친다', () => {
    const files = parseChanges(fx('name-status-rename.bin'), fx('numstat-rename.bin'))
    expect(files).toEqual([
      { status: 'R', path: 'src/main app.ts', oldPath: 'src/app.ts', similarity: 50, additions: 1, deletions: 0, binary: false }
    ])
  })

  it('바이너리는 additions/deletions 가 null', () => {
    const files = parseChanges(Buffer.from('A\0bin/logo.bin\0'), fx('numstat-binary.bin'))
    expect(files).toEqual([{ status: 'A', path: 'bin/logo.bin', additions: null, deletions: null, binary: true }])
  })

  it('삭제', () => {
    const files = parseChanges(fx('name-status-delete.bin'), Buffer.from('0\t3\tREADME.md\0'))
    expect(files).toEqual([{ status: 'D', path: 'README.md', additions: 0, deletions: 3, binary: false }])
  })

  it('merge 커밋(첫 부모 대비): 추가 + 이름 변경 순서를 유지한다', () => {
    const ns = parseNameStatus(fx('name-status-merge.bin'))
    expect(ns.map((e) => [e.status, e.path, e.oldPath])).toEqual([
      ['A', 'bin/logo.bin', undefined],
      ['R', 'src/main app.ts', 'src/app.ts']
    ])
  })

  it('diff-tree 출력: 한글 + 공백 경로', () => {
    const files = parseChanges(fx('dt-name-status-korean.bin'), fx('dt-numstat-korean.bin'))
    expect(files.map((f) => f.path)).toEqual(['src/app.ts', '문서/설계 노트.md'])
    expect(files[1]).toMatchObject({ status: 'A', additions: 1, deletions: 0, binary: false })
  })

  it('diff-tree --root 출력', () => {
    expect(parseNameStatus(fx('dt-name-status-root.bin'))).toEqual([{ status: 'A', path: 'README.md' }])
  })
})

describe('parseNameStatus / parseNumstat (합성 입력)', () => {
  it('알 수 없는 상태 문자는 M, 복사는 C + 유사도', () => {
    const buf = Buffer.from('X\0a\0B\0b\0C075\0old\0new\0T\0t\0')
    expect(parseNameStatus(buf)).toEqual([
      { status: 'M', path: 'a' },
      { status: 'M', path: 'b' },
      { status: 'C', path: 'new', oldPath: 'old', similarity: 75 },
      { status: 'T', path: 't' }
    ])
  })

  it('numstat: 이름 변경 3토큰, 경로 안의 탭', () => {
    const buf = Buffer.from('1\t2\t\0old\0new\0-\t-\t\0o.bin\0n.bin\0' + '3\t4\ta\tb\0')
    expect(parseNumstat(buf)).toEqual([
      { path: 'new', additions: 1, deletions: 2, binary: false },
      { path: 'n.bin', additions: null, deletions: null, binary: true },
      { path: 'a\tb', additions: 3, deletions: 4, binary: false }
    ])
  })

  it('numstat 에 없는 파일은 통계가 null', () => {
    expect(parseChanges(Buffer.from('M\0x\0'), Buffer.alloc(0))).toEqual([
      { status: 'M', path: 'x', additions: null, deletions: null, binary: false }
    ])
  })
})

describe('parseCommitMeta', () => {
  const fields = (body: string, parents = 'p1 p2') =>
    Buffer.from(['abc123', parents, '홍길동', 'a@x', '1700000000', '커미터', 'c@x', '1700000100', '제목 줄', body].join('\0') + '\0')

  it('필드를 나누고 본문을 trim 한다', () => {
    const m = parseCommitMeta(fields('\n본문 1\n\n본문 2\n'))
    expect(m).toEqual({
      hash: 'abc123',
      parents: ['p1', 'p2'],
      authorName: '홍길동',
      authorEmail: 'a@x',
      authorDate: 1700000000,
      committerName: '커미터',
      committerEmail: 'c@x',
      commitDate: 1700000100,
      subject: '제목 줄',
      body: '본문 1\n\n본문 2'
    })
  })

  it('루트 커밋은 parents 가 빈 배열, 본문이 비어도 된다', () => {
    const m = parseCommitMeta(fields('', ''))
    expect(m.parents).toEqual([])
    expect(m.body).toBe('')
  })

  it('형식이 어긋나면 던진다', () => {
    expect(() => parseCommitMeta(Buffer.from('abc\0def'))).toThrow()
  })
})
