import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { avatarBaseUrl, avatarPx, avatarSrc, normalizeAvatarUrl, parseNoreply } from '../src/shared/avatar'
import { AvatarResolver } from '../src/main/github/avatars'
import { GitHubClient } from '../src/main/github/client'
import { GitHubService } from '../src/main/github/service'
import { parseAvatarAuthors } from '../src/main/github/validate'
import { fakeFetch, json } from './ghFake'

const SHA_A = 'a'.repeat(40)
const SHA_B = 'b'.repeat(40)
const SHA_C = 'c'.repeat(40)
const slug = { owner: 'octo', repo: 'hello' }
const RATE = (remaining: number, reset = 1900000000) => ({ 'x-ratelimit-remaining': String(remaining), 'x-ratelimit-limit': '60', 'x-ratelimit-reset': String(reset) })

describe('parseNoreply', () => {
  it('<id>+<login>@users.noreply.github.com', () => {
    expect(parseNoreply('12345+octocat@users.noreply.github.com')).toEqual({ login: 'octocat', id: 12345 })
  })
  it('<login>@users.noreply.github.com (옛 형식, ID 없음)', () => {
    expect(parseNoreply('octocat@users.noreply.github.com')).toEqual({ login: 'octocat' })
  })
  it('대소문자·앞뒤 공백 허용, 봇은 ID 가 있어야 한다', () => {
    expect(parseNoreply(' 7+Foo-Bar@Users.Noreply.GitHub.com ')).toEqual({ login: 'Foo-Bar', id: 7 })
    expect(parseNoreply('49699333+dependabot[bot]@users.noreply.github.com')).toEqual({ login: 'dependabot[bot]', id: 49699333 })
    expect(parseNoreply('dependabot[bot]@users.noreply.github.com')).toBeNull()
  })
  it('다른 도메인·잘못된 로그인은 null', () => {
    expect(parseNoreply('me@example.com')).toBeNull()
    expect(parseNoreply('me@users.noreply.github.com.evil.com')).toBeNull()
    expect(parseNoreply('evil@x@users.noreply.github.com')).toBeNull()
    expect(parseNoreply('-bad@users.noreply.github.com')).toBeNull()
    expect(parseNoreply('a/b@users.noreply.github.com')).toBeNull()
    expect(parseNoreply('@users.noreply.github.com')).toBeNull()
  })
})

describe('아바타 주소', () => {
  it('ID 가 있으면 /u/<id>, 없으면 /<login>', () => {
    expect(avatarBaseUrl('octocat', 5)).toBe('https://avatars.githubusercontent.com/u/5')
    expect(avatarBaseUrl('octocat')).toBe('https://avatars.githubusercontent.com/octocat')
  })
  it('API avatar_url 은 호스트를 고정하고 쿼리를 버린다', () => {
    expect(normalizeAvatarUrl('https://avatars.githubusercontent.com/u/9?v=4', 'x', 9)).toBe('https://avatars.githubusercontent.com/u/9')
    expect(normalizeAvatarUrl('https://avatars.githubusercontent.com/u/9?v=4', 'x')).toBe('https://avatars.githubusercontent.com/u/9')
    expect(normalizeAvatarUrl('https://evil.example/u/9', 'x', 9)).toBeNull()
    expect(normalizeAvatarUrl('http://avatars.githubusercontent.com/u/9', 'x', 9)).toBeNull()
    expect(normalizeAvatarUrl(undefined, 'x')).toBeNull()
  })
  it('요청 크기는 몇 단계로 묶인다', () => {
    expect(avatarPx(18, 1)).toBe(32)
    expect(avatarPx(18, 2)).toBe(48)
    expect(avatarPx(32, 2)).toBe(64)
    expect(avatarPx(500, 1)).toBe(192)
    expect(avatarSrc('https://avatars.githubusercontent.com/u/1', 48)).toBe('https://avatars.githubusercontent.com/u/1?s=48')
  })
})

describe('parseAvatarAuthors (IPC 인자 검증)', () => {
  it('정상 입력을 정규화한다(sha 소문자, 이메일 중복 제거)', () => {
    expect(parseAvatarAuthors([{ email: 'A@x.io', sha: SHA_A.toUpperCase() }, { email: 'a@X.io', sha: SHA_B }])).toEqual([{ email: 'A@x.io', sha: SHA_A }])
    expect(parseAvatarAuthors([{ email: 'a@x.io', sha: 'f'.repeat(64) }])).toHaveLength(1)
    expect(parseAvatarAuthors([])).toEqual([])
  })
  it('형식이 틀리면 거부', () => {
    expect(() => parseAvatarAuthors('x')).toThrow()
    expect(() => parseAvatarAuthors(null)).toThrow()
    expect(() => parseAvatarAuthors([null])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a@x.io', sha: 'abc' }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a@x.io', sha: '../' + 'a'.repeat(37) }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a@x.io', sha: `${SHA_A.slice(0, 39)}g` }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a\n@x.io', sha: SHA_A }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a'.repeat(255), sha: SHA_A }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 7, sha: SHA_A }])).toThrow()
    expect(() => parseAvatarAuthors([{ email: 'a@x.io' }])).toThrow()
  })
  it('200건을 넘으면 거부', () => {
    const many = Array.from({ length: 201 }, (_, i) => ({ email: `u${i}@x.io`, sha: SHA_A }))
    expect(() => parseAvatarAuthors(many)).toThrow()
    expect(parseAvatarAuthors(many.slice(0, 200))).toHaveLength(200)
  })
})

describe('AvatarResolver', () => {
  let dir: string
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-avatar-'))
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  const signal = new AbortController().signal
  const commit = (sha: string, author: unknown) => json([{ sha, author }], { headers: RATE(50) })

  it('noreply 는 네트워크 없이 풀고, GitHub 저장소가 아니어도 동작한다', async () => {
    const { fetch, calls } = fakeFetch(() => undefined)
    const r = new AvatarResolver(new GitHubClient(fetch))
    const out = await r.resolve(null, [{ email: '12+me@users.noreply.github.com', sha: SHA_A }, { email: 'x@y.io', sha: SHA_B }], signal)
    expect(out).toEqual({ '12+me@users.noreply.github.com': { login: 'me', url: 'https://avatars.githubusercontent.com/u/12' } })
    expect(calls).toHaveLength(0)
  })

  it('sha → login 매핑, author:null 은 null, 이메일은 GitHub 로 보내지 않는다', async () => {
    const { fetch, calls } = fakeFetch((req) => {
      const sha = new URL(req.url).searchParams.get('sha')
      if (sha === SHA_A) return commit(SHA_A, { login: 'octocat', id: 583231, avatar_url: 'https://avatars.githubusercontent.com/u/583231?v=4' })
      if (sha === SHA_B) return commit(SHA_B, null)
      return undefined
    })
    const r = new AvatarResolver(new GitHubClient(fetch))
    const out = await r.resolve(slug, [{ email: 'octo@corp.io', sha: SHA_A }, { email: 'ghost@corp.io', sha: SHA_B }], signal)
    expect(out['octo@corp.io']).toEqual({ login: 'octocat', url: 'https://avatars.githubusercontent.com/u/583231' })
    expect(out['ghost@corp.io']).toBeNull()
    expect(calls).toHaveLength(2)
    for (const c of calls) {
      const u = new URL(c.url)
      expect(u.origin + u.pathname).toBe('https://api.github.com/repos/octo/hello/commits')
      expect(u.searchParams.get('per_page')).toBe('1')
      expect(c.url).not.toContain('corp.io')
      expect(c.url).not.toContain('%40')
    }
  })

  it('캐시 적중이면 다시 호출하지 않고, 디스크 캐시는 재시작 뒤에도 쓴다(이메일은 해시로만 저장)', async () => {
    const file = join(dir, 'avatar-cache.json')
    const { fetch, calls } = fakeFetch(() => commit(SHA_A, { login: 'octocat', id: 1, avatar_url: 'https://avatars.githubusercontent.com/u/1?v=4' }))
    const r1 = new AvatarResolver(new GitHubClient(fetch), file)
    await r1.resolve(slug, [{ email: 'octo@corp.io', sha: SHA_A }], signal)
    await r1.resolve(slug, [{ email: 'OCTO@corp.io', sha: SHA_A }], signal)
    expect(calls).toHaveLength(1)
    r1.flush()
    expect(readFileSync(file, 'utf8')).not.toContain('corp.io')

    const again = fakeFetch(() => undefined)
    const r2 = new AvatarResolver(new GitHubClient(again.fetch), file)
    const out = await r2.resolve(slug, [{ email: 'octo@corp.io', sha: SHA_A }], signal)
    expect(out['octo@corp.io']?.login).toBe('octocat')
    expect(again.calls).toHaveLength(0)
  })

  it('TTL: 부정은 7일, 긍정은 30일 뒤 다시 조회한다', async () => {
    let t = 1_000_000_000_000
    const { fetch, calls } = fakeFetch((req) => (new URL(req.url).searchParams.get('sha') === SHA_A ? commit(SHA_A, null) : commit(SHA_B, { login: 'me', id: 2, avatar_url: '' })))
    const r = new AvatarResolver(new GitHubClient(fetch), undefined, () => t)
    const authors = [{ email: 'n@x.io', sha: SHA_A }, { email: 'p@x.io', sha: SHA_B }]
    await r.resolve(slug, authors, signal)
    expect(calls).toHaveLength(2)
    t += 6 * 86_400_000
    await r.resolve(slug, authors, signal)
    expect(calls).toHaveLength(2)
    t += 2 * 86_400_000
    await r.resolve(slug, authors, signal)
    expect(calls).toHaveLength(3)
    t += 25 * 86_400_000
    await r.resolve(slug, authors, signal)
    expect(calls).toHaveLength(5)
  })

  it('미푸시 커밋(404)은 기록하지 않고 결과에서 뺀다(잠시 뒤 다시)', async () => {
    let t = 1_000_000_000_000
    const { fetch, calls } = fakeFetch(() => json({ message: 'Not Found' }, { status: 404, headers: RATE(50) }))
    const r = new AvatarResolver(new GitHubClient(fetch), undefined, () => t)
    const a = [{ email: 'x@corp.io', sha: SHA_A }]
    expect(await r.resolve(slug, a, signal)).toEqual({})
    await r.resolve(slug, a, signal)
    expect(calls).toHaveLength(1)
    t += 11 * 60_000
    await r.resolve(slug, a, signal)
    expect(calls).toHaveLength(2)
  })

  it('남은 호출이 10 미만이면 멈추고 reset 이후 재개한다', async () => {
    let t = 1_900_000_000_000 - 3_600_000
    const reset = 1_900_000_000
    const { fetch, calls } = fakeFetch((req) => {
      const sha = new URL(req.url).searchParams.get('sha')!
      return json([{ sha, author: { login: `u${calls.length}`, id: calls.length, avatar_url: '' } }], { headers: RATE(9, reset) })
    })
    const r = new AvatarResolver(new GitHubClient(fetch), undefined, () => t)
    const first = await r.resolve(slug, [{ email: '1@x.io', sha: SHA_A }, { email: '2@x.io', sha: SHA_B }, { email: '3@x.io', sha: SHA_C }], signal)
    // 동시성 3 이라 첫 묶음은 모두 나갈 수 있다. 중요한 것은 이후 새 요청이 한도 때문에 막힌다는 점
    expect(calls.length).toBeGreaterThan(0)
    const n = calls.length
    const second = await r.resolve(slug, [{ email: '9@x.io', sha: 'd'.repeat(40) }], signal)
    expect(second).toEqual({})
    expect(calls).toHaveLength(n)
    expect(Object.keys(first).length).toBe(n)
    t = (reset + 1) * 1000
    await r.resolve(slug, [{ email: '9@x.io', sha: 'd'.repeat(40) }], signal)
    expect(calls).toHaveLength(n + 1)
  })

  it('429/403 한도 초과 응답이면 일시 중지한다', async () => {
    const { fetch, calls } = fakeFetch(() => json({ message: 'rate limited' }, { status: 403, headers: RATE(0) }))
    const r = new AvatarResolver(new GitHubClient(fetch))
    expect(await r.resolve(slug, [{ email: '1@x.io', sha: SHA_A }], signal)).toEqual({})
    await r.resolve(slug, [{ email: '2@x.io', sha: SHA_B }], signal)
    expect(calls).toHaveLength(1)
  })

  it('동시 호출을 3개로 제한하고, 같은 작성자의 동시 요청은 합친다', async () => {
    let active = 0
    let peak = 0
    const { fetch, calls } = fakeFetch(async (req) => {
      active++
      peak = Math.max(peak, active)
      await new Promise((r) => setTimeout(r, 15))
      active--
      const sha = new URL(req.url).searchParams.get('sha')!
      return commit(sha, { login: 'u', id: 1, avatar_url: '' })
    })
    const r = new AvatarResolver(new GitHubClient(fetch))
    const shas = Array.from({ length: 8 }, (_, i) => String(i).repeat(40))
    const authors = shas.map((sha, i) => ({ email: `u${i}@x.io`, sha }))
    await Promise.all([r.resolve(slug, authors, signal), r.resolve(slug, authors, signal)])
    expect(peak).toBeLessThanOrEqual(3)
    expect(calls).toHaveLength(8)
  })

  it('취소되면 더 호출하지 않는다', async () => {
    const ac = new AbortController()
    ac.abort()
    const { fetch, calls } = fakeFetch(() => undefined)
    const r = new AvatarResolver(new GitHubClient(fetch))
    expect(await r.resolve(slug, [{ email: 'a@x.io', sha: SHA_A }], ac.signal)).toEqual({})
    expect(calls).toHaveLength(0)
  })
})

describe('GitHubService.avatars', () => {
  const store = { load: () => null, save: () => true, clear: () => {}, storageUnavailable: false } as unknown as ConstructorParameters<typeof GitHubService>[0]['store']
  const config = { get: () => ({ ui: {} }) }

  it('GitHub remote 가 있으면 sha 로 조회하고 결과에 토큰·이메일 조회가 새지 않는다', async () => {
    const { fetch, calls } = fakeFetch(() => json([{ sha: SHA_A, author: { login: 'octocat', id: 1, avatar_url: 'https://avatars.githubusercontent.com/u/1?v=4' } }]))
    const svc = new GitHubService({ config, store, repos: { ghRepo: async () => ({ owner: 'octo', repo: 'hello', remote: 'origin' }) }, fetchImpl: fetch })
    const res = await svc.avatars('r1', 'req-1', [{ email: 'o@corp.io', sha: SHA_A }])
    expect(res).toEqual({ github: true, avatars: { 'o@corp.io': { login: 'octocat', url: 'https://avatars.githubusercontent.com/u/1' } } })
    expect(calls).toHaveLength(1)
  })

  it('GitHub 저장소가 아니면 github:false, noreply 는 그래도 풀린다', async () => {
    const { fetch, calls } = fakeFetch(() => undefined)
    const svc = new GitHubService({ config, store, repos: { ghRepo: async () => null }, fetchImpl: fetch })
    const res = await svc.avatars('r1', 'req-2', [{ email: '3+me@users.noreply.github.com', sha: SHA_A }, { email: 'o@corp.io', sha: SHA_B }])
    expect(res.github).toBe(false)
    expect(Object.keys(res.avatars)).toEqual(['3+me@users.noreply.github.com'])
    expect(calls).toHaveLength(0)
  })
})
