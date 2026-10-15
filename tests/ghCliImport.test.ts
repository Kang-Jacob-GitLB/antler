// GitHub CLI 로그인 가져오기 · 아바타 조회 차단 사유 · 비로그인 단기 캐시.
// 실제 gh 는 부르지 않는다: 서비스 테스트는 가짜 GhCli 를, 실행 래퍼 테스트는 node 자신(process.execPath)을 gh 대신 쓴다.
// (Windows 에서 셸 없이 .cmd 를 실행하면 EINVAL 이라 가짜 gh.cmd 는 쓸 수 없다)
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AvatarResolver } from '../src/main/github/avatars'
import { GitHubClient } from '../src/main/github/client'
import { createGhCli, GhCliError, ghEnv, realExec, type GhCli } from '../src/main/github/ghCli'
import { GitHubService } from '../src/main/github/service'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { fakeFetch, json } from './ghFake'

const SHA = (c: string): string => c.repeat(40)
const slug = { owner: 'octo', repo: 'hello' }
const RATE = (remaining: number, reset = 1900000000) => ({ 'x-ratelimit-remaining': String(remaining), 'x-ratelimit-limit': '60', 'x-ratelimit-reset': String(reset) })

const safe: SafeStorageLike = {
  isEncryptionAvailable: () => true,
  encryptString: (s) => Buffer.concat([Buffer.from('ENC:'), Buffer.from(s, 'utf8').map((b) => b ^ 0x5a)]),
  decryptString: (b) => Buffer.from(b.subarray(4).map((x) => x ^ 0x5a)).toString('utf8')
}

let dir: string
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-gh-cli-'))
})
afterEach(() => rmSync(dir, { recursive: true, force: true }))

describe('createGhCli', () => {
  const env = (): NodeJS.ProcessEnv => ghEnv()

  it('gh 가 없으면 available=false, token 은 설치 안내 에러', async () => {
    const cli = createGhCli(undefined, env, 'antler-no-such-gh-binary')
    expect(await cli.available()).toBe(false)
    await expect(cli.token()).rejects.toMatchObject({ code: 'GH_CLI_MISSING' })
  })

  it('실행 결과를 trim 하고, 인자는 auth token --hostname github.com 으로 고정한다', async () => {
    const seen: string[][] = []
    const cli = createGhCli(async (_f, args) => {
      seen.push(args)
      return { stdout: '  gho_abc123\r\n' }
    })
    expect(await cli.token()).toBe('gho_abc123')
    expect(seen[0]).toEqual(['auth', 'token', '--hostname', 'github.com'])
  })

  it('available 은 한 번만 실행하고 캐시한다', async () => {
    let n = 0
    const cli = createGhCli(async () => {
      n++
      return { stdout: 'gh version 2' }
    })
    expect(await Promise.all([cli.available(), cli.available()])).toEqual([true, true])
    await cli.available()
    expect(n).toBe(1)
  })

  it('비정상 종료·빈 출력은 "로그인 안 됨" — stderr 를 메시지에 싣지 않는다', async () => {
    const failing = createGhCli(async () => {
      throw Object.assign(new Error('exit 1: gho_LEAK'), { code: 1, stderr: 'gho_LEAK' })
    })
    const err = await failing.token().catch((e: unknown) => e)
    expect(err).toBeInstanceOf(GhCliError)
    expect((err as GhCliError).code).toBe('GH_CLI_NOT_LOGGED_IN')
    expect((err as GhCliError).message).not.toContain('gho_LEAK')
    await expect(createGhCli(async () => ({ stdout: '\n' })).token()).rejects.toMatchObject({ code: 'GH_CLI_NOT_LOGGED_IN' })
  })

  it('realExec: 셸 없이 실행하고 stdout 을 돌려준다', async () => {
    const out = await realExec(process.execPath, ['-e', 'process.stdout.write("  gho_real\\n")'], { env: ghEnv(), timeout: 10_000, maxBuffer: 4096 })
    expect(out.stdout.trim()).toBe('gho_real')
    // 셸 메타 문자가 해석되지 않는다
    const lit = await realExec(process.execPath, ['-e', 'process.stdout.write(process.argv[1])', '--', '$(echo hi) && x'], { env: ghEnv(), timeout: 10_000, maxBuffer: 4096 })
    expect(lit.stdout).toBe('$(echo hi) && x')
  })

  it('realExec: 시간 초과면 중단한다', async () => {
    await expect(realExec(process.execPath, ['-e', 'setTimeout(()=>{},10000)'], { env: ghEnv(), timeout: 300, maxBuffer: 4096 })).rejects.toMatchObject({ killed: true })
  })

  it('ghEnv: PATH/Path 처럼 대소문자만 다른 이름은 하나만 넘긴다', () => {
    const e = ghEnv({ PATH: '/a', Path: '/b' })
    expect(Object.keys(e).filter((k) => k.toLowerCase() === 'path')).toHaveLength(1)
  })

  it('ghEnv: 필요한 변수만 넘기고 토큰 환경 변수는 넘기지 않는다', () => {
    const e = ghEnv({ PATH: '/bin', HOME: '/h', GH_TOKEN: 'secret', GITHUB_TOKEN: 'secret2', AWS_SECRET_ACCESS_KEY: 'x' })
    expect(e).toMatchObject({ PATH: '/bin', HOME: '/h', GH_PROMPT_DISABLED: '1', NO_COLOR: '1' })
    expect(Object.keys(e)).not.toContain('GH_TOKEN')
    expect(Object.keys(e)).not.toContain('GITHUB_TOKEN')
    expect(Object.keys(e)).not.toContain('AWS_SECRET_ACCESS_KEY')
  })
})

describe('GitHubService.importCliToken', () => {
  const repos = { ghRepo: async () => ({ owner: 'octo', repo: 'hello', remote: 'origin' }) }
  const config = { get: () => ({ ui: {} }) }
  const fakeCli = (impl: () => Promise<string>, available = true): GhCli => ({ available: async () => available, token: impl })
  const user = { login: 'me', avatar_url: 'https://a/me.png' }

  it('토큰을 검증·저장하고 method=gh-cli 로 로그인한다(상태에 토큰이 없다)', async () => {
    const store = new TokenStore(dir, safe)
    const { fetch, calls } = fakeFetch((req) => (req.headers['authorization'] === 'Bearer gho_good' ? json(user, { headers: RATE(4999) }) : json({ message: 'Bad credentials' }, { status: 401 })))
    const svc = new GitHubService({ config, repos, store, fetchImpl: fetch, cli: fakeCli(async () => 'gho_good') })
    expect(await svc.auth()).toMatchObject({ signedIn: false, cliAvailable: true })
    const state = await svc.importCliToken()
    expect(state).toMatchObject({ signedIn: true, method: 'gh-cli', user: { login: 'me' }, cliAvailable: true })
    expect(JSON.stringify(state)).not.toContain('gho_good')
    expect(calls.at(-1)!.url).toBe('https://api.github.com/user')
    // 재시작 뒤에도 방식이 gh-cli 로 남는다
    expect(new TokenStore(dir, safe).load()).toEqual({ token: 'gho_good', method: 'gh-cli' })
  })

  it('gh 가 없으면 cliAvailable=false, 가져오기는 친절한 에러', async () => {
    const store = new TokenStore(dir, safe)
    const cli = createGhCli(undefined, () => ghEnv(), 'antler-no-such-gh-binary')
    const svc = new GitHubService({ config, repos, store, fetchImpl: fakeFetch(() => undefined).fetch, cli })
    expect((await svc.auth()).cliAvailable).toBe(false)
    await expect(svc.importCliToken()).rejects.toThrow('설치되어 있지 않습니다')
    expect(store.load()).toBeNull()
  })

  it('gh 에 로그인 안 되어 있으면 에러(저장하지 않음)', async () => {
    const store = new TokenStore(dir, safe)
    const cli = createGhCli(async () => {
      throw Object.assign(new Error('x'), { code: 1 })
    })
    const svc = new GitHubService({ config, repos, store, fetchImpl: fakeFetch(() => undefined).fetch, cli })
    await expect(svc.importCliToken()).rejects.toThrow('gh auth login')
    expect(store.load()).toBeNull()
  })

  it('GitHub 가 토큰을 거부하면 저장하지 않고, 에러에 토큰이 없다', async () => {
    const store = new TokenStore(dir, safe)
    const { fetch } = fakeFetch(() => json({ message: 'Bad credentials' }, { status: 401 }))
    const svc = new GitHubService({ config, repos, store, fetchImpl: fetch, cli: fakeCli(async () => 'gho_dead') })
    const err = (await svc.importCliToken().catch((e: unknown) => e)) as Error
    expect(err.message).toContain('올바르지 않')
    expect(err.message).not.toContain('gho_dead')
    expect(store.load()).toBeNull()
  })

  it('형식이 이상한 출력(공백 포함)은 거부한다', async () => {
    const svc = new GitHubService({ config, repos, store: new TokenStore(dir, safe), fetchImpl: fakeFetch(() => undefined).fetch, cli: fakeCli(async () => 'not a token') })
    await expect(svc.importCliToken()).rejects.toThrow('형식')
  })

  it('cli 가 없으면 사용할 수 없다', async () => {
    const svc = new GitHubService({ config, repos, store: new TokenStore(dir, safe), fetchImpl: fakeFetch(() => undefined).fetch })
    await expect(svc.importCliToken()).rejects.toThrow()
    expect('cliAvailable' in (await svc.auth())).toBe(false)
  })
})

describe('AvatarResolver 차단 사유', () => {
  const signal = new AbortController().signal

  it('비로그인에서 404 만 쌓이고 저장소도 404 면 needs-login — 더 묻지 않고 디스크에 부정 결과를 남기지 않는다', async () => {
    const file = join(dir, 'avatar.json')
    const { fetch, calls } = fakeFetch((req) => {
      const u = new URL(req.url)
      return u.pathname === '/repos/octo/hello/commits' || u.pathname === '/repos/octo/hello' ? json({ message: 'Not Found' }, { status: 404, headers: RATE(50) }) : undefined
    })
    const r = new AvatarResolver(new GitHubClient(fetch), file)
    const authors = ['a', 'b', 'c', 'd'].map((c) => ({ email: `${c}@x.io`, sha: SHA(c) }))
    // 한 묶음에 10명: 비공개가 확정된 뒤에는 대기 중이던 나머지를 호출하지 않는다(동시성 슬롯 몫만 더 나간다)
    const batch = 'abcdefghij'.split('').map((c) => ({ email: `${c}@x.io`, sha: SHA(c) }))
    const out = await r.resolve(slug, batch, signal)
    expect(out).toEqual({})
    expect(calls.length).toBeLessThanOrEqual(7)
    expect(r.blockedFor(slug)).toEqual({ reason: 'needs-login', signedIn: false })
    const n = calls.length
    await r.resolve(slug, authors, signal)
    expect(calls.length).toBe(n) // 더 묻지 않는다
    r.flush()
    expect(() => readFileSyncOrNull(file)).not.toThrow()
    expect(readFileSyncOrNull(file)).toBeNull() // 404 는 디스크에 저장하지 않는다
    // 로그인(세션 초기화 + 토큰) 뒤에는 다시 묻는다
    r.resetSession()
    expect(r.blockedFor(slug)).toBeNull()
  })

  it('공개 저장소의 미푸시 커밋 404 는 needs-login 이 아니다', async () => {
    const { fetch } = fakeFetch((req) => {
      const u = new URL(req.url)
      if (u.pathname === '/repos/octo/hello') return json({ full_name: 'octo/hello' }, { headers: RATE(50) })
      return json({ message: 'Not Found' }, { status: 404, headers: RATE(50) })
    })
    const r = new AvatarResolver(new GitHubClient(fetch))
    await r.resolve(slug, ['a', 'b', 'c'].map((c) => ({ email: `${c}@x.io`, sha: SHA(c) })), signal)
    expect(r.blockedFor(slug)).toBeNull()
  })

  it('한도 소진은 rate-limited 와 풀리는 시각, 로그인 후 resetSession 이 풀어 준다', async () => {
    const reset = Math.floor(Date.now() / 1000) + 1800
    const { fetch } = fakeFetch(() => json({ message: 'API rate limit exceeded' }, { status: 403, headers: RATE(0, reset) }))
    const r = new AvatarResolver(new GitHubClient(fetch))
    await r.resolve(slug, [{ email: 'a@x.io', sha: SHA('a') }], signal)
    expect(r.blockedFor(slug)).toEqual({ reason: 'rate-limited', resetAt: reset, signedIn: false })
  })

  it('세션 상한은 session-cap, resetSession 으로 풀린다', async () => {
    const { fetch } = fakeFetch((req) => json([{ sha: new URL(req.url).searchParams.get('sha'), author: null }], { headers: RATE(50) }))
    const r = new AvatarResolver(new GitHubClient(fetch))
    r.calls = 40
    expect(r.blockedFor(slug)).toEqual({ reason: 'session-cap', signedIn: false })
    r.resetSession()
    expect(r.blockedFor(slug)).toBeNull()
  })

  it('서비스: 로그인하면 차단 표시와 한도 정보가 초기화된다', async () => {
    const reset = Math.floor(Date.now() / 1000) + 1800
    let signedIn = false
    const { fetch } = fakeFetch((req) => {
      if (new URL(req.url).pathname === '/user') return json({ login: 'me', avatar_url: 'https://a/me.png' }, { headers: RATE(4999) })
      if (!req.headers['authorization']) return json({ message: 'API rate limit exceeded' }, { status: 403, headers: RATE(0, reset) })
      signedIn = true
      return json([{ sha: new URL(req.url).searchParams.get('sha'), author: { login: 'octocat', id: 5, avatar_url: '' } }], { headers: RATE(4990) })
    })
    const svc = new GitHubService({
      config: { get: () => ({ ui: {} }) },
      repos: { ghRepo: async () => ({ owner: 'octo', repo: 'hello', remote: 'origin' }) },
      store: new TokenStore(dir, safe),
      fetchImpl: fetch,
      cli: { available: async () => true, token: async () => 'gho_ok' }
    })
    const q = [{ email: 'a@x.io', sha: SHA('a') }]
    const blocked = await svc.avatars('r', 'req1', q)
    expect(blocked.avatars).toEqual({})
    expect(blocked.blocked).toMatchObject({ reason: 'rate-limited', resetAt: reset, signedIn: false })
    await svc.importCliToken()
    const after = await svc.avatars('r', 'req2', q)
    expect(signedIn).toBe(true)
    expect(after.blocked).toBeUndefined()
    expect(after.avatars['a@x.io']).toMatchObject({ login: 'octocat' })
  })
})

describe('비로그인 단기 캐시', () => {
  afterEach(() => vi.useRealTimers())

  it('3분 안의 같은 요청은 네트워크를 쓰지 않고, 지나면 조건부 요청을 보낸다. 로그인 상태는 항상 조건부', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    const { fetch, calls } = fakeFetch((req) => (req.headers['if-none-match'] === '"v1"' ? new Response(null, { status: 304 }) : json([1], { headers: { etag: '"v1"' } })))
    const anon = new GitHubClient(fetch)
    await anon.get('/x')
    await anon.get('/x')
    expect(calls).toHaveLength(1)
    vi.setSystemTime(Date.now() + 4 * 60_000)
    await anon.get('/x')
    expect(calls).toHaveLength(2)
    expect(calls[1]!.headers['if-none-match']).toBe('"v1"')

    const authed = new GitHubClient(fetch, () => 'tok')
    await authed.get('/x')
    await authed.get('/x')
    expect(calls).toHaveLength(4)
    expect(calls[3]!.headers['if-none-match']).toBe('"v1"')
  })
})

import { readFileSync } from 'node:fs'
function readFileSyncOrNull(file: string): string | null {
  try {
    return readFileSync(file, 'utf8')
  } catch {
    return null
  }
}
