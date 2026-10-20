import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { JIRA_META_FILE, JIRA_TOKEN_FILE, JiraService } from '../src/main/jira/service'
import { allowedJiraUrl, parseConnectInput, parseIssueKey, parseSearchQuery, parseSiteUrl } from '../src/main/jira/validate'
import { startFakeJira, sendJson, worklogHandler, type FakeJira, type FakeWorklog, type FakeWorklogSite, type RecordedReq } from './jiraFake'

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
    expect(q.get('expand')).toBeNull() // Cloud 는 ADF 를 직접 변환하므로 서버 렌더 HTML 을 요청하지 않는다
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

  it('개발 정보: 이슈 id → 요약의 연동 종류 → PR·커밋 상세, https 가 아닌 주소와 잘못된 sha 는 버린다', async () => {
    fake = await startFakeJira((req, res) => {
      const u = new URL(req.url, 'http://x')
      if (u.pathname.endsWith('/myself')) return sendJson(res, { displayName: 'x' })
      if (u.pathname.endsWith('/issue/ABC-1')) return sendJson(res, { id: '10001', key: 'ABC-1', fields: {} })
      if (u.pathname.endsWith('/dev-status/latest/issue/summary')) {
        return sendJson(res, { summary: { pullrequest: { byInstanceType: { GitHub: {} } }, repository: { byInstanceType: { GitHub: {}, 'bad type!': {} } } } })
      }
      if (u.pathname.endsWith('/dev-status/latest/issue/detail')) {
        if (u.searchParams.get('dataType') === 'pullrequest') {
          return sendJson(res, {
            detail: [
              {
                pullRequests: [
                  { name: '로그인 수정', url: 'https://github.com/o/r/pull/7', status: 'MERGED', author: { name: '홍길동' }, repositoryName: 'o/r', source: { branch: 'fix/ABC-1' }, lastUpdate: '2026-01-02T00:00:00.000+0000' },
                  { name: '나쁜 주소', url: 'javascript:alert(1)', status: 'OPEN', repositoryName: 'o/r' }
                ]
              }
            ]
          })
        }
        return sendJson(res, {
          detail: [
            {
              repositories: [
                {
                  name: 'o/r',
                  commits: [
                    { id: 'abcdef1234567', message: 'ABC-1 로그인 수정\n\n본문', url: 'https://github.com/o/r/commit/abcdef1234567', author: { name: '홍길동' }, authorTimestamp: 1767225600000 },
                    { id: 'not-a-sha', message: 'x' }
                  ]
                }
              ]
            }
          ]
        })
      }
      return sendJson(res, {}, 404)
    })
    const svc = newService()
    await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    const dev = await svc.devInfo('ABC-1')
    expect(dev.available).toBe(true)
    expect(dev.pulls).toEqual([
      { title: '로그인 수정', url: 'https://github.com/o/r/pull/7', state: 'merged', author: '홍길동', repo: 'o/r', branch: 'fix/ABC-1', updated: '2026-01-02T00:00:00.000+0000' },
      { title: '나쁜 주소', url: '', state: 'open', author: '', repo: 'o/r', branch: '', updated: '' }
    ])
    expect(dev.commits).toEqual([{ sha: 'abcdef1234567', message: 'ABC-1 로그인 수정', url: 'https://github.com/o/r/commit/abcdef1234567', author: '홍길동', repo: 'o/r', date: '2026-01-01T00:00:00.000Z' }])
    // 이상한 연동 종류 이름은 요청하지 않는다
    expect(fake.requests.some((r) => r.url.includes('bad'))).toBe(false)
    expect(JSON.stringify(dev)).not.toContain(SECRET)
  })

  it('개발 정보: 연동 API 가 없으면(404) 오류가 아니라 available:false', async () => {
    fake = await startFakeJira(jiraHandler)
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    // jiraHandler 의 이슈 응답에는 id 가 없다 → 요약까지 가지 않는다
    expect(await svc.devInfo('ABC-1')).toEqual({ available: false, pulls: [], commits: [] })
    fake.requests.length = 0
    await fake.close()
    fake = await startFakeJira((req, res) =>
      req.url.includes('/myself') ? sendJson(res, { displayName: 'x' }) : req.url.includes('/issue/ABC-1') ? sendJson(res, { id: '5' }) : sendJson(res, {}, 404)
    )
    const svc2 = newService()
    await svc2.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect(await svc2.devInfo('ABC-1')).toEqual({ available: false, pulls: [], commits: [] })
    expect(fake.requests.some((r) => r.url.startsWith('/rest/dev-status/latest/issue/summary?issueId=5'))).toBe(true)
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


describe('워크로그·사용자 검색', () => {
  const adfComment = (t: string): unknown => ({ type: 'doc', version: 1, content: [{ type: 'paragraph', content: [{ type: 'text', text: t }] }, { type: 'paragraph', content: [{ type: 'text', text: '둘째' }] }] })
  const wl = (id: string, author: string, started: string, extra: Partial<FakeWorklog> = {}): FakeWorklog => ({ id, author, started, seconds: 3600, ...extra })
  const Q = { from: '2026-10-19', to: '2026-10-20', users: [] as string[] }
  const jqlOf = (r: RecordedReq): string => new URL(r.url, 'http://x').searchParams.get('jql') ?? ''
  const worklogReqs = (): RecordedReq[] => fake!.requests.filter((r) => /\/issue\/[^/]+\/worklog/.test(r.url))

  async function connected(site: FakeWorklogSite): Promise<JiraService> {
    fake = await startFakeJira(worklogHandler(site))
    const svc = newService()
    await svc.connect(site.kind === 'cloud' ? { kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET } : { kind: 'server', siteUrl: fake.origin, token: SECRET })
    return svc
  }

  it('Cloud: 내장 worklog 로 충분하면 다시 받지 않고, 나만·범위 안만 남긴다', async () => {
    const svc = await connected({
      kind: 'cloud',
      issues: [
        {
          key: 'ABC-1',
          summary: '로그인',
          type: 'Bug',
          worklogs: [
            wl('1', 'acc-me', '2026-10-19T09:00:00.000+0900', { comment: adfComment('작업') }),
            wl('2', 'acc-other', '2026-10-19T09:00:00.000+0900'),
            wl('3', 'acc-me', '2026-10-10T09:00:00.000+0900'),
            wl('4', 'acc-me', '2026-10-21T23:00:00.000+0900')
          ]
        }
      ]
    })
    const r = await svc.worklogs(Q)
    expect(r.me).toEqual({ id: 'acc-me', name: '나' })
    expect(r.truncated).toBe(false)
    expect(r.worklogs.map((w) => w.id)).toEqual(['1', '4']) // 4 는 로컬 날짜 필터가 화면 몫인 여유분
    expect(r.worklogs[0]).toMatchObject({ issueKey: 'ABC-1', issueSummary: '로그인', issueType: 'Bug', authorId: 'acc-me', author: '표시-acc-me', seconds: 3600, comment: '작업 둘째' })
    const search = fake!.requests.find((x) => x.url.startsWith('/rest/api/3/search/jql'))!
    expect(jqlOf(search)).toBe('worklogAuthor = currentUser() AND worklogDate >= "2026-10-18" AND worklogDate <= "2026-10-21" ORDER BY updated DESC')
    expect(new URL(search.url, 'http://x').searchParams.get('fields')).toBe('summary,issuetype,worklog')
    expect(worklogReqs()).toHaveLength(0)
    expect(fake!.requests.every((x) => x.method === 'GET')).toBe(true)
  })

  it('Cloud: 내장 worklog 가 total 보다 적으면 그 이슈만 범위를 주어 다시 받는다', async () => {
    const mk = (n: number): FakeWorklog[] => Array.from({ length: n }, (_, i) => wl(`w${i}`, 'acc-x', '2026-10-20T10:00:00.000+0900'))
    const svc = await connected({
      kind: 'cloud',
      issues: [
        { key: 'ABC-1', worklogs: mk(3), embed: 1 },
        { key: 'ABC-2', worklogs: [wl('z', 'acc-x', '2026-10-20T10:00:00.000+0900')] },
        { key: 'ABC-3', worklogs: [wl('far', 'acc-x', '2026-01-01T10:00:00.000+0900'), ...mk(2).map((w) => ({ ...w, id: `n${w.id}` }))], embed: 0 }
      ]
    })
    const r = await svc.worklogs({ ...Q, users: ['acc-x', 'acc-y'] })
    expect(r.worklogs.map((w) => w.id).sort()).toEqual(['nw0', 'nw1', 'w0', 'w1', 'w2', 'z'].sort())
    const reqs = worklogReqs()
    expect(reqs.map((x) => x.url.split('?')[0])).toEqual(['/rest/api/3/issue/ABC-1/worklog', '/rest/api/3/issue/ABC-3/worklog'])
    const p = new URL(reqs[0]!.url, 'http://x').searchParams
    expect(Number(p.get('startedAfter'))).toBe(Date.parse('2026-10-18T00:00:00Z'))
    expect(Number(p.get('startedBefore'))).toBe(Date.parse('2026-10-22T00:00:00Z'))
    expect(jqlOf(fake!.requests.find((x) => x.url.startsWith('/rest/api/3/search/jql'))!)).toContain('worklogAuthor in ("acc-x","acc-y")')
  })

  it('Server: name 과 key 둘 다로 작성자를 맞추고, 쿼리 없이 받은 전체를 클라이언트에서 거른다', async () => {
    const svc = await connected({
      kind: 'server',
      issues: [
        {
          key: 'DC-1',
          embed: 0,
          worklogs: [
            wl('1', 'alice', '2026-10-19T09:00:00.000+0900', { comment: '평문 설명' }),
            wl('2', 'bob', '2026-10-19T09:00:00.000+0900', { authorKey: 'JIRAUSER9' }),
            wl('3', 'carol', '2026-10-19T09:00:00.000+0900'),
            wl('4', 'alice', '2026-09-01T09:00:00.000+0900')
          ]
        }
      ]
    })
    const r = await svc.worklogs({ ...Q, users: ['alice', 'JIRAUSER9'] })
    expect(r.me).toEqual({ id: 'me', name: '나' })
    expect(r.worklogs.map((w) => [w.id, w.authorId, w.comment])).toEqual([['1', 'alice', '평문 설명'], ['2', 'bob', '']])
    expect(worklogReqs().map((x) => x.url)).toEqual(['/rest/api/2/issue/DC-1/worklog'])
    expect(jqlOf(fake!.requests.find((x) => x.url.startsWith('/rest/api/2/search'))!)).toContain('worklogAuthor in ("alice","JIRAUSER9")')
  })

  it('사용자를 비우면 Server 도 me.id(name)로 거른다', async () => {
    const svc = await connected({ kind: 'server', issues: [{ key: 'DC-1', worklogs: [wl('1', 'me', '2026-10-19T09:00:00.000+0900'), wl('2', 'alice', '2026-10-19T09:00:00.000+0900')] }] })
    expect((await svc.worklogs(Q)).worklogs.map((w) => w.id)).toEqual(['1'])
  })

  it('내 id 는 연결 동안 캐시하고, 재연결하면 다시 읽는다', async () => {
    const svc = await connected({ kind: 'cloud', issues: [] })
    const myselfCount = (): number => fake!.requests.filter((x) => x.url.endsWith('/myself')).length
    const base = myselfCount()
    await svc.worklogs(Q)
    await svc.worklogs(Q)
    expect(myselfCount()).toBe(base + 1)
    await svc.connect({ kind: 'cloud', siteUrl: fake!.origin, email: 'a@b.c', token: SECRET })
    const afterConnect = myselfCount()
    await svc.worklogs(Q)
    expect(myselfCount()).toBe(afterConnect + 1)
  })

  it('이슈가 300건을 넘으면 300건에서 자르고 truncated 로 알린다', async () => {
    const issues = Array.from({ length: 350 }, (_, i) => ({ key: `ABC-${i + 1}`, worklogs: [wl(`w${i}`, 'acc-me', '2026-10-19T09:00:00.000+0900')] }))
    for (const kind of ['cloud', 'server'] as const) {
      await fake?.close()
      const svc = await connected({ kind, issues: issues.map((i) => ({ ...i, worklogs: i.worklogs.map((w) => ({ ...w, author: kind === 'cloud' ? 'acc-me' : 'me' })) })) })
      const r = await svc.worklogs(Q)
      expect(r.worklogs).toHaveLength(300)
      expect(r.truncated).toBe(true)
    }
  })

  it('요청 ID 로 취소하면 AbortError', async () => {
    fake = await startFakeJira((req, res) => (req.url.includes('/myself') ? sendJson(res, { accountId: 'acc-me', displayName: 'x' }) : undefined))
    const svc = newService()
    await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    const p = svc.worklogs(Q, 'wl-1')
    setTimeout(() => svc.cancel('wl-1'), 50)
    await expect(p).rejects.toMatchObject({ name: 'AbortError' })
  })

  it('사용자 검색: Cloud 는 사람 계정만, Server 는 name 이 id', async () => {
    let svc = await connected({
      kind: 'cloud',
      issues: [],
      users: [
        { accountId: '712020:aaaa-bbbb', accountType: 'atlassian', displayName: '홍길동' },
        { accountId: 'app1', accountType: 'app', displayName: '봇' },
        { accountId: 'cust1', accountType: 'customer', displayName: '고객' }
      ]
    })
    expect(await svc.userSearch(' 홍 ')).toEqual([{ id: '712020:aaaa-bbbb', name: '홍길동' }])
    const r = fake!.requests.find((x) => x.url.startsWith('/rest/api/3/user/search'))!
    const p = new URL(r.url, 'http://x').searchParams
    expect([p.get('query'), p.get('maxResults')]).toEqual(['홍', '20'])
    await fake!.close()
    svc = await connected({ kind: 'server', issues: [], users: [{ name: 'alice', key: 'JIRAUSER2', displayName: 'Alice' }] })
    expect(await svc.userSearch('al')).toEqual([{ id: 'alice', name: 'Alice' }])
    expect(fake!.requests.some((x) => x.url.startsWith('/rest/api/2/user/search?username=al&maxResults=20'))).toBe(true)
  })
})
