import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import type { GhRepoRef } from '../src/shared/types'
import { IPC_CHANNELS } from '../src/shared/ipc'
import { AbortError } from '../src/main/executors/GitExecutor'
import { GitHubApiError } from '../src/main/github/client'
import { GhCliError, type GhCli } from '../src/main/github/ghCli'
import { GitHubService } from '../src/main/github/service'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { registerGithubIpc, type IpcHandle } from '../src/main/ipc/github'
import type { RepoService } from '../src/main/services/RepoService'
import { startFakeJira, type FakeJira, type RecordedReq } from './jiraFake'

// gh:* 채널 골든(Phase 4 PR 9). TS 가 기준이다: 실제 src/main/ipc/github.ts 의 핸들러(인자 처리)와 GitHubService 를 가짜 GitHub(루프백)에 붙여
// tests/fixtures/net/github.scenarios.json 의 단계를 돌리고, 단계마다 봉투(오류는 code 만)와 서버가 받은 요청(method·path·정렬한 query·Authorization 종류·본문)을
// tests/golden/channels/github.json 에 남긴다. Rust(src-tauri/tests/channels.rs 의 `github_채널_시나리오가_ts_골든과_같다`)가 라우터로 같은 시나리오를 돌려 같은 결과를 내는지 본다.
// 골든 갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenGithubChannels.test.ts`
// 서비스가 사이드카에 묻는 저장소 정보(RepoService)와 gh CLI 는 시나리오의 fake 로 대신한다(Rust 는 RepoQuery·ExternalTokenProvider 의 가짜).

const { openExternal } = vi.hoisted(() => ({ openExternal: vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()) }))
vi.mock('electron', () => ({ shell: { openExternal } }))

const SCENARIOS = join(__dirname, 'fixtures', 'net', 'github.scenarios.json')
const GOLDEN = join(__dirname, 'golden', 'channels', 'github.json')
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 키를 정렬하고 2칸·LF 로 쓴다(goldenChannels.test.ts 와 같은 모양) */
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

interface MockResponse {
  status: number
  body?: unknown
  base64?: string
  headers?: Record<string, string>
  delayMs?: number
}
interface MockRoute {
  method: string
  path: string
  query?: Record<string, string>
  responses: MockResponse[]
}
interface FakeState {
  clientId: string
  repoIds: string[]
  repos: Record<string, { ref: GhRepoRef | null; remotes: GhRepoRef[]; pushed: string[] }>
  cli: { available: boolean; token?: string; error?: string }
}
interface Step {
  name: string
  ch: string
  args: unknown[]
  routes?: MockRoute[]
  restart?: boolean
  fake?: Partial<FakeState>
  cancel?: { afterRequests: number; id: string }
}
interface Scenarios {
  fake: FakeState
  routes: MockRoute[]
  steps: Step[]
}

/** 평문과 구분되는 가짜 암호화(jiraService.test.ts 와 같다) */
function fakeSafe(): SafeStorageLike {
  return {
    isEncryptionAvailable: () => true,
    encryptString: (s) => Buffer.concat([Buffer.from('ENC:'), Buffer.from(s, 'utf8').map((b) => b ^ 0x5a)]),
    decryptString: (b) => Buffer.from(b.subarray(4).map((x) => x ^ 0x5a)).toString('utf8')
  }
}

/** 가짜 서버의 경로표. 일치 규칙은 시나리오 파일 머리의 _doc 과 Rust 의 목 서버가 같다 */
class RouteTable {
  private readonly hits = new Map<MockRoute, number>()
  stepRoutes: MockRoute[] = []
  constructor(private readonly base: MockRoute[]) {}

  find(method: string, url: string): { response: MockResponse } | null {
    const u = new URL(url, 'http://x')
    for (const route of [...this.stepRoutes, ...this.base]) {
      if (route.method !== method || route.path !== u.pathname) continue
      if (!Object.entries(route.query ?? {}).every(([k, v]) => u.searchParams.get(k) === v)) continue
      const n = this.hits.get(route) ?? 0
      this.hits.set(route, n + 1)
      return { response: route.responses[Math.min(n, route.responses.length - 1)]! }
    }
    return null
  }

  resetStep(routes: MockRoute[]): void {
    for (const r of this.stepRoutes) this.hits.delete(r)
    this.stepRoutes = routes
  }
}

/** handlers.ts 의 handle() 이 예외를 봉투로 바꾸는 순서(ABORTED → GH_* code → ERROR). electron 을 불러와 직접 쓸 수 없어 같은 규칙을 적고, 아래 테스트가 원본과 대조한다 */
function envelopeOf(err: unknown): { ok: false; error: string; code: string } {
  const code = err instanceof AbortError ? 'ABORTED' : err instanceof GitHubApiError ? err.code : 'ERROR'
  return { ok: false, error: err instanceof Error ? err.message : String(err), code }
}

const deep = (v: unknown, fn: (s: string) => string): unknown => {
  if (typeof v === 'string') return fn(v)
  if (Array.isArray(v)) return v.map((x) => deep(x, fn))
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, deep(x, fn)]))
  return v
}

/** 시계에서 나오는 값(Date.now)은 골든에서 가린다 */
const maskTime = (v: unknown): unknown => {
  if (Array.isArray(v)) return v.map(maskTime)
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, k === 'fetchedAt' ? '<time>' : maskTime(x)]))
  return v
}

describe('gh:* 채널 골든(channels/github.json)', () => {
  let dir: string
  let fake: FakeJira

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-gh-chan-'))
  })
  afterAll(async () => {
    await fake?.close()
    rmSync(dir, { recursive: true, force: true })
  })

  it('handlers.ts 의 봉투 규칙이 이 테스트가 적은 순서와 같다', () => {
    const src = readFileSync(join(__dirname, '..', 'src', 'main', 'ipc', 'handlers.ts'), 'utf8')
    const at = (s: string): number => src.indexOf(s)
    expect(at('err instanceof AbortError')).toBeGreaterThan(-1)
    expect(at('err instanceof AbortError')).toBeLessThan(at('err instanceof GitHubApiError || err instanceof JiraApiError'))
    expect(at('err instanceof GitHubApiError || err instanceof JiraApiError')).toBeLessThan(at("'ERROR'"))
    expect(src).toContain('? err.code')
  })

  it('gh 채널 전부(gh:repo 는 사이드카가 처리해 제외)가 IPC_CHANNELS 에 있고 시나리오가 모두 다룬다', () => {
    const sc = JSON.parse(readFileSync(SCENARIOS, 'utf8')) as Scenarios
    const gh = IPC_CHANNELS.filter((c) => c.startsWith('gh:') && c !== 'gh:repo')
    expect(gh.length).toBe(23)
    const used = new Set(sc.steps.map((s) => s.ch))
    for (const c of gh) expect(used.has(c), `${c} 시나리오가 없다`).toBe(true)
    expect(used.has('gh:repo')).toBe(false)
  })

  it('시나리오 결과가 골든과 같다', async () => {
    const sc = JSON.parse(readFileSync(SCENARIOS, 'utf8')) as Scenarios
    const table = new RouteTable(sc.routes)
    fake = await startFakeJira((req, res) => {
      const found = table.find(req.method, req.url)
      const r = found?.response ?? { status: 404, body: { message: 'Not Found' } }
      const send = (): void => {
        const headers: Record<string, string> = { ...(r.base64 === undefined && r.body !== undefined && typeof r.body !== 'string' ? { 'content-type': 'application/json' } : {}), ...(r.headers ?? {}) }
        res.writeHead(r.status, headers)
        if (r.base64 !== undefined) res.end(Buffer.from(r.base64, 'base64'))
        else if (r.body === undefined) res.end()
        else res.end(typeof r.body === 'string' ? r.body : JSON.stringify(r.body))
      }
      if (r.delayMs) setTimeout(send, r.delayMs)
      else send()
    })
    const origin = fake.origin
    const sub = (s: string): string => s.replaceAll('{base}', origin)
    // 운영 주소(api.github.com·github.com)로 가는 요청을 같은 목 서버로 보낸다. 두 호스트의 경로는 겹치지 않는다
    const fetchImpl: typeof fetch = (input, init) => fetch(String(input).replace(/^https:\/\/(api\.)?github\.com/, origin), init)

    const state: FakeState = structuredClone(sc.fake)
    const reposFake = {
      ghRepo: (id: string): Promise<GhRepoRef | null> => Promise.resolve(state.repos[id]?.ref ?? null),
      ghRemotes: (id: string): Promise<GhRepoRef[]> => Promise.resolve(state.repos[id]?.remotes ?? []),
      pushedShasByAuthor: (id: string, _email: string, limit: number): Promise<string[]> => Promise.resolve((state.repos[id]?.pushed ?? []).slice(0, limit)),
      invalidateGhRemotes: (): void => undefined
    }
    const cli: GhCli = {
      available: () => Promise.resolve(state.cli.available),
      token: () => (state.cli.error ? Promise.reject(new GhCliError(state.cli.error, 'GH_CLI_NOT_LOGGED_IN')) : Promise.resolve(state.cli.token ?? ''))
    }
    const store = new TokenStore(dir, fakeSafe())
    const make = (): { gh: GitHubService; handlers: Map<string, (...a: unknown[]) => unknown> } => {
      const gh = new GitHubService({
        config: { get: () => ({ ui: { githubClientId: state.clientId }, repos: state.repoIds.map((id) => ({ id })) }) },
        repos: reposFake,
        store,
        fetchImpl,
        sleep: () => Promise.resolve(),
        cli
      })
      const handlers = new Map<string, (...a: unknown[]) => unknown>()
      const handle: IpcHandle = (channel, fn) => {
        handlers.set(channel, (...a) => (fn as unknown as (e: never, ...x: unknown[]) => unknown)({} as never, ...a))
      }
      registerGithubIpc(handle, gh, reposFake as unknown as RepoService)
      return { gh, handlers }
    }
    let svc = make()

    const out: unknown[] = []
    for (const step of sc.steps) {
      Object.assign(state, step.fake)
      if (step.restart) {
        svc.gh.dispose()
        svc = make()
      }
      table.resetStep(step.routes ?? [])
      const from = fake.requests.length
      openExternal.mockClear()
      const args = (deep(step.args, sub) as unknown[]).map((a) => (a !== null && typeof a === 'object' && !Array.isArray(a) && Object.keys(a).length === 1 && '$undefined' in a ? undefined : a))
      // 취소는 시간이 아니라 목 서버가 요청을 받은 뒤에 보낸다(느린 러너에서도 요청 목록이 같다)
      let canceller: Promise<void> | undefined
      if (step.cancel) {
        const { afterRequests, id } = step.cancel
        canceller = (async () => {
          const deadline = Date.now() + 10_000
          while (fake.requests.length < from + afterRequests && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5))
          svc.gh.cancel(id)
        })()
      }
      const fn = svc.handlers.get(step.ch)
      expect(fn, `${step.ch} 핸들러`).toBeDefined()
      let env: { ok: boolean; data?: unknown; code?: string }
      try {
        env = { ok: true, data: await fn!(...args) }
      } catch (err) {
        env = envelopeOf(err)
      }
      await canceller
      const unsub = (s: string): string => s.replaceAll(origin, '{base}')
      const envelope = JSON.parse(unsub(JSON.stringify(maskTime(env.ok ? { ok: true, data: env.data } : { ok: false, code: env.code })) ?? 'null')) as unknown
      const requests = fake.requests.slice(from).map((r: RecordedReq) => {
        const u = new URL(r.url, 'http://x')
        const query = [...u.searchParams.entries()].sort(([ak, av], [bk, bv]) => (ak < bk ? -1 : ak > bk ? 1 : av < bv ? -1 : av > bv ? 1 : 0))
        const auth = /^(Basic|Bearer) /.exec(r.headers.authorization ?? '')?.[1] ?? null
        return { method: r.method, path: u.pathname, query, auth, ...(r.body ? { body: JSON.parse(r.body) as unknown } : {}) }
      })
      requests.sort((a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : JSON.stringify(a) > JSON.stringify(b) ? 1 : 0))
      out.push({ name: step.name, envelope, requests, ...(step.ch === 'gh:openExternal' ? { opened: openExternal.mock.calls.map((c) => unsub(c[0])) } : {}) })
    }
    svc.gh.dispose()

    const got = stable({ steps: out })
    if (UPDATE) {
      mkdirSync(join(GOLDEN, '..'), { recursive: true })
      writeFileSync(GOLDEN, got)
      return
    }
    expect(existsSync(GOLDEN), 'UPDATE_GOLDEN=1 npx vitest run tests/goldenGithubChannels.test.ts 로 골든을 만든다').toBe(true)
    expect(got).toBe(readFileSync(GOLDEN, 'utf8').replace(/\r\n/g, '\n'))
  }, 60_000)
})
