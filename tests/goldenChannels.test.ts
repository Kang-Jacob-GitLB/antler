import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { logArgs } from '../src/main/git/commands'
import { RepoService, StaleLogError } from '../src/main/services/RepoService'
import type { LogQuery } from '../src/shared/types'
import { IPC_CHANNELS } from '../src/shared/ipc'

// 채널 패리티 골든. TS(RepoService)가 기준이고, Rust 라우터(src-tauri/tests/channels.rs)가 같은 fixture·시나리오로 같은 결과를 내는지 본다.
//  - tests/golden/exec/channels.json: IPC_CHANNELS 목록. Rust 의 RUST_CHANNELS 가 이것의 부분집합인지 Rust 테스트가 강제한다
//  - tests/golden/channels/log.json: git:log 시나리오(tests/fixtures/repos/log.scenarios.json — 다단계: 페이지·범위·저장소 조작). logArgs.json: logArgs 의 argv(logArgs.cases.json)
//  - tests/golden/channels/{compare,fileDiff,fileHistory,blame}.json: 파일 단위 채널 시나리오(tests/fixtures/repos/files.scenarios.json)
//  - tests/golden/channels/commit.json: git:commit 시나리오(tests/fixtures/repos/commit.scenarios.json)의 결과 envelope.
//    오류는 code 만 남긴다(문구 패리티는 하지 않는다). 해시는 .fi 가 날짜·내용을 고정해 결정적이다.
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
