import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService, StaleLogError } from '../src/main/services/RepoService'
import { IPC_CHANNELS } from '../src/shared/ipc'

// 채널 패리티 골든. TS(RepoService)가 기준이고, Rust 라우터(src-tauri/tests/channels.rs)가 같은 fixture·시나리오로 같은 결과를 내는지 본다.
//  - tests/golden/exec/channels.json: IPC_CHANNELS 목록. Rust 의 RUST_CHANNELS 가 이것의 부분집합인지 Rust 테스트가 강제한다
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
