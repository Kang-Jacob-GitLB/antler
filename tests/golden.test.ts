import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { Branch, FileDiff, RemoteGroup } from '../src/shared/types'
import { bucketize, TimeStreamParser } from '../src/main/git/activity'
import { assertCommitId, imageMime, isResolvableRef, normalizeDir, normalizeFilePath, parseTree } from '../src/main/git/codeBrowse'
import { isSafeSubmodulePath } from '../src/main/git/commands'
import { pickDefaultBranch } from '../src/main/git/defaultBranch'
import { assertRelPath, filterOverrideArgs, hasShortNameSegment, parseFilterDrivers } from '../src/main/git/wip'
import { addedLines, gitattributesUsesLfs, parseCheckAttrLfs, parseLfsPointer } from '../src/shared/lfs'
import { parseGitflowConfig } from '../src/shared/gitflow'
import { parseBlame } from '../src/main/git/parsers/blame'
import { parseChanges, parseNameStatus, parseNumstat } from '../src/main/git/parsers/changes'
import { parseCommitMeta } from '../src/main/git/parsers/commit'
import { DiffParser } from '../src/main/git/parsers/diff'
import { parseFileHistory } from '../src/main/git/parsers/fileHistory'
import { parseLog } from '../src/main/git/parsers/log'
import { splitNul } from '../src/main/git/parsers/nulRecords'
import { parseRefs, parseRemoteNames } from '../src/main/git/parsers/refs'
import { parseStashes } from '../src/main/git/parsers/stash'
import { parseGitlinks, parseGitmodules, parseProbe } from '../src/main/git/parsers/submodule'
import { WipStatusParser } from '../src/main/git/parsers/wipStatus'
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
  nulRecords: (b) => splitNul(b),
  // args.path·oldPath: 파일 경로, maxLines·maxBytes·countLimit·countBytes: DiffParseOpts, chunk: 입력을 이 바이트 수씩 나눠 push(없으면 한 번에),
  // summary: hunk 의 줄 목록 대신 줄 수와 처음·마지막 줄만(큰 diff 의 골든을 작게 둔다)
  diff: (b, a) => {
    const n = (k: string): number | undefined => (a[k] === undefined ? undefined : Number(a[k]))
    const p = new DiffParser(a.path!, a.oldPath, { maxLines: n('maxLines'), maxBytes: n('maxBytes'), countLimit: n('countLimit'), countBytes: n('countBytes') })
    const size = n('chunk')
    if (size) for (let i = 0; i < b.length; i += size) p.push(b.subarray(i, i + size))
    else p.push(b)
    const d = p.finish()
    return a.summary ? summarizeDiff(d) : d
  },
  // args.chunk: 청크 크기, args.untrackedLimit: 추적 안 되는 파일 목록 상한
  wipStatus: (b, a) => {
    const p = new WipStatusParser(a.untrackedLimit === undefined ? undefined : Number(a.untrackedLimit))
    const size = a.chunk === undefined ? 0 : Number(a.chunk)
    if (size) for (let i = 0; i < b.length; i += size) p.push(b.subarray(i, i + size))
    else p.push(b)
    return p.finish()
  },
  nameStatus: (b) => parseNameStatus(b),
  numstat: (b) => parseNumstat(b),
  // args.numstat: `--numstat -z` 출력 fixture
  changes: (b, a) => parseChanges(b, readFileSync(join(FIXTURES, a.numstat!))),
  // 필드가 모자라면 던진다 — 던진 메시지를 {error} 로 고정한다
  commit: (b) => {
    try {
      return parseCommitMeta(b)
    } catch (e) {
      return { error: (e as Error).message }
    }
  },
  blame: (b) => parseBlame(b),
  // args.fallback: name-status 가 없는 레코드에 쓸 요청 경로
  fileHistory: (b, a) => parseFileHistory(b, a.fallback ?? ''),
  gitlinks: (b) => parseGitlinks(b),
  // Map → 삽입 순서를 지킨 [{path, name, url?, branch?}]
  gitmodules: (b) => [...parseGitmodules(b)].map(([path, m]) => ({ path, ...m })),
  probe: (b) => parseProbe(b.toString('utf8')),

  // ---- Phase 6a PR 1: 코드 탐색기·LFS·gitflow·활동·기본 브랜치·WIP 경로 검증 ----
  // args.dir: 정규화된 디렉터리(경로 접두)
  tree: (b, a) => parseTree(b, a.dir ?? ''),
  // JSON 입력 목록의 각 케이스를 `{in, out}`(던지면 `{in, error}`)로 바꾼다. 비문자열 케이스(5·null)는 TS 가 던지는지 본다
  commitId: (b) =>
    cases(b, (c) => {
      assertCommitId(c)
      return true
    }),
  // args.file: 파일 경로 정규화(빈 경로 거부)
  normalizeDir: (b, a) => cases(b, (c) => (a.file ? normalizeFilePath(c) : normalizeDir(c))),
  resolveRef: (b) => cases(b, (c) => isResolvableRef(c as string)),
  imageMime: (b) => cases(b, (c) => imageMime(c as string) ?? null),
  lfsPointer: (b) => cases(b, (c) => parseLfsPointer(c as string)),
  checkAttrLfs: (b) => cases(b, (c) => parseCheckAttrLfs(c as string)),
  addedLines: (b) => cases(b, (c) => addedLines(c as string)),
  gitattributes: (b) => cases(b, (c) => gitattributesUsesLfs(c as string)),
  gitflowConfig: (b) => cases(b, (c) => parseGitflowConfig(c as string)),
  // args.chunk: 청크 크기. 값은 Uint32Array 에 담겨 2^32 로 감긴다
  timeStream: (b, a) => {
    const p = new TimeStreamParser()
    const size = a.chunk === undefined ? 0 : Number(a.chunk)
    if (size) for (let i = 0; i < b.length; i += size) p.push(b.subarray(i, i + size))
    else p.push(b)
    p.end()
    return { n: p.n, values: Array.from(p.values.subarray(0, p.n)) }
  },
  // 케이스: {times, now} — `now` 를 고정해 결정적으로 만든다
  bucketize: (b) => cases(b, (c) => bucketize((c as { times: number[] }).times, (c as { times: number[] }).times.length, (c as { now: number }).now)),
  defaultBranch: (b) =>
    cases(b, (c) => {
      const x = c as { originHead: string | null; local: Branch[]; remotes: RemoteGroup[] }
      return pickDefaultBranch(x.originHead, x.local, x.remotes) ?? null
    }),
  // 케이스는 `git config -z --get-regexp` stdout 전체: 문자열(UTF-8) 또는 `{parts: [문자열 | {hex} | {repeat: [단위, 횟수]}]}`(바이트 — 단위의 `{i}` 는 0 부터의 번호).
  // 결과는 덮어쓰기 `-c` 인자 목록, 덮을 수 없으면(이름에 "=" · 잘못된 UTF-8 · 개수·길이 상한) 오류
  filterOverrides: (b) => cases(b, (c) => filterOverrideArgs(caseBytes(c))),
  // 같은 케이스에서 `parseFilterDrivers` 의 결과(드라이버 이름 목록, 개수 상한까지만 — 길이 상한은 위 filterOverrides 몫)
  filterDrivers: (b) => cases(b, (c) => parseFilterDrivers(caseBytes(c))),
  relPath: (b) =>
    cases(b, (c) => {
      assertRelPath(c)
      return true
    }),
  shortName: (b) => cases(b, (c) => hasShortNameSegment(c as string)),
  submodulePath: (b) => cases(b, (c) => isSafeSubmodulePath(c as string))
}

/** filterOverrides 케이스를 바이트로 푼다. 문자열은 UTF-8, `{parts}` 는 조각을 이어 붙인다(잘못된 UTF-8 은 `{hex}` 로 쓴다) */
function caseBytes(c: unknown): Buffer {
  if (typeof c === 'string') return Buffer.from(c, 'utf8')
  const parts = (c as { parts: (string | { hex: string } | { repeat: [string, number] })[] }).parts
  return Buffer.concat(
    parts.map((p) => {
      if (typeof p === 'string') return Buffer.from(p, 'utf8')
      if ('hex' in p) return Buffer.from(p.hex, 'hex')
      const [unit, n] = p.repeat
      return Buffer.from(Array.from({ length: n }, (_, i) => unit.replaceAll('{i}', String(i))).join(''), 'utf8')
    })
  )
}

/** JSON 입력 목록의 각 케이스를 `{in, out}`(던지면 `{in, error: 메시지}`)로 바꾼다. out 이 undefined 면 null */
function cases(input: Buffer, fn: (c: unknown) => unknown): unknown[] {
  return (JSON.parse(input.toString('utf8')) as unknown[]).map((c) => {
    try {
      return { in: c, out: fn(c) ?? null }
    } catch (e) {
      return { in: c, error: (e as Error).message }
    }
  })
}

function summarizeDiff(d: FileDiff): unknown {
  return {
    ...d,
    hunks: d.hunks.map(({ lines, ...h }) => ({ ...h, lineCount: lines.length, first: lines[0], last: lines[lines.length - 1] }))
  }
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

// ---- 2차 이식 파서 ----
const E = (f: string) => `edge/${f}`
const diff = (f: string, args: Record<string, string> = {}, variant = ''): void => add('diff', E(f), { path: 'x', ...args }, variant)
diff('diff-basic.diff', { path: 'src/a.ts' })
diff('diff-basic.diff', { chunk: '1' }, '@chunk1')
diff('diff-basic.diff', { chunk: '7' }, '@chunk7')
diff('diff-basic.diff', { maxLines: '12' }, '@max12') // 내용 줄 수와 같다: 잘린 것이 아니다
diff('diff-basic.diff', { maxLines: '11' }, '@max11')
diff('diff-basic.diff', { maxLines: '8' }, '@max8') // hunk 경계에서 멈춘다
diff('diff-basic.diff', { maxLines: '3' }, '@max3')
diff('diff-basic.diff', { maxLines: '0' }, '@max0')
diff('diff-basic.diff', { maxLines: '3', chunk: '7' }, '@max3-chunk7')
diff('diff-basic.diff', { maxLines: '3', countLimit: '4' }, '@max3-count4') // 세기 한계 → totalIsLowerBound
diff('diff-basic.diff', { maxLines: '3', countBytes: '20' }, '@max3-bytes20')
diff('diff-small.diff', { maxBytes: '21' }, '@maxbytes21') // 입력 상한 직후에 끝남: 잘린 것이 아니다
diff('diff-small.diff', { maxBytes: '10', chunk: '5' }, '@maxbytes10-chunk5')
diff('diff-longline.diff', { maxBytes: '100' }, '@maxbytes100')
diff('diff-longline.diff', { maxBytes: '100', chunk: '50' }, '@maxbytes100-chunk50') // 개행 없는 조각이 상한을 넘는다
diff('diff-meta.diff', { path: '새 이름.ts', oldPath: 'old name.ts' })
diff('diff-meta-only.diff')
diff('diff-binary.diff')
diff('diff-gitbinary.diff')
diff('diff-typechange.diff')
diff('diff-crlf.diff')
diff('diff-crlf.diff', { chunk: '1' }, '@chunk1')
diff('diff-weird.diff')
diff('diff-weird.diff', { chunk: '3' }, '@chunk3')
diff('diff-big.diff', { summary: '1' }) // 기본 상한 3000줄 → truncated
diff('diff-big.diff', { summary: '1', chunk: '4096' }, '@chunk4096')
diff('diff-big.diff', { summary: '1', countLimit: '50' }, '@count50')
diff('diff-big.diff', { summary: '1', chunk: '4096', countBytes: '1000' }, '@chunk4096-bytes1000')
diff('diff-exact3000.diff', { summary: '1' })
diff('empty.bin')

const wip = (f: string, args?: Record<string, string>, variant = ''): void => add('wipStatus', E(f), args, variant)
wip('wip-status.dat')
wip('wip-status.dat', { chunk: '1' }, '@chunk1')
wip('wip-status.dat', { chunk: '5' }, '@chunk5')
wip('wip-status.dat', { untrackedLimit: '2' }, '@limit2')
wip('wip-status.dat', { untrackedLimit: '0' }, '@limit0')
wip('wip-edge.dat')
wip('wip-edge.dat', { chunk: '3' }, '@chunk3')
wip('wip-edge.dat', { untrackedLimit: '1' }, '@limit1')
wip('empty.bin')

for (const f of ['dt-name-status-korean.bin', 'dt-name-status-root.bin', 'name-status-delete.bin', 'name-status-merge.bin', 'name-status-rename.bin']) add('nameStatus', f)
for (const f of ['name-status-edge.dat', 'name-status-no-trailing-nul.dat', 'name-status-dangling.dat', 'empty.bin']) add('nameStatus', E(f))
for (const f of ['dt-numstat-korean.bin', 'numstat-binary.bin', 'numstat-rename.bin']) add('numstat', f)
for (const f of ['numstat-edge.dat', 'empty.bin']) add('numstat', E(f))
add('changes', 'dt-name-status-korean.bin', { numstat: 'dt-numstat-korean.bin' })
add('changes', 'name-status-rename.bin', { numstat: 'numstat-rename.bin' })
add('changes', 'name-status-merge.bin', { numstat: 'numstat-binary.bin' }) // 통계가 없는 파일은 null
add('changes', 'edge/name-status-edge.dat', { numstat: 'edge/numstat-edge.dat' })
add('changes', 'edge/name-status-edge.dat', { numstat: 'edge/empty.bin' }, '@no-numstat')
add('changes', 'edge/empty.bin', { numstat: 'edge/numstat-edge.dat' })

for (const f of ['commit-meta', 'commit-merge', 'commit-root', 'commit-edge', 'commit-hex-date', 'commit-short', 'commit-nine', 'commit-no-terminator', 'commit-extra']) add('commit', E(`${f}.dat`))
add('commit', E('empty.bin'))

for (const f of ['blame.txt', 'blame-edge.txt', 'blame-dangling.txt', 'empty.bin']) add('blame', E(f))

add('fileHistory', E('filehistory.dat'), { fallback: 'src/fallback.ts' })
add('fileHistory', E('filehistory.dat'), undefined, '@no-fallback')
add('fileHistory', E('filehistory-edge.dat'), { fallback: 'src/fallback.ts' })
add('fileHistory', E('empty.bin'), { fallback: 'src/fallback.ts' })

for (const f of ['gitlinks.dat', 'gitlinks-edge.dat', 'empty.bin']) add('gitlinks', E(f))
for (const f of ['gitmodules.dat', 'gitmodules-edge.dat', 'empty.bin']) add('gitmodules', E(f))
for (const f of ['probe-ok', 'probe-nohead', 'probe-crlf-upper', 'probe-sha256', 'probe-bad-length', 'probe-dir-only', 'probe-empty-dir']) add('probe', E(`${f}.txt`))
add('probe', E('empty.bin'))

// ---- Phase 6a PR 1 ----
add('tree', E('tree-edge.dat'))
add('tree', E('tree-edge.dat'), { dir: 'src/sub' }, '@dir')
add('tree', E('empty.bin'))
for (const [parser, f] of [
  ['commitId', 'commit-id'],
  ['resolveRef', 'resolve-ref'],
  ['imageMime', 'image-mime'],
  ['lfsPointer', 'lfs-pointer'],
  ['checkAttrLfs', 'check-attr'],
  ['addedLines', 'added-lines'],
  ['gitattributes', 'gitattributes'],
  ['gitflowConfig', 'gitflow'],
  ['bucketize', 'bucketize'],
  ['defaultBranch', 'default-branch'],
  ['filterOverrides', 'filter-overrides'],
  ['filterDrivers', 'filter-overrides'],
  ['relPath', 'rel-path'],
  ['shortName', 'short-name'],
  ['submodulePath', 'submodule-path']
] as const) {
  add(parser, E(`${f}-cases.json`))
}
add('normalizeDir', E('normalize-dir-cases.json'))
add('normalizeDir', E('normalize-dir-cases.json'), { file: '1' }, '@file')
add('timeStream', E('time-stream.txt'))
for (const chunk of ['1', '3', '7']) add('timeStream', E('time-stream.txt'), { chunk }, `@chunk${chunk}`)
add('timeStream', E('time-stream-huge.txt'))
add('timeStream', E('time-stream-huge.txt'), { chunk: '5' }, '@chunk5')
add('timeStream', E('empty.bin'))

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
