import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseLog } from '../src/main/git/parsers/log'
import { splitNul } from '../src/main/git/parsers/nulRecords'
import { parseRefs, parseRemoteNames } from '../src/main/git/parsers/refs'
import { parseStashes } from '../src/main/git/parsers/stash'
import { parseWorktrees } from '../src/main/git/parsers/worktree'

// TS 파서의 출력을 골든 JSON 으로 고정한다. Rust 이식(src-tauri/tests/golden.rs)이 같은 manifest·골든을 읽어 같은 결과를 내는지 본다.
// 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts` 로 다시 쓴다.
// 이 테스트는 git 을 실행하지 않는다(순수 파서 + 저장된 fixture).

const FIXTURES = join(__dirname, 'fixtures')
const GOLDEN = join(__dirname, 'golden', 'parsers')
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** manifest 의 한 항목. fixture 는 tests/fixtures 기준 경로, args 는 파서별 추가 입력 */
interface Entry {
  parser: string
  fixture: string
  args?: Record<string, string>
  golden: string
}

type Parser = (input: Buffer, args: Record<string, string>) => unknown

const PARSERS: Record<string, Parser> = {
  log: (b) => parseLog(b),
  // args.remotes: `git remote` 출력 fixture(없으면 리모트 이름 목록 없이)
  refs: (b, a) => parseRefs(b, a.remotes ? parseRemoteNames(readFileSync(join(FIXTURES, a.remotes))) : []),
  stash: (b) => parseStashes(b),
  worktree: (b) => parseWorktrees(b),
  nulRecords: (b) => splitNul(b)
}

const EDGE = ['empty.bin']
const entries: Entry[] = []
const add = (parser: string, fixture: string, args?: Record<string, string>, variant = ''): void => {
  const golden = `${parser}__${fixture.replace(/\//g, '_')}${variant}.json`
  entries.push(args ? { parser, fixture, args, golden } : { parser, fixture, golden })
}

add('log', 'log.bin')
add('log', 'edge/log-edge.bin')
add('refs', 'for-each-ref.bin', { remotes: 'remotes.txt' })
add('refs', 'edge/refs-edge.bin', { remotes: 'edge/remotes-edge.txt' })
add('refs', 'edge/refs-edge.bin', undefined, '@no-remotes') // 리모트 이름 목록 없이: 첫 '/' 앞이 리모트 이름
add('refs', 'edge/refs-no-trailing-newline.bin')
add('stash', 'stash.bin')
add('stash', 'edge/stash-edge.bin')
add('worktree', 'worktree.bin')
add('worktree', 'edge/worktree-edge.bin')
for (const e of EDGE) for (const p of ['log', 'refs', 'stash', 'worktree']) add(p, `edge/${e}`)
// nulRecords 는 NUL 구분 fixture 전부(.bin)에 돌린다
for (const f of readdirSync(FIXTURES).filter((n) => n.endsWith('.bin')).sort()) add('nulRecords', f)
for (const f of readdirSync(join(FIXTURES, 'edge')).filter((n) => n.endsWith('.bin')).sort()) add('nulRecords', `edge/${f}`)

/** 키를 정렬해 안정적인 JSON 으로 쓴다(2칸 들여쓰기·LF·끝 줄바꿈). undefined 는 JSON.stringify 처럼 빠지고 NaN 은 null 이 된다 */
function stable(value: unknown): string {
  const emit = (v: unknown, indent: string): string => {
    if (Array.isArray(v)) {
      if (!v.length) return '[]'
      const inner = `${indent}  `
      return `[\n${v.map((x) => inner + emit(x, inner)).join(',\n')}\n${indent}]`
    }
    if (v && typeof v === 'object') {
      const keys = Object.keys(v).filter((k) => (v as Record<string, unknown>)[k] !== undefined).sort()
      if (!keys.length) return '{}'
      const inner = `${indent}  `
      return `{\n${keys.map((k) => `${inner}${JSON.stringify(k)}: ${emit((v as Record<string, unknown>)[k], inner)}`).join(',\n')}\n${indent}}`
    }
    return JSON.stringify(v) ?? 'null'
  }
  return `${emit(value, '')}\n`
}

const manifestText = `${JSON.stringify(entries, null, 2)}\n`

describe('골든 파일 (TS 파서 출력 고정)', () => {
  if (UPDATE) {
    it('UPDATE_GOLDEN=1: 골든과 manifest 를 다시 쓴다', () => {
      mkdirSync(GOLDEN, { recursive: true })
      writeFileSync(join(GOLDEN, 'manifest.json'), manifestText)
      for (const e of entries) {
        writeFileSync(join(GOLDEN, e.golden), stable(PARSERS[e.parser]!(readFileSync(join(FIXTURES, e.fixture)), e.args ?? {})))
      }
    })
    return
  }

  it('manifest 가 이 테스트의 목록과 같다', () => {
    expect(readFileSync(join(GOLDEN, 'manifest.json'), 'utf8').replace(/\r\n/g, '\n')).toBe(manifestText)
  })

  it('골든 폴더에 manifest 에 없는 파일이 없다', () => {
    const known = new Set([...entries.map((e) => e.golden), 'manifest.json'])
    expect(readdirSync(GOLDEN).filter((n) => !known.has(n))).toEqual([])
  })

  it.each(entries)('$parser × $fixture', (e) => {
    const path = join(GOLDEN, e.golden)
    expect(existsSync(path), `골든이 없다: ${e.golden} (UPDATE_GOLDEN=1 로 만든다)`).toBe(true)
    // autocrlf 로 CRLF 가 되어 체크아웃된 골든도 같은 내용으로 본다
    expect(stable(PARSERS[e.parser]!(readFileSync(join(FIXTURES, e.fixture)), e.args ?? {}))).toBe(readFileSync(path, 'utf8').replace(/\r\n/g, '\n'))
  })
})
