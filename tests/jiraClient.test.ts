import { afterEach, describe, expect, it } from 'vitest'
import { AbortError } from '../src/main/executors/GitExecutor'
import { authHeader, JiraApiError, JiraClient, parseRetryAfter, type JiraCredential } from '../src/main/jira/client'
import { startFakeJira, sendJson, type FakeJira } from './jiraFake'

const SECRET = 'tok_SECRET_0123456789'
let fake: FakeJira | undefined
let other: FakeJira | undefined

afterEach(async () => {
  await fake?.close()
  await other?.close()
  fake = other = undefined
})

const cloud = (siteUrl: string): JiraCredential => ({ kind: 'cloud', siteUrl, email: 'me@example.com', token: SECRET })
const server = (siteUrl: string): JiraCredential => ({ kind: 'server', siteUrl, email: '', token: SECRET })
const clientFor = (c: JiraCredential, sleep?: (ms: number) => Promise<void>): JiraClient => new JiraClient(fetch, () => c, sleep)

describe('인증 헤더', () => {
  it('Cloud 는 Basic base64(이메일:토큰)', () => {
    expect(authHeader(cloud('https://x.atlassian.net'))).toBe(`Basic ${Buffer.from(`me@example.com:${SECRET}`).toString('base64')}`)
  })

  it('Data Center 는 Bearer 토큰', () => {
    expect(authHeader(server('https://jira.example.com'))).toBe(`Bearer ${SECRET}`)
  })

  it('요청에 정확한 헤더가 실리고 컨텍스트 경로가 유지된다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, { ok: true }))
    await clientFor(cloud(`${fake.origin}/jira`)).get('/rest/api/3/myself')
    const r = fake.requests[0]!
    expect(r.url).toBe('/jira/rest/api/3/myself')
    expect(r.headers['authorization']).toBe(`Basic ${Buffer.from(`me@example.com:${SECRET}`).toString('base64')}`)
    expect(r.method).toBe('GET')

    await clientFor(server(fake.origin)).get('/rest/api/2/myself')
    expect(fake.requests[1]!.headers['authorization']).toBe(`Bearer ${SECRET}`)
  })
})

describe('리다이렉트', () => {
  it('다른 호스트로 보내도 따라가지 않고, 그쪽에는 요청 자체가 가지 않는다', async () => {
    other = await startFakeJira((_req, res) => sendJson(res, { leaked: true }))
    fake = await startFakeJira((_req, res) => {
      res.writeHead(302, { location: `${other!.altOrigin}/steal` })
      res.end()
    })
    // fake 는 127.0.0.1, other.altOrigin 은 localhost — 호스트가 다르다
    const err = await clientFor(cloud(fake.origin)).get('/rest/api/3/myself').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(JiraApiError)
    expect((err as JiraApiError).code).toBe('JIRA_REDIRECT')
    expect(other.requests).toHaveLength(0)
    expect((err as Error).message).not.toContain(SECRET)
  })

  it('같은 호스트 안의 리다이렉트도 따라가지 않는다', async () => {
    fake = await startFakeJira((req, res) => {
      if (req.url === '/rest/api/3/myself') {
        res.writeHead(301, { location: '/elsewhere' })
        res.end()
      } else sendJson(res, {})
    })
    await expect(clientFor(cloud(fake.origin)).get('/rest/api/3/myself')).rejects.toMatchObject({ code: 'JIRA_REDIRECT' })
    expect(fake.requests).toHaveLength(1)
  })
})

describe('오류 처리', () => {
  it.each([
    [401, 'JIRA_AUTH'],
    [403, 'JIRA_FORBIDDEN'],
    [404, 'JIRA_NOT_FOUND'],
    [500, 'JIRA_HTTP']
  ])('HTTP %i → %s, 메시지에 토큰이 없다', async (status, code) => {
    fake = await startFakeJira((_req, res) => sendJson(res, { errorMessages: ['nope'] }, status))
    const err = await clientFor(cloud(fake.origin)).get('/rest/api/3/issue/ABC-1').catch((e: unknown) => e)
    expect(err).toMatchObject({ code, status })
    const text = JSON.stringify({ m: (err as Error).message, s: String((err as Error).stack) })
    expect(text).not.toContain(SECRET)
    expect(text).not.toContain(Buffer.from(`me@example.com:${SECRET}`).toString('base64'))
  })

  it('연결 실패 메시지에도 토큰이 없다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}))
    const dead = fake.origin
    await fake.close()
    fake = undefined
    const err = await clientFor(cloud(dead)).get('/rest/api/3/myself').catch((e: unknown) => e)
    expect(err).toMatchObject({ code: 'JIRA_NETWORK' })
    expect(JSON.stringify((err as Error).message)).not.toContain(SECRET)
  })

  it('본문이 JSON 이 아니면 해석 오류', async () => {
    fake = await startFakeJira((_req, res) => {
      res.writeHead(200)
      res.end('<html>')
    })
    await expect(clientFor(cloud(fake.origin)).get('/x')).rejects.toMatchObject({ code: 'JIRA_HTTP' })
  })

  it('이미 취소된 신호면 AbortError', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}))
    const ac = new AbortController()
    ac.abort()
    await expect(clientFor(cloud(fake.origin)).get('/x', { signal: ac.signal })).rejects.toBeInstanceOf(AbortError)
  })

  it('요청 중 취소하면 AbortError', async () => {
    fake = await startFakeJira(() => undefined) // 응답하지 않는다
    const ac = new AbortController()
    const p = clientFor(cloud(fake.origin)).get('/x', { signal: ac.signal })
    setTimeout(() => ac.abort(), 50)
    await expect(p).rejects.toBeInstanceOf(AbortError)
  })

  it('API 경로는 사이트 밖을 가리킬 수 없다', async () => {
    const c = clientFor(cloud('https://x.atlassian.net'))
    await expect(c.get('//evil.example/x')).rejects.toThrow('invalid api path')
    await expect(c.get('rest/api')).rejects.toThrow('invalid api path')
  })
})

describe('429 Retry-After', () => {
  it('짧으면 기다렸다 한 번 다시 시도한다', async () => {
    fake = await startFakeJira((_req, res, n) => (n === 1 ? sendJson(res, {}, 429, { 'retry-after': '2' }) : sendJson(res, { ok: 1 })))
    const waits: number[] = []
    const data = await clientFor(cloud(fake.origin), async (ms) => void waits.push(ms)).get<{ ok: number }>('/x')
    expect(data.ok).toBe(1)
    expect(waits).toEqual([2000])
    expect(fake.requests).toHaveLength(2)
  })

  it('길면 기다리지 않고 남은 시간을 알린다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 429, { 'retry-after': '120' }))
    const waits: number[] = []
    const err = await clientFor(cloud(fake.origin), async (ms) => void waits.push(ms)).get('/x').catch((e: unknown) => e)
    expect(err).toMatchObject({ code: 'JIRA_RATE_LIMIT', status: 429 })
    expect((err as Error).message).toContain('120초')
    expect(waits).toEqual([])
    expect(fake.requests).toHaveLength(1)
  })

  it('다시 시도해도 429 면 거기서 멈춘다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 429, { 'retry-after': '1' }))
    await expect(clientFor(cloud(fake.origin), async () => undefined).get('/x')).rejects.toMatchObject({ code: 'JIRA_RATE_LIMIT' })
    expect(fake.requests).toHaveLength(2)
  })

  it('Retry-After 파싱: 초·날짜·이상한 값', () => {
    expect(parseRetryAfter('5')).toBe(5)
    expect(parseRetryAfter(new Date(10_000).toUTCString(), 4_000)).toBe(6)
    expect(parseRetryAfter('soon')).toBeNull()
    expect(parseRetryAfter(null)).toBeNull()
  })
})
