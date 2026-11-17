import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { ProcessQueue } from '../src/main/executors/ProcessQueue'
import { SshExecutor } from '../src/main/executors/SshExecutor'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { FetchService } from '../src/main/services/FetchService'
import { logArgs, refsSignature } from '../src/main/git/commands'
import { RepoService, StaleLogError } from '../src/main/services/RepoService'
import type { LogQuery } from '../src/shared/types'
import { IPC_CHANNELS } from '../src/shared/ipc'
import { knownHostsLine, makeHome, newKeyPair, startServer, type TestServer } from './sshHarness'

// 채널 패리티 골든. TS(RepoService)가 기준이고, Rust 라우터(src-tauri/tests/channels.rs)가 같은 fixture·시나리오로 같은 결과를 내는지 본다.
//  - tests/golden/exec/channels.json: IPC_CHANNELS 목록. Rust 의 RUST_CHANNELS 가 이것의 부분집합인지 Rust 테스트가 강제한다
//  - tests/golden/channels/log.json: git:log 시나리오(tests/fixtures/repos/log.scenarios.json — 다단계: 페이지·범위·저장소 조작). logArgs.json: logArgs 의 argv(logArgs.cases.json)
//  - tests/golden/channels/{compare,fileDiff,fileHistory,blame}.json: 파일 단위 채널 시나리오(tests/fixtures/repos/files.scenarios.json)
//  - tests/golden/channels/{resolveRev,tree,blob,onRemote}.json: 코드 탐색기 채널 시나리오(tests/fixtures/repos/code.scenarios.json — 한 서비스 인스턴스가 순서대로 돌아 캐시 적중도 본다. 큰 파일은 setup 으로 시험 시점에 만든다)
//  - tests/golden/channels/{refs,refsDetails}.json: 사이드바 ref 채널 시나리오(tests/fixtures/repos/refs.scenarios.json — 다단계: 저장소 조작 뒤 조회. knownDiff 는 정렬 근사 때문에 Rust 가 다른 순서를 내는 알려진 차이)
//  - tests/golden/channels/{search,activity}.json: 검색·활동 채널 시나리오(tests/fixtures/repos/search.scenarios.json — 다단계: 저장소 조작 뒤 조회. 악성 질의·범위·해시 갈래·스태시 같은 닿지 않는 커밋을 포함한다)
//  - tests/golden/channels/{repoExtras,lfsPaths}.json: LFS·gitflow 채널 시나리오(tests/fixtures/repos/extras.scenarios.json — 다단계: 저장소·작업 트리·info/attributes·설정 조작 뒤 조회. 요청 검증·묶음·1000개 상한을 포함한다)
//  - tests/golden/channels/commit.json: git:commit 시나리오(tests/fixtures/repos/commit.scenarios.json)의 결과 envelope.
//    오류는 code 만 남긴다(문구 패리티는 하지 않는다). 해시는 .fi 가 날짜·내용을 고정해 결정적이다.
//  - tests/golden/channels/fetch.json: git:fetch 시나리오(tests/fixtures/repos/fetch.scenarios.json)의 단계별 상태. TS FetchService 와 Rust 라우터가 같은 순서로 돌려 같은 상태·확인 창 횟수·승인 저장을 낸다.
//    target:'ssh' 시나리오는 127.0.0.1 의 ssh2 하네스(tests/sshHarness.ts)와 SshExecutor 로 돌린다(실제 ~/.ssh·agent 는 쓰지 않는다 — 임시 홈).
// 골든 갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts`

const FIXTURES = join(__dirname, 'fixtures', 'repos')
const GOLDEN_DIR = join(__dirname, 'golden')
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 키를 정렬하고 2칸·LF 로 쓴다(goldenExec.test.ts 와 같은 모양) */
function stable(value: unknown): string {
  const emit = (v: unknown, indent: string): string => {
    if (Array.isArray(v)) {
      if (!v.length) return '[]'
      const inner = `${indent}  `
      return `[\n${v.map((x) => inner + emit(x, inner)).join(',\n')}\n${indent}]`
    }
    if (v && typeof v === 'object') {
      const keys = Object.keys(v).sort()
      if (!keys.length) return '{}'
      const inner = `${indent}  `
      return `{\n${keys.map((k) => `${inner}${JSON.stringify(k)}: ${emit((v as Record<string, unknown>)[k], inner)}`).join(',\n')}\n${indent}}`
    }
    return JSON.stringify(v)
  }
  return `${emit(value, '')}\n`
}

function check(path: string, got: string): void {
  if (UPDATE) {
    mkdirSync(join(path, '..'), { recursive: true })
    writeFileSync(path, got)
    return
  }
  expect(existsSync(path), 'UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts 로 골든을 만든다').toBe(true)
  expect(got).toBe(readFileSync(path, 'utf8').replace(/\r\n/g, '\n'))
}

describe('채널 목록 골든(exec/channels.json)', () => {
  it('IPC_CHANNELS 가 골든과 같다', () => {
    check(join(GOLDEN_DIR, 'exec', 'channels.json'), stable({ channels: [...IPC_CHANNELS] }))
  })
})

interface Scenario {
  name: string
  repo: string
  rev?: string
  hash?: unknown
  upper?: boolean
  short?: number
  prefix?: string
  suffix?: string
}

/** src/main/ipc/handlers.ts 의 handle() 이 오류를 code 로 바꾸는 순서(git:commit 이 낼 수 있는 것만). handle 은 electron 을 불러와 직접 쓸 수 없다 */
function codeOf(err: unknown): string {
  return err instanceof AbortError ? 'ABORTED' : err instanceof StaleLogError ? 'STALE' : err instanceof GitError ? 'GIT' : 'ERROR'
}

/** 파일이 많은(상한 시나리오) 결과는 골든이 커지지 않게 개수와 처음·마지막만 남긴다. Rust 테스트도 같은 규칙으로 줄인 뒤 비교한다 */
function summarize(data: Record<string, unknown>): Record<string, unknown> {
  const files = data['files'] as unknown[]
  return files.length > 20 ? { ...data, files: { count: files.length, first: files[0], last: files[files.length - 1] } } : data
}

describe('git:commit 채널 골든(channels/commit.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'commit.scenarios.json'), 'utf8')) as { scenarios: Scenario[] }).scenarios
  const repos = [...new Set(scenarios.map((s) => s.repo))].sort()
  let root: string
  const dirs: Record<string, string> = {}
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chan-'))
    for (const r of repos) {
      const dir = join(root, r)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${r}.fi`)))
      dirs[r] = dir
    }
  }, 60_000)
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const services: RepoService[] = []
    const ids: Record<string, { svc: RepoService; id: string }> = {}
    for (const r of repos) {
      const config = new ConfigStore(join(root, `${r}.config.json`))
      const id = config.addRepo({ sourceId: 'local', path: dirs[r]!, name: r }).id
      const svc = new RepoService(config)
      services.push(svc)
      ids[r] = { svc, id }
    }
    const results: unknown[] = []
    for (const s of scenarios) {
      const { svc, id } = ids[s.repo]!
      let hash: unknown = s.hash
      if (s.rev) {
        let h = git(dirs[s.repo]!, ['rev-parse', s.rev])
        if (s.short) h = h.slice(0, s.short)
        if (s.upper) h = h.toUpperCase()
        hash = `${s.prefix ?? ''}${h}${s.suffix ?? ''}`
      }
      let result: unknown
      try {
        result = { ok: true, data: summarize(JSON.parse(JSON.stringify(await svc.commit(id, hash as string))) as Record<string, unknown>) }
      } catch (err) {
        result = { ok: false, code: codeOf(err) }
      }
      results.push({ name: s.name, repo: s.repo, hash, result })
    }
    await Promise.all(services.map((s) => s.dispose()))
    check(join(GOLDEN_DIR, 'channels', 'commit.json'), stable({ scenarios: results }))
  }, 60_000)
})

// ── git:log ────────────────────────────────────────────

type LogStep = { log: unknown; full?: boolean } | { git: string[] }
interface LogScenario {
  name: string
  repo: string
  pagesEqualFull?: [number, number, number]
  steps: LogStep[]
}

/** 시나리오 값의 '@rev:<식>'·$gen·$str 을 푼다(log.scenarios.json 의 _ 설명 참고). Rust 테스트도 같은 규칙이다 */
function expand(v: unknown, rev: (expr: string) => string): unknown {
  if (typeof v === 'string') return v.startsWith('@rev:') ? rev(v.slice(5)) : v
  if (Array.isArray(v)) return v.map((x) => expand(x, rev))
  if (v && typeof v === 'object') {
    const o = v as Record<string, unknown>
    const keys = Object.keys(o)
    if (keys.length === 1 && keys[0] === '$gen') {
      const [prefix, n] = o['$gen'] as [string, number]
      return Array.from({ length: n }, (_, i) => `${prefix}${i}`)
    }
    if (keys.length === 1 && keys[0] === '$str') {
      const [prefix, ch, n] = o['$str'] as [string, string, number]
      return prefix + ch.repeat(n)
    }
    if (keys.length === 1 && keys[0] === '$cat') return (expand(o['$cat'], rev) as string[]).join('')
    if (keys.length === 1 && keys[0] === '$upper') return String(expand(o['$upper'], rev)).toUpperCase()
    if (keys.length === 1 && keys[0] === '$short') {
      const [v, n] = o['$short'] as [unknown, number]
      return String(expand(v, rev)).slice(0, n)
    }
    return Object.fromEntries(keys.map((k) => [k, expand(o[k], rev)]))
  }
  return v
}

interface PageCommit {
  hash: string
  parents: string[]
  subject: string
  refs: { kind: string; name: string }[]
}

/** 한 줄 요약. Rust 테스트(channels.rs 의 `commit_line`)와 같은 모양이다 */
function commitLine(c: PageCommit): string {
  return `${c.hash.slice(0, 12)} ${c.parents.map((p) => p.slice(0, 7)).join(',')} ${c.subject} ${c.refs.map((r) => `${r.kind}:${r.name}`).join(',')}`
}

describe('git:log 채널 골든(channels/log.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'log.scenarios.json'), 'utf8')) as { scenarios: LogScenario[] }).scenarios
  let root: string
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chanlog-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const results: unknown[] = []
    for (const [si, sc] of scenarios.entries()) {
      const dir = join(root, `s${si}`)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${sc.repo}.fi`)))
      const config = new ConfigStore(join(root, `s${si}.config.json`))
      const id = config.addRepo({ sourceId: 'local', path: dir, name: sc.repo }).id
      const svc = new RepoService(config)
      const rev = (expr: string): string => git(dir, ['rev-parse', expr])
      const out: unknown[] = []
      const pages: PageCommit[][] = []
      for (const [i, step] of sc.steps.entries()) {
        if ('git' in step) {
          git(dir, step.git)
          out.push({ git: step.git })
          pages.push([])
          continue
        }
        const query = expand(step.log, rev)
        let result: Record<string, unknown>
        let commits: PageCommit[] = []
        try {
          const page = JSON.parse(JSON.stringify(await svc.log(id, query as LogQuery, `req-${i}`))) as { commits: PageCommit[]; hasMore: boolean }
          commits = page.commits
          result = { ok: true, data: { commits: step.full ? page.commits : page.commits.map(commitLine), hasMore: page.hasMore } }
        } catch (err) {
          const code = codeOf(err)
          result = { ok: false, code, ...(code === 'ERROR' ? { error: (err as Error).message } : {}) }
        }
        pages.push(commits)
        out.push({ log: step.log, result })
      }
      if (sc.pagesEqualFull) {
        const [full, first, last] = sc.pagesEqualFull
        const joined = pages.slice(first, last + 1).flat().map((c) => c.hash)
        expect(joined, sc.name).toEqual(pages[full]!.map((c) => c.hash))
      }
      await svc.dispose()
      results.push({ name: sc.name, repo: sc.repo, steps: out })
    }
    check(join(GOLDEN_DIR, 'channels', 'log.json'), stable({ scenarios: results }))
  }, 120_000)

  it('로그 인자(logArgs)가 골든과 같다', () => {
    const cases = (JSON.parse(readFileSync(join(FIXTURES, 'logArgs.cases.json'), 'utf8')) as { cases: { name: string; scope?: unknown; skip: number }[] }).cases
    const results = cases.map((c) => {
      try {
        return { name: c.name, args: logArgs(c.scope as never, c.skip) }
      } catch (err) {
        return { name: c.name, error: (err as Error).message }
      }
    })
    check(join(GOLDEN_DIR, 'channels', 'logArgs.json'), stable({ cases: results }))
  })
})

// ── 파일 단위 채널: git:compare · git:fileDiff · git:fileHistory · git:blame ────────────────

interface FileScenario {
  name: string
  repo?: string
  args: unknown[]
  /** true 면 ERROR 의 문구도 비교한다(검증이 만든 문구만 — TypeError 문구는 TS·Rust 가 다르다) */
  msg?: boolean
}

/** diff 는 줄이 많아 hunk 의 줄 목록을 개수·처음·마지막으로 줄인다. Rust 테스트(channels.rs 의 `summarize_diff`)도 같은 규칙이다 */
function summarizeDiff(data: Record<string, unknown>): Record<string, unknown> {
  const hunks = (data['hunks'] as { lines: unknown[] }[]).map((h) => (h.lines.length > 40 ? { ...h, lines: { count: h.lines.length, first: h.lines[0], last: h.lines[h.lines.length - 1] } } : h))
  return { ...data, hunks }
}

describe('파일 단위 채널 골든(channels/{compare,fileDiff,fileHistory,blame}.json)', () => {
  const all = JSON.parse(readFileSync(join(FIXTURES, 'files.scenarios.json'), 'utf8')) as Record<'compare' | 'fileDiff' | 'fileHistory' | 'blame', FileScenario[]>
  let root: string
  const dirs: Record<string, string> = {}
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chanfile-'))
    for (const r of ['files', 'empty']) {
      const dir = join(root, r)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${r}.fi`)))
      dirs[r] = dir
    }
  }, 60_000)
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const config = new ConfigStore(join(root, 'config.json'))
    const svc = new RepoService(config)
    const ids: Record<string, string> = {}
    for (const r of Object.keys(dirs)) ids[r] = config.addRepo({ sourceId: 'local', path: dirs[r]!, name: r }).id
    for (const channel of ['compare', 'fileDiff', 'fileHistory', 'blame'] as const) {
      const results: unknown[] = []
      for (const [i, s] of all[channel].entries()) {
        const repo = s.repo ?? 'files'
        const args = expand(s.args, (expr) => git(dirs[repo]!, ['rev-parse', expr])) as unknown[]
        let result: Record<string, unknown>
        try {
          const call = svc as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>
          const data = JSON.parse(JSON.stringify(await call[channel]!.call(svc, ids[repo], ...args, `req-${i}`))) as Record<string, unknown>
          result = { ok: true, data: channel === 'compare' ? summarize(data) : channel === 'fileDiff' ? summarizeDiff(data) : data }
        } catch (err) {
          result = { ok: false, code: codeOf(err), ...(s.msg ? { error: (err as Error).message } : {}) }
        }
        results.push({ name: s.name, repo, args, result })
      }
      check(join(GOLDEN_DIR, 'channels', `${channel}.json`), stable({ scenarios: results }))
    }
    await svc.dispose()
  }, 120_000)
})

// ── 코드 탐색기: git:resolveRev · git:tree · git:blob · git:onRemote ────────────────

interface CodeSetup {
  repo: string
  ref: string
  files: { path: string; fill: [string, number] }[]
}

/** 큰 파일 커밋(고아)을 만든다 — 작성자·시각이 고정이라 해시가 결정적이다. Rust 테스트(channels.rs 의 `make_big_commit`)도 같은 명령이다 */
function makeBigCommit(dir: string, setup: CodeSetup): void {
  const env = {
    ...process.env,
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_AUTHOR_NAME: '홍길동',
    GIT_AUTHOR_EMAIL: 'dev@example.com',
    GIT_AUTHOR_DATE: '1700000000 +0900',
    GIT_COMMITTER_NAME: '홍길동',
    GIT_COMMITTER_EMAIL: 'dev@example.com',
    GIT_COMMITTER_DATE: '1700000000 +0900'
  }
  const run = (args: string[], input?: Buffer | string): string => execFileSync('git', args, { cwd: dir, env, input }).toString('utf8').trim()
  const entries = setup.files.map((f) => `100644 blob ${run(['hash-object', '-w', '--stdin'], Buffer.alloc(f.fill[1], f.fill[0]))}\t${f.path}\n`).join('')
  const tree = run(['mktree'], entries)
  const commit = run(['-c', 'commit.gpgsign=false', 'commit-tree', tree, '-m', 'big files'])
  run(['update-ref', setup.ref, commit])
}

/** text·dataUrl 이 200 글자(UTF-16 단위)를 넘으면 {len, head, tail}, tree 항목이 20개를 넘으면 {count, first, last}. Rust 테스트(channels.rs 의 `summarize_code`)도 같은 규칙이다 */
function summarizeCode(channel: string, data: Record<string, unknown>): Record<string, unknown> {
  const out = { ...data }
  if (channel === 'blob') {
    for (const k of ['text', 'dataUrl']) {
      const v = out[k]
      if (typeof v === 'string' && v.length > 200) out[k] = { len: v.length, head: v.slice(0, 40), tail: v.slice(-40) }
    }
  }
  const entries = out['entries']
  if (channel === 'tree' && Array.isArray(entries) && entries.length > 20) out['entries'] = { count: entries.length, first: entries[0], last: entries[entries.length - 1] }
  return out
}

describe('코드 탐색기 채널 골든(channels/{resolveRev,tree,blob,onRemote}.json)', () => {
  const all = JSON.parse(readFileSync(join(FIXTURES, 'code.scenarios.json'), 'utf8')) as Record<'resolveRev' | 'tree' | 'blob' | 'onRemote', FileScenario[]> & { setup: CodeSetup[] }
  let root: string
  const dirs: Record<string, string> = {}
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chancode-'))
    for (const r of ['code', 'branchy', 'many', 'empty']) {
      const dir = join(root, r)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${r}.fi`)))
      dirs[r] = dir
    }
    for (const s of all.setup) makeBigCommit(dirs[s.repo]!, s)
  }, 60_000)
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const config = new ConfigStore(join(root, 'config.json'))
    const svc = new RepoService(config)
    const ids: Record<string, string> = {}
    for (const r of Object.keys(dirs)) ids[r] = config.addRepo({ sourceId: 'local', path: dirs[r]!, name: r }).id
    const methods = { resolveRev: 'resolveRev', tree: 'codeTree', blob: 'codeBlob', onRemote: 'codeOnRemote' } as const
    for (const channel of ['resolveRev', 'tree', 'blob', 'onRemote'] as const) {
      const results: unknown[] = []
      for (const [i, s] of all[channel].entries()) {
        const repo = s.repo ?? 'code'
        const args = expand(s.args, (expr) => git(dirs[repo]!, ['rev-parse', expr])) as unknown[]
        if (channel === 'tree' || channel === 'blob') args.push(`req-${channel}-${i}`)
        let result: Record<string, unknown>
        try {
          const call = svc as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>
          const data = JSON.parse(JSON.stringify(await call[methods[channel]]!.call(svc, ids[repo], ...args)))
          result = { ok: true, data: channel === 'resolveRev' || channel === 'onRemote' ? data : summarizeCode(channel, data as Record<string, unknown>) }
        } catch (err) {
          result = { ok: false, code: codeOf(err), ...(s.msg ? { error: (err as Error).message } : {}) }
        }
        if (channel === 'tree' || channel === 'blob') args.pop()
        results.push({ name: s.name, repo, args, result })
      }
      check(join(GOLDEN_DIR, 'channels', `${channel}.json`), stable({ scenarios: results }))
    }
    await svc.dispose()
  }, 180_000)
})

// ── 사이드바 ref 채널: git:refs · git:refsDetails ────────────────

type RefsStep = { git: unknown[]; in?: string } | { mkdir: string } | { refs: true } | { refsDetails: true }
interface RefsScenario {
  name: string
  repo: string
  steps: RefsStep[]
  knownDiff?: { rustLocal: string[] }
}

/** 시나리오의 git 단계가 쓰는 고정 작성자·시각 — 커밋·스태시 해시가 TS·Rust 에서 같다. Rust 테스트(channels.rs 의 `refs_git`)도 같은 값이다 */
const REFS_STEP_ENV = {
  GIT_AUTHOR_NAME: '홍길동',
  GIT_AUTHOR_EMAIL: 'dev@example.com',
  GIT_AUTHOR_DATE: '1700000000 +0900',
  GIT_COMMITTER_NAME: '홍길동',
  GIT_COMMITTER_EMAIL: 'dev@example.com',
  GIT_COMMITTER_DATE: '1700000000 +0900'
}

/** 결과의 문자열에 든 시험 루트 경로(`/`·`\` 두 모양)를 <ROOT> 로 바꾼다 — 작업 트리 경로가 시험 디렉터리마다 다르다. Rust 테스트(channels.rs 의 `scrub_root`)도 같은 규칙이다 */
function scrubRoot(v: unknown, root: string): unknown {
  const fwd = root.replace(/\\/g, '/')
  if (typeof v === 'string') return v.split(fwd).join('<ROOT>').split(root).join('<ROOT>')
  if (Array.isArray(v)) return v.map((x) => scrubRoot(x, root))
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, scrubRoot(x, root)]))
  return v
}

describe('사이드바 ref 채널 골든(channels/{refs,refsDetails}.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'refs.scenarios.json'), 'utf8')) as { scenarios: RefsScenario[] }).scenarios
  let root: string
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, ...REFS_STEP_ENV, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chanrefs-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const out: Record<'refs' | 'refsDetails', unknown[]> = { refs: [], refsDetails: [] }
    for (const [si, sc] of scenarios.entries()) {
      const dir = join(root, `s${si}`, 'repo')
      mkdirSync(dir, { recursive: true })
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${sc.repo}.fi`)))
      const config = new ConfigStore(join(root, `s${si}.config.json`))
      const id = config.addRepo({ sourceId: 'local', path: dir, name: sc.repo }).id
      const svc = new RepoService(config)
      // '@rev:<경로>::<식>' 은 그 하위 저장소의 rev-parse 다
      const rev = (expr: string): string => {
        const [sub, e] = expr.includes('::') ? expr.split('::') : [undefined, expr]
        return git(sub ? join(dir, sub) : dir, ['rev-parse', e!])
      }
      for (const [i, step] of sc.steps.entries()) {
        if ('mkdir' in step) {
          mkdirSync(join(dir, step.mkdir), { recursive: true })
        } else if ('git' in step) {
          git(step.in ? join(dir, step.in) : dir, expand(step.git, rev) as string[])
        } else {
          const channel = 'refs' in step ? 'refs' : 'refsDetails'
          let result: Record<string, unknown>
          try {
            const data = JSON.parse(JSON.stringify(await svc[channel](id))) as unknown
            result = { ok: true, data: scrubRoot(data, root) }
          } catch (err) {
            result = { ok: false, code: codeOf(err) }
          }
          out[channel].push({ name: sc.name, step: i, result })
        }
      }
      await svc.dispose()
    }
    for (const channel of ['refs', 'refsDetails'] as const) check(join(GOLDEN_DIR, 'channels', `${channel}.json`), stable({ scenarios: out[channel] }))
  }, 180_000)
})

// ── 검색·활동: git:search · git:activity ────────────────

type SearchStep = { git: string[] } | { search: unknown[]; msg?: boolean } | { activity: unknown[]; msg?: boolean }
interface SearchScenario {
  name: string
  repo: string
  steps: SearchStep[]
}

describe('검색·활동 채널 골든(channels/{search,activity}.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'search.scenarios.json'), 'utf8')) as { scenarios: SearchScenario[] }).scenarios
  let root: string
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, ...REFS_STEP_ENV, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chansearch-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const out: Record<'search' | 'activity', unknown[]> = { search: [], activity: [] }
    for (const [si, sc] of scenarios.entries()) {
      const dir = join(root, `s${si}`)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${sc.repo}.fi`)))
      const config = new ConfigStore(join(root, `s${si}.config.json`))
      const id = config.addRepo({ sourceId: 'local', path: dir, name: sc.repo }).id
      const svc = new RepoService(config)
      const rev = (expr: string): string => git(dir, ['rev-parse', expr])
      for (const [i, step] of sc.steps.entries()) {
        if ('git' in step) {
          git(dir, expand(step.git, rev) as string[])
          // 앱에서는 감시가 ref 변경을 알아 기억한 시그니처를 새로 계산한다 — 이 하네스에는 감시가 없으므로 같은 효과로 버린다.
          // Rust(PR 4)는 기억값을 쓰지 않고 매번 새로 계산하므로 이 호출이 없어도 같은 결과여야 한다
          svc.noteLog('logStale', id)
          continue
        }
        const channel = 'search' in step ? 'search' : 'activity'
        const list = expand('search' in step ? step.search : step.activity, rev) as unknown[]
        let result: Record<string, unknown>
        try {
          const data = channel === 'search' ? await svc.search(id, list[0] as string, list[1] as never, `req-${i}`) : await svc.activity(id, list[0] as never, `req-${i}`)
          result = { ok: true, data: JSON.parse(JSON.stringify(data)) }
        } catch (err) {
          result = { ok: false, code: codeOf(err), ...(step.msg ? { error: (err as Error).message } : {}) }
        }
        out[channel].push({ name: sc.name, step: i, args: list, result })
      }
      await svc.dispose()
    }
    for (const channel of ['search', 'activity'] as const) check(join(GOLDEN_DIR, 'channels', `${channel}.json`), stable({ scenarios: out[channel] }))
  }, 180_000)
})

// ── LFS·gitflow: git:repoExtras · git:lfsPaths ────────────────

type ExtrasStep = { git: string[] } | { write: [string, string] } | { extras: true } | { lfsPaths: unknown[]; msg?: boolean }
interface ExtrasScenario {
  name: string
  repo: string
  steps: ExtrasStep[]
}

/** 골든에 남기는 인자: 요청의 paths 가 20개를 넘으면 {count, first, last} 로 줄인다. Rust 테스트(channels.rs 의 `summarize_lfs_args`)도 같은 규칙이다 */
function summarizeLfsArgs(list: unknown[]): unknown[] {
  const req = list[0] as { paths?: unknown } | null | undefined
  if (!req || typeof req !== 'object' || !Array.isArray(req.paths) || req.paths.length <= 20) return list
  const p = req.paths
  return [{ ...req, paths: { count: p.length, first: p[0], last: p[p.length - 1] } }, ...list.slice(1)]
}

describe('LFS·gitflow 채널 골든(channels/{repoExtras,lfsPaths}.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'extras.scenarios.json'), 'utf8')) as { scenarios: ExtrasScenario[] }).scenarios
  let root: string
  const git = (cwd: string, args: string[], input?: Buffer): string =>
    execFileSync('git', args, { cwd, input, env: { ...process.env, ...REFS_STEP_ENV, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-chanextras-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const out: Record<'repoExtras' | 'lfsPaths', unknown[]> = { repoExtras: [], lfsPaths: [] }
    for (const [si, sc] of scenarios.entries()) {
      const dir = join(root, `s${si}`)
      mkdirSync(dir)
      git(dir, ['init', '-q', '-b', 'main'])
      git(dir, ['fast-import', '--quiet'], readFileSync(join(FIXTURES, `${sc.repo}.fi`)))
      const config = new ConfigStore(join(root, `s${si}.config.json`))
      const id = config.addRepo({ sourceId: 'local', path: dir, name: sc.repo }).id
      const svc = new RepoService(config)
      const rev = (expr: string): string => git(dir, ['rev-parse', expr])
      for (const [i, step] of sc.steps.entries()) {
        if ('git' in step) {
          git(dir, expand(step.git, rev) as string[])
          continue
        }
        if ('write' in step) {
          const [path, content] = expand(step.write, rev) as [string, string]
          mkdirSync(dirname(join(dir, path)), { recursive: true })
          writeFileSync(join(dir, path), content)
          continue
        }
        let result: Record<string, unknown>
        if ('extras' in step) {
          try {
            result = { ok: true, data: JSON.parse(JSON.stringify(await svc.repoExtras(id))) }
          } catch (err) {
            result = { ok: false, code: codeOf(err) }
          }
          out.repoExtras.push({ name: sc.name, step: i, result })
          continue
        }
        const list = expand(step.lfsPaths, rev) as unknown[]
        try {
          const data = JSON.parse(JSON.stringify(await svc.lfsPaths(id, list[0] as never, `req-${i}`))) as string[]
          result = { ok: true, data: data.length > 20 ? { count: data.length, first: data[0], last: data[data.length - 1] } : data }
        } catch (err) {
          result = { ok: false, code: codeOf(err), ...(step.msg ? { error: (err as Error).message } : {}) }
        }
        out.lfsPaths.push({ name: sc.name, step: i, args: summarizeLfsArgs(list), result })
      }
      await svc.dispose()
    }
    for (const channel of ['repoExtras', 'lfsPaths'] as const) check(join(GOLDEN_DIR, 'channels', `${channel}.json`), stable({ scenarios: out[channel] }))
  }, 180_000)
})

// ── git:fetch ──────────────────────────────────────────

interface FetchStep {
  in?: 'origin' | 'repo'
  git?: string[]
  fetch?: { manual?: unknown; confirm?: number | null; during?: { in: 'origin' | 'repo'; git: string[] } }
}
interface FetchScenario {
  name: string
  server?: 'auth401'
  /** 'ssh' 면 repo 를 SSH 서버 너머의 원격 저장소로 둔다. ssh 는 그 서버의 이상(refused: 닫힌 포트, hostkey: known_hosts 와 다른 호스트 키) */
  target?: 'ssh'
  ssh?: 'refused' | 'hostkey'
  steps: FetchStep[]
}

/** target:'ssh' 시나리오의 환경: 임시 홈·SSH 서버·SshExecutor. RepoService 의 실행기 공장은 실제 ~/.ssh 를 읽으므로 fetch 가 쓰는 두 메서드만 이 실행기로 바꾼다 */
async function sshRig(
  config: ConfigStore,
  repo: string,
  anomaly: 'refused' | 'hostkey' | undefined
): Promise<{ id: string; svc: RepoService; close: () => Promise<void> }> {
  const clientKey = newKeyPair()
  const server: TestServer = await startServer({ authorized: [clientKey] })
  const port = server.port
  if (anomaly === 'refused') await server.close()
  const env = makeHome(clientKey)
  writeFileSync(env.configPath, `Host test
  HostName 127.0.0.1
  Port ${port}
  User tester
  IdentityFile ${env.keyPath}
  IdentitiesOnly yes
`)
  writeFileSync(env.knownHostsPath, knownHostsLine(port, anomaly === 'hostkey' ? newKeyPair() : server.hostKey))
  const exec = new SshExecutor('test', { configPath: env.configPath, knownHostsPath: env.knownHostsPath, agent: null, homeDir: env.home })
  const queue = new ProcessQueue(1)
  class SshRepos extends RepoService {
    override fetchContext(): { exec: SshExecutor; path: string; queue: ProcessQueue } {
      return { exec, path: repo, queue }
    }
    override signature(_repoId: string, signal?: AbortSignal): Promise<string> {
      return refsSignature(exec, repo, { signal })
    }
  }
  const id = config.addRepo({ sourceId: 'ssh:test', path: repo, name: 'repo' }, { id: 'ssh:test', kind: 'ssh', label: 'SSH: test', sshHost: 'test' }).id
  return { id, svc: new SshRepos(config), close: async () => { await exec.dispose(); if (anomaly !== 'refused') await server.close() } }
}

/** 상태에서 비교할 것만 남긴다: 시각(lastOkAt)은 유무만, 오류 문구는 유무와 file 전송 안내 여부만(문구 패리티는 하지 않는다) */
function fetchState(st: {
  fetching: boolean
  lastOkAt: number | null
  error: string | null
  blocked: { keys: string[]; confirmable: boolean } | null
  authPaused: boolean
  needsManual: boolean
  noRemote: boolean
}): Record<string, unknown> {
  return {
    fetching: st.fetching,
    lastOk: st.lastOkAt !== null,
    error: st.error !== null,
    fileHint: st.error?.startsWith('자동 fetch 는 로컬 경로(file) remote') ?? false,
    blocked: st.blocked,
    authPaused: st.authPaused,
    needsManual: st.needsManual,
    noRemote: st.noRemote
  }
}

describe('git:fetch 채널 골든(channels/fetch.json)', () => {
  const scenarios = (JSON.parse(readFileSync(join(FIXTURES, 'fetch.scenarios.json'), 'utf8')) as { scenarios: FetchScenario[] }).scenarios
  let root: string
  const git = (cwd: string, args: string[]): string =>
    execFileSync('git', args, { cwd, env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } }).toString('utf8').trim()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-fetch-chan-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('시나리오 결과가 골든과 같다', async () => {
    const results: unknown[] = []
    for (const [n, sc] of scenarios.entries()) {
      const dir = join(root, `s${n}`)
      mkdirSync(dir)
      const origin = join(dir, 'origin')
      mkdirSync(origin)
      git(origin, ['init', '-q', '-b', 'main'])
      execFileSync('git', ['fast-import', '--quiet'], { cwd: origin, input: readFileSync(join(FIXTURES, 'basic.fi')), env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } })
      const repo = join(dir, 'repo')
      execFileSync('git', ['clone', '-q', origin, repo], { env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' } })

      let server: ReturnType<typeof createServer> | null = null
      let port = 0
      if (sc.server === 'auth401') {
        server = createServer((_req, res) => {
          res.writeHead(401, { 'WWW-Authenticate': 'Basic realm="x"' })
          res.end()
        })
        await new Promise<void>((r) => server!.listen(0, '127.0.0.1', r))
        port = (server.address() as AddressInfo).port
      }
      const run = (s: { in: 'origin' | 'repo'; git: string[] }): void => {
        git(s.in === 'origin' ? origin : repo, s.git.map((a) => a.replace('{port}', String(port))))
      }

      const config = new ConfigStore(join(dir, 'config.json'))
      let id: string
      let svc: RepoService
      let closeRig = async (): Promise<void> => {}
      if (sc.target === 'ssh') {
        const rig = await sshRig(config, repo, sc.ssh)
        ;({ id, svc } = rig)
        closeRig = rig.close
      } else {
        id = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' }).id
        svc = new RepoService(config)
      }
      let changed = 0
      let confirms = 0
      let reply: number | null = 2
      let during: { in: 'origin' | 'repo'; git: string[] } | undefined
      const approvals: string[] = []
      const setApproval = config.setFetchApproval.bind(config)
      config.setFetchApproval = (...a) => (approvals.push('approval'), setApproval(...a))
      const setTrusted = config.setTrustedConfig.bind(config)
      config.setTrustedConfig = (...a) => (approvals.push('trusted'), setTrusted(...a))
      const fetcher = new FetchService(config, svc, () => {}, () => void (changed += 1), join(dir, 'no-hooks'))

      const steps: unknown[] = []
      for (const step of sc.steps) {
        if (step.git) {
          run({ in: step.in!, git: step.git })
          continue
        }
        const f = step.fetch!
        reply = f.confirm === undefined ? 2 : f.confirm
        during = f.during
        changed = 0
        confirms = 0
        approvals.length = 0
        // src/main/ipc/handlers.ts 의 git:fetch 와 confirmDangerousConfig 의 해석: manual 은 정확히 true 만, 버튼 0 → 허용, 1 → 항상, 그 밖 → 거부
        const st = await fetcher.fetch(id, {
          manual: f.manual === true,
          confirm: async () => {
            confirms += 1
            if (during) run(during)
            return reply === 0 ? true : reply === 1 ? 'always' : false
          }
        })
        steps.push({ manual: f.manual ?? null, confirm: f.confirm ?? null, state: fetchState(st), confirms, changed, approvals: [...approvals] })
      }
      fetcher.dispose()
      await svc.dispose()
      await closeRig()
      await new Promise<void>((r) => (server ? server.close(() => r()) : r()))
      results.push({ name: sc.name, steps })
    }
    check(join(GOLDEN_DIR, 'channels', 'fetch.json'), stable({ scenarios: results }))
  }, 180_000)
})
