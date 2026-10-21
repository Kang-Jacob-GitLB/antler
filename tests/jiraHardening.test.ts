// 응답 크기 상한·ADF 방어·Data Center 댓글·오류 문구·설정 검증(리뷰 반영분)
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { TokenStore, type SafeStorageLike } from '../src/main/github/tokenStore'
import { adfToHtml } from '../src/main/jira/adf'
import { JiraClient, type JiraCredential } from '../src/main/jira/client'
import { JIRA_META_FILE, JIRA_TOKEN_FILE, JiraService } from '../src/main/jira/service'
import { jqlQuote, parseUserSearch, parseWorklogQuery } from '../src/main/jira/validate'
import { ConfigStore } from '../src/main/services/ConfigStore'
import type { UiPrefs } from '../src/shared/types'
import { sendJson, startFakeJira, type FakeJira } from './jiraFake'

const SECRET = 'tok_SECRET_0123456789'
let fake: FakeJira | undefined
let dir: string
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-jira-h-'))
})
afterEach(async () => {
  await fake?.close()
  fake = undefined
  rmSync(dir, { recursive: true, force: true })
})

const cloud = (siteUrl: string): JiraCredential => ({ kind: 'cloud', siteUrl, email: 'me@example.com', token: SECRET })
const server = (siteUrl: string): JiraCredential => ({ kind: 'server', siteUrl, email: '', token: SECRET })
const clientFor = (c: JiraCredential, sleep?: (ms: number) => Promise<void>): JiraClient => new JiraClient(fetch, () => c, sleep)

function fakeSafe(): SafeStorageLike {
  return {
    isEncryptionAvailable: () => true,
    encryptString: (s) => Buffer.concat([Buffer.from('ENC:'), Buffer.from(s, 'utf8').map((b) => b ^ 0x5a)]),
    decryptString: (b) => Buffer.from(b.subarray(4).map((x) => x ^ 0x5a)).toString('utf8')
  }
}
const newService = (): JiraService =>
  new JiraService({
    store: new TokenStore(dir, fakeSafe(), process.platform, { token: JIRA_TOKEN_FILE, meta: JIRA_META_FILE }),
    allowInsecureLoopback: true,
    sleep: async () => undefined
  })

const text = (t: string, marks?: unknown[]): unknown => ({ type: 'text', text: t, ...(marks ? { marks } : {}) })
const doc = (...content: unknown[]): unknown => ({ type: 'doc', version: 1, content })

describe('응답 크기 상한', () => {
  it('Content-Length 없이 chunked 로 거대한 본문을 보내도 상한에서 읽기를 중단한다', async () => {
    let closed = false
    fake = await startFakeJira((_req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' }) // Content-Length 없음 → chunked
      res.on('close', () => (closed = true))
      const chunk = Buffer.alloc(1024 * 1024, 0x61)
      let sent = 0
      const pump = (): void => {
        while (sent < 200 && !res.destroyed) {
          sent++
          if (!res.write(chunk)) return void res.once('drain', pump)
        }
        res.end()
      }
      pump()
    })
    const err = await clientFor(cloud(fake.origin)).get('/x').catch((e: unknown) => e)
    expect(err).toMatchObject({ code: 'JIRA_HTTP' })
    expect((err as Error).message).toContain('너무 큽니다')
    await new Promise((r) => setTimeout(r, 200))
    expect(closed).toBe(true)
  })

  it('429 재시도 경로에서 거대한 본문도 상한을 지킨다', async () => {
    fake = await startFakeJira((_req, res, n) => {
      if (n > 1) return sendJson(res, { ok: 1 })
      res.writeHead(429, { 'retry-after': '1' })
      res.end(Buffer.alloc(6 * 1024 * 1024, 0x61))
    })
    const data = await clientFor(cloud(fake.origin), async () => undefined).get<{ ok: number }>('/x')
    expect(data.ok).toBe(1)
  })

  it('본문을 받는 중 시간이 초과되면 TIMEOUT 으로 표시한다', async () => {
    fake = await startFakeJira((_req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.write('{"a":') // 나머지는 보내지 않는다
    })
    const real = AbortSignal.timeout
    // 15초를 기다리지 않도록 타임아웃만 짧게 바꾼다
    AbortSignal.timeout = () => real.call(AbortSignal, 150)
    try {
      await expect(clientFor(cloud(fake.origin)).get('/x')).rejects.toMatchObject({ code: 'JIRA_TIMEOUT' })
    } finally {
      AbortSignal.timeout = real
    }
  })
})

describe('오류 문구', () => {
  it('401 문구는 Cloud(이메일·API 토큰)와 Data Center(PAT)가 다르다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 401))
    const a = await clientFor(cloud(fake.origin)).get('/x').catch((e: Error) => e.message)
    const b = await clientFor(server(fake.origin)).get('/x').catch((e: Error) => e.message)
    expect(a).toContain('이메일·API 토큰')
    expect(b).toContain('Personal Access Token')
    expect(b).not.toContain('이메일')
  })

  it('404 문구는 호출한 쪽이 정한다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 404))
    const err = await clientFor(cloud(fake.origin)).get('/x', { notFound: '검색 API 없음' }).catch((e: unknown) => e)
    expect(err).toMatchObject({ code: 'JIRA_NOT_FOUND', message: '검색 API 없음' })
  })

  it('검색·연결 확인 404 는 각각의 문구', async () => {
    fake = await startFakeJira((req, res) => (req.url.includes('/myself') ? sendJson(res, { displayName: 'x' }) : sendJson(res, {}, 404)))
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    await expect(svc.search({ jql: 'x = 1' })).rejects.toThrow('검색 API')
    await expect(svc.issue('ABC-1')).rejects.toThrow('이슈를 찾을 수 없')
    await fake.close()
    fake = await startFakeJira((_req, res) => sendJson(res, {}, 404))
    await expect(newService().connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })).rejects.toThrow('사이트 주소')
  })
})

describe('adfToHtml 방어', () => {
  it('범위를 벗어난 timestamp 는 RangeError 없이 빈 값', () => {
    for (const ts of [8.64e15 + 1, -8.64e15 - 1, 1e300, Infinity, NaN, 'x']) {
      const d = doc({ type: 'paragraph', content: [{ type: 'date', attrs: { timestamp: ts } }] })
      expect(() => adfToHtml(d)).not.toThrow()
      expect(adfToHtml(d)).toBe('<p></p>')
    }
    expect(adfToHtml(doc({ type: 'paragraph', content: [{ type: 'date', attrs: { timestamp: 0 } }] }))).toBe('<p>1970-01-01</p>')
  })

  it('null 원소·잘못된 marks·content 는 무시한다', () => {
    const html = adfToHtml(
      doc(
        null,
        7,
        { type: 'paragraph', content: [null, 'str', text('ok', [null, 5, { type: 'strong' }, { type: 'link' }]), { type: 'text' }] },
        { type: 'codeBlock', content: [null, text('x'), 3] },
        { type: 'bulletList', content: { not: 'array' } },
        { type: 'paragraph', content: [{ type: 'text', text: 'm', marks: 'strong' }] }
      )
    )
    expect(html).toContain('<strong>ok</strong>')
    expect(html).toContain('<pre><code>x</code></pre>')
    expect(html).toContain('<ul></ul>')
    expect(html).toContain('<p>m</p>')
  })

  it('변환 중 예외가 나면 평문으로 대체한다(이스케이프됨)', () => {
    const evil = {
      type: 'doc',
      get content(): unknown {
        throw new Error('boom')
      }
    }
    expect(() => adfToHtml(evil)).not.toThrow()
    expect(adfToHtml(evil)).toBe('')
    // 한 노드만 예외를 던지면 나머지 text 를 평문 문단으로 보존한다
    const partial = doc({
      type: 'paragraph',
      content: [
        text('<b>남는 글</b>'),
        {
          type: 'heading',
          get attrs(): unknown {
            throw new Error('boom')
          }
        }
      ]
    })
    expect(adfToHtml(partial)).toBe('<p>&lt;b&gt;남는 글&lt;/b&gt;</p>')
  })
})

describe('Data Center 댓글·이슈 주소', () => {
  it('DC 는 total 로 마지막 페이지를 받아 최신 댓글이 먼저 오게 한다(orderBy 를 쓰지 않는다)', async () => {
    const all = Array.from({ length: 12 }, (_, i) => ({ author: { displayName: `u${i + 1}` }, created: `c${i + 1}`, renderedBody: `<p>${i + 1}</p>` }))
    fake = await startFakeJira((req, res) => {
      const u = new URL(req.url, 'http://x')
      if (u.pathname.endsWith('/myself')) return sendJson(res, { displayName: 'x' })
      const start = Number(u.searchParams.get('startAt') ?? 0)
      const max = Number(u.searchParams.get('maxResults') ?? 50)
      sendJson(res, { comments: all.slice(start, start + max), total: all.length, startAt: start })
    })
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    const list = await svc.comments('ABC-1')
    expect(list.map((c) => c.author)).toEqual(['u12', 'u11', 'u10', 'u9', 'u8'])
    const calls = fake.requests.filter((r) => r.url.includes('/comment'))
    expect(calls.map((r) => new URL(r.url, 'http://x').searchParams.get('startAt'))).toEqual(['0', '7'])
    expect(calls.every((r) => !r.url.includes('orderBy'))).toBe(true)
  })

  it('Cloud 댓글은 orderBy=-created 로 최신순을 받고 expand 는 쓰지 않는다', async () => {
    fake = await startFakeJira((req, res) => (req.url.includes('/myself') ? sendJson(res, { displayName: 'x' }) : sendJson(res, { comments: [] })))
    const svc = newService()
    await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    await svc.comments('ABC-1')
    const u = new URL(fake.requests.find((r) => r.url.includes('/comment'))!.url, 'http://x')
    expect(u.searchParams.get('orderBy')).toBe('-created')
    expect(u.searchParams.get('expand')).toBeNull()
  })

  it('이슈 주소는 요청한 키 기준이다', async () => {
    fake = await startFakeJira((req, res) =>
      req.url.includes('/myself') ? sendJson(res, { displayName: 'x' }) : sendJson(res, { key: 'NEW-9', fields: { summary: 's' } })
    )
    const svc = newService()
    await svc.connect({ kind: 'server', siteUrl: fake.origin, token: SECRET })
    expect((await svc.issue('OLD-1')).url).toBe(`${fake.origin}/browse/OLD-1`)
  })
})

describe('ConfigStore 의 사이트별 Jira 설정', () => {
  it('모양과 크기를 검사한다', () => {
    const store = new ConfigStore(join(dir, 'cfg.json'))
    const ok = { 'https://x.atlassian.net': { filters: [{ name: 'a', jql: 'b' }], recent: [{ key: 'ABC-1', summary: 's' }] } }
    expect(store.setUi({ jiraSites: ok }).ui.jiraSites).toEqual(ok)
    expect(() => store.setUi({ jiraSites: { x: { recent: [{ key: 'abc-1', summary: 's' }] } } })).toThrow('Jira 설정')
    expect(() => store.setUi({ jiraSites: { x: { filters: [{ name: 'a'.repeat(101), jql: 'b' }] } } })).toThrow('Jira 설정')
    expect(() => store.setUi({ jiraSites: [] as never })).toThrow('Jira 설정')
  })

  it('워크로그 하루 표시 시각은 정수·범위·시작<종료를 검사한다(저장된 반대쪽 값과도)', () => {
    const store = new ConfigStore(join(dir, 'cfg-hours.json'))
    expect(store.setUi({ worklogDayStart: 9, worklogDayEnd: 18 }).ui).toMatchObject({ worklogDayStart: 9, worklogDayEnd: 18 })
    expect(store.setUi({ worklogDayStart: 0, worklogDayEnd: 24 }).ui.worklogDayEnd).toBe(24)
    const bads: Partial<UiPrefs>[] = [{ worklogDayStart: 18, worklogDayEnd: 9 }, { worklogDayStart: 9, worklogDayEnd: 9 }, { worklogDayStart: -1 }, { worklogDayEnd: 25 }, { worklogDayStart: 24 }, { worklogDayEnd: 0 }, { worklogDayStart: 7.5 }, { worklogDayEnd: '18' as never }]
    for (const bad of bads) expect(() => store.setUi(bad)).toThrow('Jira 설정')
    // 한쪽만 바꿔 저장된 다른 쪽과 어긋나는 경우
    store.setUi({ worklogDayStart: 9, worklogDayEnd: 18 })
    expect(() => store.setUi({ worklogDayStart: 20 })).toThrow('Jira 설정')
    expect(() => store.setUi({ worklogDayEnd: 8 })).toThrow('Jira 설정')
    // 거절된 패치는 저장값을 바꾸지 않는다
    expect(store.get().ui).toMatchObject({ worklogDayStart: 9, worklogDayEnd: 18 })
  })

  it('손상된 저장 시각은 load 에서 버려지고 무관한 setUi 는 성공한다', () => {
    for (const ui of [{ worklogDayStart: 22 }, { worklogDayStart: '9', worklogDayEnd: 18 }, { worklogDayStart: 20, worklogDayEnd: 10 }, { worklogDayStart: 9.5, worklogDayEnd: 18 }]) {
      const file = join(dir, 'cfg-corrupt.json')
      writeFileSync(file, JSON.stringify({ version: 1, ui }))
      const store = new ConfigStore(file)
      expect(store.get().ui.worklogDayStart).toBeUndefined()
      expect(store.get().ui.worklogDayEnd).toBeUndefined()
      expect(store.setUi({ theme: 'dark' }).ui.theme).toBe('dark')
    }
    const file = join(dir, 'cfg-ok.json')
    writeFileSync(file, JSON.stringify({ version: 1, ui: { worklogDayStart: 9, worklogDayEnd: 18 } }))
    expect(new ConfigStore(file).get().ui).toMatchObject({ worklogDayStart: 9, worklogDayEnd: 18 })
  })

  it('워크로그 마지막 보기(worklogMode)는 day·week·month 만 저장하고 load 에서 잘못된 값은 버린다', () => {
    const store = new ConfigStore(join(dir, 'cfg-mode.json'))
    for (const m of ['day', 'week', 'month'] as const) expect(store.setUi({ worklogMode: m }).ui.worklogMode).toBe(m)
    for (const bad of ['year', '', 3, null, 'Day']) expect(() => store.setUi({ worklogMode: bad as never })).toThrow('Jira 설정')
    expect(store.get().ui.worklogMode).toBe('month')
    const file = join(dir, 'cfg-mode-corrupt.json')
    writeFileSync(file, JSON.stringify({ version: 1, ui: { worklogMode: 'year' } }))
    const loaded = new ConfigStore(file)
    expect(loaded.get().ui.worklogMode).toBeUndefined()
    expect(loaded.setUi({ theme: 'dark' }).ui.theme).toBe('dark')
    writeFileSync(file, JSON.stringify({ version: 1, ui: { worklogMode: 'week' } }))
    expect(new ConfigStore(file).get().ui.worklogMode).toBe('week')
  })
})

describe('워크로그 조회·사용자 검색 입력 검증', () => {
  const ok = { from: '2026-10-19', to: '2026-10-20', users: [] as string[] }

  it('정상 입력은 통과하고 사용자는 중복을 없앤다', () => {
    expect(parseWorklogQuery({ ...ok, users: ['712020:aaaa-bbbb', 'a.b_c@d+e', 'x', 'x'] }).users).toEqual(['712020:aaaa-bbbb', 'a.b_c@d+e', 'x'])
    // 정확히 62일(양끝 포함)은 허용
    expect(() => parseWorklogQuery({ from: '2026-01-01', to: '2026-03-03', users: [] })).not.toThrow()
    expect(() => parseWorklogQuery({ from: '2024-02-29', to: '2024-02-29', users: [] })).not.toThrow()
  })

  it('잘못된 날짜·존재하지 않는 날짜·거꾸로·62일 초과를 거부한다', () => {
    for (const bad of ['2026-1-1', '20261019', '2026-10-19T00:00', '2026-02-30', '2026-13-01', '2026-00-10', '2025-02-29', 'abc', '', 20261019, null]) {
      expect(() => parseWorklogQuery({ ...ok, from: bad })).toThrow()
      expect(() => parseWorklogQuery({ ...ok, to: bad })).toThrow()
    }
    expect(() => parseWorklogQuery({ ...ok, from: '2026-10-21' })).toThrow('늦습니다')
    expect(() => parseWorklogQuery({ from: '2026-01-01', to: '2026-03-04', users: [] })).toThrow('62일')
    for (const bad of [null, 'x', 5, []]) expect(() => parseWorklogQuery(bad)).toThrow()
  })

  it('JQL 주입을 노린 사용자 id 와 31명 초과를 거부한다', () => {
    for (const id of ['a" OR 1=1', 'a"', 'a\\', 'a b', 'a)', 'a\n', '', 'x'.repeat(129), 5, null]) {
      expect(() => parseWorklogQuery({ ...ok, users: [id] })).toThrow()
    }
    expect(() => parseWorklogQuery({ ...ok, users: Array.from({ length: 31 }, (_, i) => `u${i}`) })).toThrow('30명')
    expect(() => parseWorklogQuery({ ...ok, users: Array.from({ length: 30 }, (_, i) => `u${i}`) })).not.toThrow()
    expect(() => parseWorklogQuery({ ...ok, users: 'abc' })).toThrow()
    expect(() => parseWorklogQuery({ from: ok.from, to: ok.to })).toThrow()
    expect(parseWorklogQuery({ ...ok, includeMe: true }).includeMe).toBe(true)
    expect(parseWorklogQuery({ ...ok, includeMe: undefined })).not.toHaveProperty('includeMe')
    for (const bad of ['true', 1, null]) expect(() => parseWorklogQuery({ ...ok, includeMe: bad })).toThrow('includeMe')
    expect(() => jqlQuote('a" OR 1=1')).toThrow()
    expect(jqlQuote('712020:aaaa')).toBe('"712020:aaaa"')
  })

  it('검색어: 1~100자, 제어문자 금지', () => {
    expect(parseUserSearch('  홍길동 ')).toBe('홍길동')
    for (const bad of ['', '   ', 'a'.repeat(101), 'a\u0000b', 'a\nb', 5, null]) expect(() => parseUserSearch(bad)).toThrow()
  })

  it('서비스는 거부된 입력으로 Jira 에 요청을 보내지 않는다', async () => {
    fake = await startFakeJira((_req, res) => sendJson(res, { accountId: 'a', displayName: 'x' }))
    const svc = newService()
    await svc.connect({ kind: 'cloud', siteUrl: fake.origin, email: 'a@b.c', token: SECRET })
    const before = fake.requests.length
    await expect(svc.worklogs({ ...ok, users: ['a" OR 1=1'] })).rejects.toThrow()
    await expect(svc.userSearch('a\u0001')).rejects.toThrow()
    expect(fake.requests.length).toBe(before)
  })

  it('ConfigStore: 팀원(team) 모양과 크기를 검사한다', () => {
    const store = new ConfigStore(join(dir, 'cfg-team.json'))
    const site = (team: unknown): never => ({ 'https://x.atlassian.net': { team } }) as never
    const good = [{ id: '712020:aaaa', name: '홍길동' }]
    expect(store.setUi({ jiraSites: site(good) }).ui.jiraSites).toEqual({ 'https://x.atlassian.net': { team: good } })
    for (const bad of [
      'x',
      [{ id: 'a" OR 1=1', name: 'n' }],
      [{ id: '', name: 'n' }],
      [{ id: 'a', name: 'n'.repeat(201) }],
      [{ id: 'a' }],
      [null],
      Array.from({ length: 31 }, (_, i) => ({ id: `u${i}`, name: 'n' }))
    ]) {
      expect(() => store.setUi({ jiraSites: site(bad) })).toThrow('Jira 설정')
    }
  })
})
