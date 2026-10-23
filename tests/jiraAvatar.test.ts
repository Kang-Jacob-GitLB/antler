import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { avatarOf, mapSummary } from '../src/main/jira/api'
import { JiraClient, type JiraCredential } from '../src/main/jira/client'
import { JIRA_META_FILE, JIRA_TOKEN_FILE, JiraService } from '../src/main/jira/service'
import { startFakeJira, sendJson, type FakeJira, type RecordedReq } from './jiraFake'

const SECRET = 'tok_SECRET_0123456789'
const PNG = Buffer.from('89504e470d0a1a0a0000000d49484452', 'hex')

let fake: FakeJira | undefined
let dir: string
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-jira-avatar-'))
})
afterEach(async () => {
  await fake?.close()
  fake = undefined
  rmSync(dir, { recursive: true, force: true })
})

const server = (siteUrl: string): JiraCredential => ({ kind: 'server', siteUrl, email: '', token: SECRET })
const cloud = (siteUrl: string): JiraCredential => ({ kind: 'cloud', siteUrl, email: 'me@example.com', token: SECRET })

function imageHandler(req: RecordedReq, res: import('node:http').ServerResponse): void {
  const path = req.url.split('?')[0]!
  if (path.endsWith('/myself')) return sendJson(res, { displayName: '홍길동', name: 'hong' })
  if (path.endsWith('/secure/useravatar')) {
    res.writeHead(200, { 'content-type': 'image/png' })
    return void res.end(PNG)
  }
  if (path.endsWith('/html')) {
    res.writeHead(200, { 'content-type': 'text/html' })
    return void res.end('<script>x</script>')
  }
  if (path.endsWith('/svg')) {
    res.writeHead(200, { 'content-type': 'image/svg+xml' })
    return void res.end('<svg/>')
  }
  if (path.endsWith('/big')) {
    res.writeHead(200, { 'content-type': 'image/png' })
    return void res.end(Buffer.alloc(300 * 1024))
  }
  if (path.endsWith('/login')) {
    res.writeHead(302, { location: '/login.jsp' })
    return void res.end()
  }
  return sendJson(res, { errorMessages: ['nf'] }, 404)
}

describe('avatarOf', () => {
  it('avatarUrls 의 48x48 을 쓰고, http(s) 가 아니면 버린다', () => {
    expect(avatarOf({ avatarUrls: { '48x48': 'https://x/48', '24x24': 'https://x/24' } })).toBe('https://x/48')
    expect(avatarOf({ avatarUrls: { '24x24': 'https://x/24' } })).toBe('https://x/24')
    expect(avatarOf({ avatarUrls: { '48x48': 'javascript:alert(1)' } })).toBe('')
    expect(avatarOf(null)).toBe('')
  })
  it('목록 항목에 담당자 사진 주소가 있을 때만 실린다', () => {
    expect(mapSummary({ key: 'A-1', fields: { assignee: { displayName: 'a', avatarUrls: { '48x48': 'https://x/a' } } } }).assigneeAvatar).toBe('https://x/a')
    expect('assigneeAvatar' in mapSummary({ key: 'A-1', fields: { assignee: { displayName: 'a' } } })).toBe(false)
  })
  it('목록 항목에 기한을 달력 날짜로 싣고, 없거나 날짜가 아니면 빈 문자열', () => {
    expect(mapSummary({ key: 'A-1', fields: { duedate: '2026-10-24' } }).dueDate).toBe('2026-10-24')
    expect(mapSummary({ key: 'A-1', fields: { duedate: null } }).dueDate).toBe('')
    expect(mapSummary({ key: 'A-1', fields: { duedate: 'soon' } }).dueDate).toBe('')
  })
})

describe('JiraClient.image', () => {
  it('사이트 안(컨텍스트 경로 포함)의 사진은 인증을 붙여 받는다', async () => {
    fake = await startFakeJira(imageHandler)
    const c = new JiraClient(fetch, () => server(`${fake!.origin}/jira`))
    const img = await c.image(`${fake.origin}/jira/secure/useravatar?avatarId=1`)
    expect(img?.type).toBe('image/png')
    expect(img?.bytes.equals(PNG)).toBe(true)
    expect(fake.requests[0]!.url).toBe('/jira/secure/useravatar?avatarId=1')
    expect(fake.requests[0]!.headers['authorization']).toBe(`Bearer ${SECRET}`)
  })

  it('사이트 경로 밖·다른 호스트·http 공개 호스트에는 요청하지 않는다', async () => {
    fake = await startFakeJira(imageHandler)
    const c = new JiraClient(fetch, () => server(`${fake!.origin}/jira`))
    expect(await c.image(`${fake.origin}/other/secure/useravatar`)).toBeNull()
    expect(await c.image(`${fake.altOrigin}/jira/secure/useravatar`)).toBeNull()
    expect(await c.image('http://secure.gravatar.com/avatar/x')).toBeNull()
    expect(await c.image('not a url')).toBeNull()
    // 프록시가 풀면 사이트 경로 밖이 되는 인코딩된 경로
    expect(await c.image(`${fake.origin}/jira/..%2Fadmin/secure/useravatar`)).toBeNull()
    expect(await c.image(`${fake.origin}/jira/%2e%2e/admin`)).toBeNull()
    expect(fake.requests).toHaveLength(0)
  })

  it('이미지가 아니거나(SVG 포함) 너무 크거나 리다이렉트면 null', async () => {
    fake = await startFakeJira(imageHandler)
    const c = new JiraClient(fetch, () => server(fake!.origin))
    for (const p of ['/html', '/svg', '/big', '/login', '/missing']) expect(await c.image(`${fake.origin}${p}`)).toBeNull()
    // 사이트 안의 리다이렉트(로그인 화면)는 따라가지 않는다
    expect(fake.requests.map((r) => r.url)).not.toContain('/login.jsp')
  })

  it('공개 사진 호스트는 인증 없이 받고, 그 호스트들 사이의 리다이렉트만 따라간다', async () => {
    const calls: { url: string; auth: string | undefined }[] = []
    const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input)
      calls.push({ url, auth: (init?.headers as Record<string, string>)['Authorization'] })
      if (url.startsWith('https://secure.gravatar.com/')) return new Response(null, { status: 302, headers: { location: 'https://avatar-management--avatars.us-west-2.prod.public.atl-paas.net/initials/HG-1.png' } })
      if (url.includes('atl-paas.net')) return new Response(PNG, { status: 200, headers: { 'content-type': 'image/png' } })
      return new Response(null, { status: 302, headers: { location: 'https://evil.example.com/x.png' } })
    }) as typeof fetch
    const c = new JiraClient(fetchImpl, () => cloud('https://acme.atlassian.net'))
    expect((await c.image('https://secure.gravatar.com/avatar/abc?d=x'))?.type).toBe('image/png')
    expect(calls.map((x) => x.auth)).toEqual([undefined, undefined])

    calls.length = 0
    // 공개 호스트가 허용 목록 밖으로 보내면 따라가지 않는다
    expect(await c.image('https://i0.wp.com/whatever.png')).toBeNull()
    expect(calls.map((x) => x.url)).toEqual(['https://i0.wp.com/whatever.png'])
  })

  it('공개 호스트가 사이트로 리다이렉트해도 인증을 붙이지 않는다', async () => {
    const calls: (string | undefined)[] = []
    const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
      calls.push((init?.headers as Record<string, string>)['Authorization'])
      if (String(input).includes('gravatar')) return new Response(null, { status: 302, headers: { location: 'https://acme.atlassian.net/secure/useravatar' } })
      return new Response(PNG, { status: 200, headers: { 'content-type': 'image/png' } })
    }) as typeof fetch
    const c = new JiraClient(fetchImpl, () => cloud('https://acme.atlassian.net'))
    // 사이트 주소는 공개 호스트가 아니라 리다이렉트 뒤에는 요청 자체를 하지 않는다
    expect(await c.image('https://secure.gravatar.com/avatar/abc')).toBeNull()
    expect(calls).toEqual([undefined])
  })
})

describe('JiraService.avatar', () => {
  const safe: SafeStorageLike = {
    isEncryptionAvailable: () => true,
    encryptString: (s) => Buffer.from(`ENC:${s}`),
    decryptString: (b) => b.subarray(4).toString('utf8')
  }
  const newService = (): JiraService =>
    new JiraService({ store: new TokenStore(dir, safe, process.platform, { token: JIRA_TOKEN_FILE, meta: JIRA_META_FILE }), allowInsecureLoopback: true, sleep: async () => undefined })

  it('data URL 로 돌려주고, 같은 주소는 다시 받지 않으며, 연결을 끊으면 캐시를 비운다', async () => {
    fake = await startFakeJira(imageHandler)
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    const url = `${fake.origin}/secure/useravatar?avatarId=7`
    const [a, b] = await Promise.all([svc.avatar(url), svc.avatar(url)])
    expect(a).toBe(`data:image/png;base64,${PNG.toString('base64')}`)
    expect(b).toBe(a)
    expect(await svc.avatar(url)).toBe(a)
    const hits = (): number => fake!.requests.filter((r) => r.url.startsWith('/secure/useravatar')).length
    expect(hits()).toBe(1)

    svc.disconnect()
    expect(await svc.avatar(url)).toBeNull()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect(await svc.avatar(url)).toBe(a)
    expect(hits()).toBe(2)
  })

  it('문자열이 아니거나 너무 긴 주소는 요청하지 않고 null', async () => {
    fake = await startFakeJira(imageHandler)
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    const before = fake.requests.length
    expect(await svc.avatar(42)).toBeNull()
    expect(await svc.avatar(`${fake.origin}/secure/useravatar?x=${'a'.repeat(3000)}`)).toBeNull()
    expect(fake.requests.length).toBe(before)
  })
})
