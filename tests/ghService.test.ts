import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { GitHubService } from '../src/main/github/service'
import { META_FILE, TOKEN_FILE, TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { allowedExternalUrl, parseListQuery, parseNumber, parsePat } from '../src/main/github/validate'
import { fakeFetch, json } from './ghFake'

/** 평문과 구분되는 가짜 암호화(XOR + 접두사) */
function fakeSafe(available = true): SafeStorageLike & { available: boolean } {
  const self = {
    available,
    isEncryptionAvailable: () => self.available,
    encryptString: (s: string) => Buffer.concat([Buffer.from('ENC:'), Buffer.from(s, 'utf8').map((b) => b ^ 0x5a)]),
    decryptString: (b: Buffer) => {
      if (b.subarray(0, 4).toString() !== 'ENC:') throw new Error('bad ciphertext')
      return Buffer.from(b.subarray(4).map((x) => x ^ 0x5a)).toString('utf8')
    }
  }
  return self
}

let dir: string
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-gh-token-'))
})
afterEach(() => rmSync(dir, { recursive: true, force: true }))

describe('TokenStore', () => {
  it('암호화해서 저장하고 다시 읽는다(평문이 디스크에 없다)', () => {
    const store = new TokenStore(dir, fakeSafe())
    expect(store.save('ghp_secret', 'device')).toBe(true)
    expect(readFileSync(join(dir, TOKEN_FILE)).toString('latin1')).not.toContain('ghp_secret')
    expect(readFileSync(join(dir, META_FILE), 'utf8')).not.toContain('ghp_secret')
    expect(readdirSync(dir).some((f) => f.endsWith('.tmp'))).toBe(false)
    // 새 인스턴스(앱 재시작)
    expect(new TokenStore(dir, fakeSafe()).load()).toEqual({ token: 'ghp_secret', method: 'device' })
  })

  it('암호화를 못 쓰면 메모리에만 두고 파일을 만들지 않는다', () => {
    const store = new TokenStore(dir, fakeSafe(false))
    expect(store.storageUnavailable).toBe(true)
    expect(store.save('ghp_secret', 'pat')).toBe(false)
    expect(readdirSync(dir)).toEqual([])
    expect(store.load()).toEqual({ token: 'ghp_secret', method: 'pat' })
    expect(new TokenStore(dir, fakeSafe(false)).load()).toBeNull()
  })

  it('키링 없는 Linux(basic_text)는 암호화가 "가능"해도 디스크에 두지 않는다', () => {
    const safe = { ...fakeSafe(), getSelectedStorageBackend: () => 'basic_text' }
    const linux = new TokenStore(dir, safe, 'linux')
    expect(linux.storageUnavailable).toBe(true)
    expect(linux.save('ghp_secret', 'pat')).toBe(false)
    expect(readdirSync(dir)).toEqual([])
    // 키링이 있는 Linux 와 다른 OS 는 저장한다
    expect(new TokenStore(dir, { ...fakeSafe(), getSelectedStorageBackend: () => 'gnome_libsecret' }, 'linux').storageUnavailable).toBe(false)
    expect(new TokenStore(dir, safe, 'win32').storageUnavailable).toBe(false)
  })

  it('logout(clear)은 파일과 메모리를 지운다', () => {
    const store = new TokenStore(dir, fakeSafe())
    store.save('t', 'pat')
    store.clear()
    expect(existsSync(join(dir, TOKEN_FILE))).toBe(false)
    expect(existsSync(join(dir, META_FILE))).toBe(false)
    expect(store.load()).toBeNull()
    const mem = new TokenStore(dir, fakeSafe(false))
    mem.save('t', 'pat')
    mem.clear()
    expect(mem.load()).toBeNull()
  })

  it('깨진 파일은 null 을 돌려주고 지운다', () => {
    writeFileSync(join(dir, TOKEN_FILE), 'garbage')
    const store = new TokenStore(dir, fakeSafe())
    expect(store.load()).toBeNull()
    expect(existsSync(join(dir, TOKEN_FILE))).toBe(false)
  })
})

describe('GitHubService 인증', () => {
  const repos = { ghRepo: async () => ({ owner: 'octo', repo: 'hello', remote: 'origin' }) }
  const config = (githubClientId?: string) => ({ get: () => ({ ui: { githubClientId } }) })

  it('PAT 로그인: /user 로 검증하고 저장하며 토큰은 상태에 없다', async () => {
    const { fetch, calls } = fakeFetch((req) => (req.headers['authorization'] === 'Bearer ghp_good' ? json({ login: 'me', avatar_url: 'https://a/me.png' }, { headers: { 'x-ratelimit-remaining': '4999', 'x-ratelimit-limit': '5000', 'x-ratelimit-reset': '1900000000' } }) : json({ message: 'Bad credentials' }, { status: 401 })))
    const store = new TokenStore(dir, fakeSafe())
    const svc = new GitHubService({ config: config(), repos, store, fetchImpl: fetch })

    await expect(svc.loginPat('ghp_bad')).rejects.toThrow('올바르지 않')
    expect(store.load()).toBeNull()
    expect((await svc.auth()).signedIn).toBe(false)

    const state = await svc.loginPat('  ghp_good  ')
    expect(state).toEqual({ signedIn: true, user: { login: 'me', avatarUrl: 'https://a/me.png' }, method: 'pat', rateRemaining: 4999, rateLimit: 5000, rateReset: 1900000000 })
    expect(JSON.stringify(state)).not.toContain('ghp_good')
    expect(store.load()).toEqual({ token: 'ghp_good', method: 'pat' })
    expect(calls.at(-1)!.url).toBe('https://api.github.com/user')

    const out = svc.logout()
    expect(out).toEqual({ signedIn: false })
    expect(store.load()).toBeNull()
  })

  it('저장된 토큰은 첫 auth 에서 한 번만 검증한다', async () => {
    const store = new TokenStore(dir, fakeSafe())
    store.save('ghp_saved', 'device')
    const { fetch, calls } = fakeFetch(() => json({ login: 'me', avatar_url: 'x' }))
    const svc = new GitHubService({ config: config(), repos, store, fetchImpl: fetch })
    expect(calls).toHaveLength(0)
    const [a, b] = await Promise.all([svc.auth(), svc.auth()])
    await svc.auth()
    expect(calls).toHaveLength(1)
    expect(a).toMatchObject({ signedIn: true, method: 'device', user: { login: 'me' } })
    expect(b).toEqual(a)
  })

  it('저장된 토큰이 거부되면 버리고 로그아웃 상태로 보인다', async () => {
    const store = new TokenStore(dir, fakeSafe())
    store.save('ghp_old', 'pat')
    const { fetch } = fakeFetch(() => json({ message: 'Bad credentials' }, { status: 401 }))
    const svc = new GitHubService({ config: config(), repos, store, fetchImpl: fetch })
    expect((await svc.auth()).signedIn).toBe(false)
    expect(store.load()).toBeNull()
  })

  it('네트워크 오류로 검증하지 못하면 로그인 상태를 유지한다', async () => {
    const store = new TokenStore(dir, fakeSafe())
    store.save('ghp_saved', 'pat')
    const fetch = (async () => {
      throw new TypeError('fetch failed')
    }) as unknown as typeof globalThis.fetch
    const svc = new GitHubService({ config: config(), repos, store, fetchImpl: fetch })
    const s = await svc.auth()
    expect(s).toEqual({ signedIn: true, method: 'pat' })
    expect(store.load()).not.toBeNull()
  })

  it('safeStorage 불가 환경은 storageUnavailable 을 알린다', async () => {
    const store = new TokenStore(dir, fakeSafe(false))
    const { fetch } = fakeFetch(() => json({ login: 'me', avatar_url: 'x' }))
    const svc = new GitHubService({ config: config(), repos, store, fetchImpl: fetch })
    expect((await svc.loginPat('ghp_x')).storageUnavailable).toBe(true)
  })

  it('Client ID 가 없으면 Device Flow 시작을 거부한다', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    const svc = new GitHubService({ config: config(), repos, store: new TokenStore(dir, fakeSafe()), fetchImpl: fetch })
    await expect(svc.deviceStart()).rejects.toThrow('Client ID')
    await expect(svc.devicePoll('r1')).rejects.toThrow('진행 중인 로그인이 없습니다')
    expect(calls).toHaveLength(0)
  })

  it('Device Flow 전체: start → poll → 로그인(device)', async () => {
    const { fetch } = fakeFetch((req) => {
      if (req.url.endsWith('/login/device/code')) return json({ device_code: 'dc', user_code: 'U-1', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 })
      if (req.url.endsWith('/login/oauth/access_token')) return json({ access_token: 'gho_tok' })
      if (req.url.endsWith('/user')) return json({ login: 'me', avatar_url: 'x' })
      return undefined
    })
    const store = new TokenStore(dir, fakeSafe())
    const svc = new GitHubService({ config: config('cid'), repos, store, fetchImpl: fetch, sleep: async () => {} })
    const code = await svc.deviceStart()
    expect(code).toEqual({ userCode: 'U-1', verificationUri: 'https://github.com/login/device', expiresIn: 900, interval: 5 })
    const state = await svc.devicePoll('req-1')
    expect(state).toMatchObject({ signedIn: true, method: 'device', user: { login: 'me' } })
    expect(JSON.stringify(state)).not.toContain('gho_tok')
    expect(store.load()).toEqual({ token: 'gho_tok', method: 'device' })
  })

  it('devicePoll 은 cancel(requestId) 로 취소된다', async () => {
    const { fetch } = fakeFetch((req) => {
      if (req.url.endsWith('/login/device/code')) return json({ device_code: 'dc', user_code: 'U', verification_uri: 'v', expires_in: 900, interval: 5 })
      return json({ error: 'authorization_pending' })
    })
    const svc = new GitHubService({ config: config('cid'), repos, store: new TokenStore(dir, fakeSafe()), fetchImpl: fetch })
    await svc.deviceStart()
    const p = svc.devicePoll('req-2')
    const caught = p.catch((e) => e)
    svc.cancel('req-2')
    expect((await caught).name).toBe('AbortError')
  })

  it('GitHub 가 아닌 저장소는 조회를 거부한다', async () => {
    const { fetch, calls } = fakeFetch(() => json([]))
    const svc = new GitHubService({ config: config(), repos: { ghRepo: async () => null }, store: new TokenStore(dir, fakeSafe()), fetchImpl: fetch })
    await expect(svc.pulls('r', { state: 'open' })).rejects.toThrow('GitHub')
    expect(calls).toHaveLength(0)
  })

  it('저장소 조회는 owner/repo 를 경로에 쓴다', async () => {
    const { fetch, calls } = fakeFetch(() => json([]))
    const svc = new GitHubService({ config: config(), repos, store: new TokenStore(dir, fakeSafe()), fetchImpl: fetch })
    await svc.issues('r', { state: 'open' })
    expect(new URL(calls[0]!.url).pathname).toBe('/repos/octo/hello/issues')
  })
})

describe('openExternal 허용 목록', () => {
  it('github.com https 만 통과', () => {
    expect(allowedExternalUrl('https://github.com/octo/hello/pull/1')).toBe('https://github.com/octo/hello/pull/1')
    expect(allowedExternalUrl('https://github.com')).toBe('https://github.com/')
    expect(allowedExternalUrl('https://github.com:443/x')).toBe('https://github.com/x')
  })
  it.each([
    'http://github.com/x',
    'https://github.com.evil.com/x',
    'https://evil.com/github.com/',
    'https://github.com@evil.com/',
    'https://evil.com\\@github.com/',
    'https://user:pw@github.com/x',
    'https://www.github.com/x',
    'https://api.github.com/x',
    'https://github.com:8443/x',
    'javascript:alert(1)',
    'file:///c:/windows/system32/calc.exe',
    'ssh://git@github.com/x',
    '//github.com/x',
    'github.com/x',
    '',
    `https://github.com/${'a'.repeat(3000)}`
  ])('거부: %s', (u) => {
    expect(allowedExternalUrl(u)).toBeNull()
  })
  it('문자열이 아니면 거부', () => {
    expect(allowedExternalUrl(undefined)).toBeNull()
    expect(allowedExternalUrl({ toString: () => 'https://github.com/' })).toBeNull()
  })
})

describe('IPC 입력 검증', () => {
  it('번호는 1 이상 1e9 미만의 정수', () => {
    expect(parseNumber(1)).toBe(1)
    expect(parseNumber(999_999_999)).toBe(999_999_999)
    for (const bad of [0, -1, 1.5, 1e9, NaN, Infinity, '1', null, undefined]) expect(() => parseNumber(bad)).toThrow()
  })

  it('목록 질의 정규화', () => {
    expect(parseListQuery({ state: 'open' })).toEqual({ state: 'open', page: 1 })
    expect(parseListQuery({ state: 'all', label: 'bug', assignee: 'none', page: 3 })).toEqual({ state: 'all', label: 'bug', assignee: 'none', page: 3 })
    expect(parseListQuery({ state: 'closed', label: '' })).toEqual({ state: 'closed', page: 1 })
    for (const bad of [null, {}, { state: 'merged' }, { state: 'open', label: 'a'.repeat(101) }, { state: 'open', label: 'a\nb' }, { state: 'open', assignee: 'a\0b' }, { state: 'open', label: 5 }, { state: 'open', page: 0 }, { state: 'open', page: 1.5 }, { state: 'open', page: 'x' }]) {
      expect(() => parseListQuery(bad)).toThrow()
    }
    expect(parseListQuery({ state: 'open', label: 'a'.repeat(100) }).label).toHaveLength(100)
  })

  it('PAT: 공백 제거, 255자 이하, 내부 공백·제어 문자 거부', () => {
    expect(parsePat('  ghp_abc123  ')).toBe('ghp_abc123')
    expect(parsePat('a'.repeat(255))).toHaveLength(255)
    for (const bad of ['', '   ', 'a'.repeat(256), 'ghp abc', 'ghp\tabc', 'ghp\nabc', 'ghp\0abc', '토큰', 5, null]) expect(() => parsePat(bad)).toThrow()
  })
})
