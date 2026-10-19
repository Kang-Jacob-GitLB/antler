import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { JIRA_META_FILE, JIRA_TOKEN_FILE, JiraService } from '../src/main/jira/service'
import { allowedJiraUrl, parseConnectInput, parseIssueKey, parseSearchQuery, parseSiteUrl } from '../src/main/jira/validate'
import { startFakeJira, sendJson, type FakeJira, type RecordedReq } from './jiraFake'

const SECRET = 'tok_SECRET_0123456789'

/** 평문과 구분되는 가짜 암호화(XOR + 접두사) */
function fakeSafe(): SafeStorageLike {
  return {
    isEncryptionAvailable: () => true,
    encryptString: (s) => Buffer.concat([Buffer.from('ENC:'), Buffer.from(s, 'utf8').map((b) => b ^ 0x5a)]),
    decryptString: (b) => Buffer.from(b.subarray(4).map((x) => x ^ 0x5a)).toString('utf8')
  }
}

let dir: string
let fake: FakeJira | undefined
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-jira-'))
})
afterEach(async () => {
  await fake?.close()
  fake = undefined
  rmSync(dir, { recursive: true, force: true })
})

const newStore = (): TokenStore => new TokenStore(dir, fakeSafe(), process.platform, { token: JIRA_TOKEN_FILE, meta: JIRA_META_FILE })
const newService = (): JiraService => new JiraService({ store: newStore(), allowInsecureLoopback: true, sleep: async () => undefined })

const ADF = {
  type: 'doc',
  content: [
    { type: 'paragraph', content: [{ type: 'text', text: '설명 <b>본문</b>', marks: [{ type: 'strong' }] }] },
    { type: 'paragraph', content: [{ type: 'text', text: 'x', marks: [{ type: 'link', attrs: { href: 'javascript:alert(1)' } }] }] }
  ]
}

function jiraHandler(req: RecordedReq, res: import('node:http').ServerResponse): void {
  const path = req.url.split('?')[0]!
  if (path.endsWith('/myself')) return sendJson(res, { displayName: '홍길동' })
  if (/\/issue\/ABC-1$/.test(path)) {
    return sendJson(res, {
      key: 'ABC-1',
      fields: {
        summary: '로그인 오류',
        status: { name: '진행 중', statusCategory: { key: 'indeterminate' } },
        issuetype: { name: 'Bug' },
        priority: { name: 'High' },
        assignee: { displayName: '홍길동' },
        reporter: { displayName: '김철수' },
        created: '2026-01-01T00:00:00.000+0000',
        updated: '2026-01-02T00:00:00.000+0000',
        description: ADF
      },
      renderedFields: { description: '<p>서버 렌더</p>' }
    })
  }
  if (path.endsWith('/comment')) return sendJson(res, { comments: [{ author: { displayName: '이영희' }, created: '2026-01-03', body: ADF, renderedBody: '<p>댓글 렌더</p>' }] })
  if (path.endsWith('/search/jql')) return sendJson(res, { issues: [{ key: 'ABC-2', fields: { summary: 's' } }], nextPageToken: 'tok2' })
  if (path.endsWith('/search')) return sendJson(res, { issues: [{ key: 'ABC-3', fields: { summary: 's' } }], startAt: 0, total: 99 })
  return sendJson(res, { errorMessages: ['nf'] }, 404)
}

describe('JiraService', () => {
  it('연결하면 /myself 로 확인하고, 상태·응답·디스크 어디에도 토큰이 평문으로 없다', async () => {
    fake = await startFakeJira(jiraHandler)
    const svc = newService()
    const state = await svc.connect({ kind: 'cloud', siteUrl: `${fake.origin}/`, email: 'me@example.com', token: ` ${SECRET} ` })
    expect(state).toMatchObject({ connected: true, kind: 'cloud', siteUrl: fake.origin, email: 'me@example.com', displayName: '홍길동' })
    expect(JSON.stringify(state)).not.toContain(SECRET)
    expect(fake.requests[0]!.url).toBe('/rest/api/3/myself')

    const detail = await svc.issue('ABC-1')
    const comments = await svc.comments('ABC-1')
    const found = await svc.search({ jql: 'project = ABC' })
    expect(JSON.stringify({ detail, comments, found })).not.toContain(SECRET)

    // 디스크: 자격 증명 파일은 암호화되어 토큰 평문이 없다
    for (const f of readdirSync(dir)) expect(readFileSync(join(dir, f)).toString('latin1')).not.toContain(SECRET)
  })

  it('다시 시작해도(새 서비스) 저장된 연결을 복원한다', async () => {
    fake = await startFakeJira(jiraHandler)
    await newService().connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    const again = newService()
    expect(again.auth()).toMatchObject({ connected: true, kind: 'server', siteUrl: fake.origin })
    await again.issue('ABC-1')
    expect(fake.requests.at(-1)!.headers['authorization']).toBe(`Bearer ${SECRET}`)
    expect(again.disconnect().connected).toBe(false)
    expect(readdirSync(dir).filter((f) => f.startsWith('jira-'))).toEqual([])
    expect(newService().auth().connected).toBe(false)
  })

  it('잘못된 토큰(401)이면 저장하지 않고 토큰 없는 메시지로 거부한다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 401))
    const svc = newService()
    const err = await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET }).catch((e: unknown) => e)
    expect((err as Error).message).not.toContain(SECRET)
    expect(svc.auth().connected).toBe(false)
    expect(readdirSync(dir)).toEqual([])
  })

  it('Cloud 이슈: ADF 를 HTML 로 바꾸고 이스케이프하며, 위험한 링크는 싣지 않는다', async () => {
    fake = await startFakeJira(jiraHandler)
    const svc = newService()
    await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    const d = await svc.issue('ABC-1')
    expect(d).toMatchObject({ key: 'ABC-1', summary: '로그인 오류', status: '진행 중', statusCategory: 'indeterminate', issueType: 'Bug', priority: 'High', assignee: '홍길동', reporter: '김철수', url: `${fake.origin}/browse/ABC-1` })
    expect(d.descriptionHtml).toContain('<strong>설명 &lt;b&gt;본문&lt;/b&gt;</strong>')
    expect(d.descriptionHtml).not.toMatch(/javascript:|<b>/)
    expect((await svc.comments('ABC-1'))[0]).toMatchObject({ author: '이영희' })
    // 요청한 필드·확장
    const q = new URL(fake.requests.find((r) => r.url.includes('/issue/ABC-1?'))!.url, 'http://x').searchParams
    expect(q.get('expand')).toBe('renderedFields')
  })

  it('Server 이슈: 서버가 렌더한 HTML 을 쓴다(API v2)', async () => {
    fake = await startFakeJira(jiraHandler)
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect((await svc.issue('ABC-1')).descriptionHtml).toBe('<p>서버 렌더</p>')
    expect((await svc.comments('ABC-1'))[0]!.bodyHtml).toBe('<p>댓글 렌더</p>')
    expect(fake.requests.some((r) => r.url.startsWith('/rest/api/2/issue/ABC-1?'))).toBe(true)
  })

  it('검색: Cloud 는 nextPageToken, Server 는 startAt, 페이지 상한에서 멈춘다', async () => {
    fake = await startFakeJira(jiraHandler)
    const cloud = newService()
    await cloud.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    expect(await cloud.search({ jql: 'x = 1' })).toMatchObject({ nextCursor: 'tok2' })
    expect((await cloud.search({ jql: 'x = 1', cursor: 'tok2', page: 8 })).nextCursor).toBeUndefined()
    expect(new URL(fake.requests.at(-1)!.url, 'http://x').searchParams.get('nextPageToken')).toBe('tok2')
    await expect(cloud.search({ jql: 'x = 1', page: 9 })).rejects.toThrow('잘못된 페이지')

    const dc = newService()
    await dc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect(await dc.search({ jql: 'x = 1' })).toMatchObject({ items: [{ key: 'ABC-3' }], nextCursor: '1' })
    await dc.search({ jql: 'x = 1', cursor: '25', page: 2 })
    const sp = new URL(fake.requests.at(-1)!.url, 'http://x').searchParams
    expect(sp.get('startAt')).toBe('25')
    expect(sp.get('maxResults')).toBe('25')
    await expect(dc.search({ jql: 'x = 1', cursor: '100000', page: 2 })).rejects.toThrow()
  })

  it('연결 전에는 조회할 수 없고, 열 수 있는 주소는 연결한 사이트의 것뿐이다', async () => {
    const svc = newService()
    await expect(svc.issue('ABC-1')).rejects.toMatchObject({ code: 'JIRA_AUTH' })
    expect(svc.externalUrl('https://x.atlassian.net/browse/ABC-1')).toBeNull()
    fake = await startFakeJira(jiraHandler)
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect(svc.externalUrl(`${fake.origin}/browse/ABC-1`)).toBe(`${fake.origin}/browse/ABC-1`)
    expect(svc.externalUrl(`${fake.altOrigin}/browse/ABC-1`)).toBeNull()
    expect(svc.externalUrl('https://evil.example/browse/ABC-1')).toBeNull()
  })

  it('요청 ID 로 진행 중인 조회를 취소할 수 있다', async () => {
    fake = await startFakeJira((req, res) => (req.url.includes('/myself') ? sendJson(res, { displayName: 'x' }) : undefined))
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    const p = svc.issue('ABC-1', 'req-1')
    setTimeout(() => svc.cancel('req-1'), 50)
    await expect(p).rejects.toMatchObject({ name: 'AbortError' })
  })
})

describe('입력 검증', () => {
  it('사이트 주소: https 만, 자격 증명·쿼리 거부, 컨텍스트 경로 유지', () => {
    expect(parseSiteUrl('https://x.atlassian.net/')).toBe('https://x.atlassian.net')
    expect(parseSiteUrl(' https://jira.example.com/jira/ ')).toBe('https://jira.example.com/jira')
    expect(() => parseSiteUrl('http://x.atlassian.net')).toThrow('https://')
    expect(() => parseSiteUrl('ftp://x')).toThrow()
    expect(() => parseSiteUrl('https://u:p@x.atlassian.net')).toThrow('사용자 정보')
    expect(() => parseSiteUrl('https://x.atlassian.net/?a=1')).toThrow('쿼리')
    expect(() => parseSiteUrl('not a url')).toThrow()
  })

  it('루프백 http 는 내부 플래그가 있을 때 127.0.0.1·localhost 에만', () => {
    expect(() => parseSiteUrl('http://127.0.0.1:1234')).toThrow()
    expect(parseSiteUrl('http://127.0.0.1:1234', { allowInsecureLoopback: true })).toBe('http://127.0.0.1:1234')
    expect(parseSiteUrl('http://localhost:1234', { allowInsecureLoopback: true })).toBe('http://localhost:1234')
    expect(() => parseSiteUrl('http://example.com', { allowInsecureLoopback: true })).toThrow()
  })

  it('연결 입력: Cloud 는 이메일 필수, 토큰 형식', () => {
    expect(parseConnectInput({ kind: 'cloud', siteUrl: 'https://x.atlassian.net', email: 'a@b.c', token: ' t ' })).toMatchObject({ token: 't' })
    expect(() => parseConnectInput({ kind: 'cloud', siteUrl: 'https://x.atlassian.net', token: 't' })).toThrow('이메일')
    expect(() => parseConnectInput({ kind: 'cloud', siteUrl: 'https://x.atlassian.net', email: 'a:b@c', token: 't' })).toThrow('이메일')
    expect(() => parseConnectInput({ kind: 'server', siteUrl: 'https://x.example.com', token: 'a b' })).toThrow('토큰')
    expect(() => parseConnectInput({ kind: 'oauth', siteUrl: 'https://x.example.com', token: 't' })).toThrow()
  })

  it('이슈 키·검색 조건', () => {
    expect(parseIssueKey('ABC-123')).toBe('ABC-123')
    for (const bad of ['abc-1', 'ABC-', 'ABC-1/../x', 'ABC-1\n', '', 5]) expect(() => parseIssueKey(bad)).toThrow()
    expect(parseSearchQuery({ jql: ' project = A ' })).toEqual({ jql: 'project = A', page: 1 })
    expect(() => parseSearchQuery({ jql: 'x'.repeat(2001) })).toThrow()
    expect(() => parseSearchQuery({ jql: 'a\u0000b' })).toThrow()
    expect(() => parseSearchQuery({ jql: 'a', cursor: 'has space' })).toThrow()
  })

  it('allowedJiraUrl: 호스트·포트·스킴이 정확히 같아야 한다', () => {
    const site = 'https://x.atlassian.net'
    expect(allowedJiraUrl('https://x.atlassian.net/browse/A-1', site)).toBe('https://x.atlassian.net/browse/A-1')
    expect(allowedJiraUrl('http://x.atlassian.net/browse/A-1', site)).toBeNull()
    expect(allowedJiraUrl('https://x.atlassian.net.evil.com/', site)).toBeNull()
    expect(allowedJiraUrl('https://x.atlassian.net:8443/', site)).toBeNull()
    expect(allowedJiraUrl('https://u:p@x.atlassian.net/', site)).toBeNull()
    expect(allowedJiraUrl('javascript:alert(1)', site)).toBeNull()
    expect(allowedJiraUrl('https://x.atlassian.net/', undefined)).toBeNull()
  })
})
