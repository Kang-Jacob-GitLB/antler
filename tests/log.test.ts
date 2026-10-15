import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseDecorations, parseLog, parseLogRecord } from '../src/main/git/parsers/log'
import { NulRecordSplitter } from '../src/main/git/parsers/nulRecords'

const fixture = (name: string): Buffer => readFileSync(join(__dirname, 'fixtures', name))

describe('parseLog (fixture)', () => {
  const commits = parseLog(fixture('log.bin'))
  const bySubject = (s: string) => commits.find((c) => c.subject === s)!

  it('모든 커밋을 순서대로 파싱한다', () => {
    expect(commits).toHaveLength(7)
    expect(commits[0]!.subject).toBe('chore: README 삭제')
    expect(commits.at(-1)!.subject).toBe('초기 커밋')
    for (const c of commits) expect(c.hash).toMatch(/^[0-9a-f]{40}$/)
  })

  it('한글 작성자와 제목을 그대로 보존한다', () => {
    const c = bySubject('feat: 한글 경로 파일 추가')
    expect(c.authorName).toBe('홍길동')
    expect(c.authorEmail).toBe('dev@example.com')
    expect(c.authorDate).toBeGreaterThan(1_700_000_000)
  })

  it('merge 커밋은 부모가 둘이고 루트 커밋은 부모가 없다', () => {
    const merge = bySubject("Merge branch 'feature/이름-변경'")
    expect(merge.parents).toHaveLength(2)
    expect(bySubject('초기 커밋').parents).toEqual([])
  })

  it('부모 해시는 목록 안의 커밋을 가리킨다', () => {
    const hashes = new Set(commits.map((c) => c.hash))
    for (const c of commits) for (const p of c.parents) expect(hashes.has(p)).toBe(true)
  })

  it('HEAD·로컬·리모트·태그 라벨을 구분하고 origin/HEAD 는 버린다', () => {
    expect(commits[0]!.refs).toEqual([
      { kind: 'head', name: 'main' },
      { kind: 'local', name: 'main' }
    ])
    expect(bySubject('docs: README 수정').refs).toEqual([{ kind: 'remote', name: 'origin/main' }])
    expect(bySubject('chore: 바이너리 추가').refs).toEqual([{ kind: 'local', name: 'feature/이름-변경' }])
    expect(bySubject("Merge branch 'feature/이름-변경'").refs).toEqual([{ kind: 'tag', name: 'v0.2.0' }])
  })
})

describe('parseDecorations', () => {
  it('detached HEAD 와 복합 라벨', () => {
    expect(parseDecorations('HEAD, refs/heads/a b, tag: refs/tags/v1')).toEqual([
      { kind: 'head', name: 'HEAD' },
      { kind: 'local', name: 'a b' },
      { kind: 'tag', name: 'v1' }
    ])
  })
  it('stash 같은 기타 ref 는 무시한다', () => {
    expect(parseDecorations('refs/stash')).toEqual([])
    expect(parseDecorations('')).toEqual([])
  })
})

describe('parseLogRecord', () => {
  it('필드가 모자라면 null', () => {
    expect(parseLogRecord('abc\x1fdef')).toBeNull()
    expect(parseLogRecord('\n')).toBeNull()
  })
  it('제목 안의 구분자 문자도 잃지 않는다', () => {
    const c = parseLogRecord(['h', '', 'n', 'e', '1', '2', '', 'a\x1fb'].join('\x1f'))
    expect(c?.subject).toBe('a\x1fb')
  })
})

describe('NulRecordSplitter', () => {
  it('청크 경계에 걸친 UTF-8 다중 바이트 문자를 깨뜨리지 않는다', () => {
    const data = Buffer.from('가나다\0라마\0바', 'utf8')
    const s = new NulRecordSplitter()
    const out: string[] = []
    // 1바이트씩 쪼개서 밀어 넣는다 — 모든 경계 위치를 시험한다
    for (let i = 0; i < data.length; i++) out.push(...s.push(data.subarray(i, i + 1)))
    out.push(...s.end())
    expect(out).toEqual(['가나다', '라마', '바'])
  })
})
