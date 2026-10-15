import { describe, expect, it } from 'vitest'
import { AbortError } from '../src/main/executors/GitExecutor'
import { GitHubApiError, GitHubClient } from '../src/main/github/client'
import { fakeFetch, json } from './ghFake'

const rateHeaders = (remaining: number, reset = 1_900_000_000, extra: Record<string, string> = {}) => ({
  'x-ratelimit-remaining': String(remaining),
  'x-ratelimit-limit': '60',
  'x-ratelimit-reset': String(reset),
  ...extra
})

describe('GitHubClient 요청 헤더', () => {
  it('고정 헤더를 보내고 토큰이 있으면 Bearer 를 붙인다', async () => {
    const { fetch, calls } = fakeFetch(() => json({ ok: true }))
    const anon = new GitHubClient(fetch)
    await anon.get('/repos/o/r/pulls', { query: { state: 'open', page: 2, skip: undefined } })
    expect(calls[0]!.url).toBe('https://api.github.com/repos/o/r/pulls?state=open&page=2')
    expect(calls[0]!.headers).toMatchObject({ accept: 'application/vnd.github+json', 'x-github-api-version': '2022-11-28', 'user-agent': 'Antler' })
    expect(calls[0]!.headers['authorization']).toBeUndefined()

    const authed = new GitHubClient(fetch, () => 'tok123')
    await authed.get('/user')
    expect(calls[1]!.headers['authorization']).toBe('Bearer tok123')
  })

  it('html 옵션이면 html Accept', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    await new GitHubClient(fetch).get('/repos/o/r/issues/1', { html: true })
    expect(calls[0]!.headers['accept']).toBe('application/vnd.github.html+json')
  })

  it('API 밖으로 나가는 경로는 거부한다(문자열 결합이라 호스트를 바꿀 수 없다)', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    const c = new GitHubClient(fetch)
    await expect(c.get('@evil.com/x')).rejects.toThrow()
    await expect(c.get('https://evil.com/x')).rejects.toThrow()
    expect(calls).toHaveLength(0)
  })
})

describe('ETag 캐시', () => {
  it('304 면 캐시된 본문을 돌려주고 If-None-Match 를 보낸다', async () => {
    const { fetch, calls } = fakeFetch((req) =>
      req.headers['if-none-match'] === '"v1"' ? new Response(null, { status: 304 }) : json([{ n: 1 }], { headers: { etag: '"v1"', link: '<https://api.github.com/x?page=2>; rel="next"' } })
    )
    const c = new GitHubClient(fetch)
    const a = await c.get<{ n: number }[]>('/x')
    const b = await c.get<{ n: number }[]>('/x')
    expect(calls[0]!.headers['if-none-match']).toBeUndefined()
    expect(calls[1]!.headers['if-none-match']).toBe('"v1"')
    expect(b.data).toEqual([{ n: 1 }])
    expect(b.hasMore).toBe(true)
    expect(a.data).toBe(b.data)
  })

  it('토큰·URL·Accept 가 다르면 캐시를 공유하지 않는다', async () => {
    const { fetch, calls } = fakeFetch(() => json({}, { headers: { etag: '"e"' } }))
    let token: string | null = 'a'
    const c = new GitHubClient(fetch, () => token)
    await c.get('/x')
    token = 'b'
    await c.get('/x')
    token = null
    await c.get('/x')
    token = 'a'
    await c.get('/x', { html: true })
    await c.get('/x', { query: { page: 2 } })
    expect(calls.every((r) => r.headers['if-none-match'] === undefined)).toBe(true)
    await c.get('/x')
    expect(calls[5]!.headers['if-none-match']).toBe('"e"')
  })

  it('clearCache 후에는 조건부 요청을 보내지 않는다', async () => {
    const { fetch, calls } = fakeFetch(() => json({}, { headers: { etag: '"e"' } }))
    const c = new GitHubClient(fetch)
    await c.get('/x')
    c.clearCache()
    await c.get('/x')
    expect(calls[1]!.headers['if-none-match']).toBeUndefined()
  })
})

describe('속도 제한·오류', () => {
  it('남은 횟수를 기록한다(search 는 core 표시를 덮어쓰지 않는다)', async () => {
    const seen: number[] = []
    const { fetch } = fakeFetch((req) =>
      json({}, { headers: req.url.includes('/search/') ? rateHeaders(29, 1, { 'x-ratelimit-resource': 'search' }) : rateHeaders(4999, 1_900_000_000, { 'x-ratelimit-resource': 'core' }) })
    )
    const c = new GitHubClient(fetch, () => 't', (r) => seen.push(r.remaining))
    await c.get('/x')
    await c.get('/search/issues')
    expect(c.rate).toEqual({ remaining: 4999, limit: 60, reset: 1_900_000_000 })
    expect(seen).toEqual([4999])
  })

  it('403 + remaining=0 이면 한도 초과 메시지(초기화 시각 포함)', async () => {
    const { fetch } = fakeFetch(() => json({ message: 'API rate limit exceeded' }, { status: 403, headers: rateHeaders(0) }))
    const err = await new GitHubClient(fetch).get('/x').catch((e) => e)
    expect(err).toBeInstanceOf(GitHubApiError)
    expect(err.code).toBe('GH_RATE_LIMIT')
    expect(err.message).toContain('호출 한도를 초과했습니다')
    expect(err.message).toContain('로그인하면')
    const signed = await new GitHubClient(fetch, () => 't').get('/x').catch((e) => e)
    expect(signed.message).not.toContain('로그인하면')
  })

  it('429 retry-after 는 대기 시간을 알려 준다', async () => {
    const { fetch } = fakeFetch(() => json({}, { status: 429, headers: { 'retry-after': '42' } }))
    const err = await new GitHubClient(fetch).get('/x').catch((e) => e)
    expect(err.code).toBe('GH_RATE_LIMIT')
    expect(err.message).toContain('42초')
  })

  it('한도와 무관한 403 은 접근 거부', async () => {
    const { fetch } = fakeFetch(() => json({ message: 'Resource protected by organization SAML enforcement' }, { status: 403, headers: rateHeaders(10) }))
    const err = await new GitHubClient(fetch).get('/x').catch((e) => e)
    expect(err.code).toBe('GH_FORBIDDEN')
    expect(err.message).toContain('SAML')
  })

  it('404 / 401 메시지와 코드', async () => {
    const f404 = fakeFetch(() => json({ message: 'Not Found' }, { status: 404 }))
    const e404 = await new GitHubClient(f404.fetch).get('/x').catch((e) => e)
    expect(e404.code).toBe('GH_NOT_FOUND')
    expect(e404.message).toBe('저장소를 찾을 수 없거나 접근 권한이 없습니다(비공개 저장소면 로그인 필요)')

    const f401 = fakeFetch(() => json({ message: 'Bad credentials' }, { status: 401 }))
    const e401 = await new GitHubClient(f401.fetch, () => 'secret-token').get('/x').catch((e) => e)
    expect(e401.code).toBe('GH_AUTH')
    expect(e401.message).toContain('GitHub 인증이 만료되었거나 잘못되었습니다')
    expect(e401.message).not.toContain('secret-token')
  })

  it('네트워크 오류 메시지에 토큰이 실리지 않는다', async () => {
    const fetch = (async () => {
      throw new TypeError('fetch failed')
    }) as unknown as typeof globalThis.fetch
    const err = await new GitHubClient(fetch, () => 'secret-token').get('/x').catch((e) => e)
    expect(err.code).toBe('GH_NETWORK')
    expect(JSON.stringify(err.message)).not.toContain('secret-token')
  })

  it('호출자가 취소하면 AbortError', async () => {
    const { fetch } = fakeFetch(() => json({}))
    const ac = new AbortController()
    ac.abort()
    await expect(new GitHubClient(fetch).get('/x', { signal: ac.signal })).rejects.toBeInstanceOf(AbortError)
  })
})

describe('Link 페이지네이션', () => {
  it('rel="next" 가 있으면 hasMore', async () => {
    const link = '<https://api.github.com/r?page=2>; rel="next", <https://api.github.com/r?page=5>; rel="last"'
    const more = fakeFetch(() => json([], { headers: { link } }))
    expect((await new GitHubClient(more.fetch).get('/r')).hasMore).toBe(true)
    const last = fakeFetch(() => json([], { headers: { link: '<https://api.github.com/r?page=1>; rel="prev", <https://api.github.com/r?page=1>; rel="first"' } }))
    expect((await new GitHubClient(last.fetch).get('/r')).hasMore).toBe(false)
    const none = fakeFetch(() => json([]))
    expect((await new GitHubClient(none.fetch).get('/r')).hasMore).toBe(false)
  })
})
