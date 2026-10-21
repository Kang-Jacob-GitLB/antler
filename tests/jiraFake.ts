// Jira 테스트용 가짜 서버. 실제 Jira 에는 접속하지 않고, 루프백에 in-process HTTP 서버를 띄워 요청을 기록한다.
// (fetch 를 가짜로 바꾸면 리다이렉트 처리 같은 실제 동작이 검증되지 않으므로 진짜 소켓을 쓴다)
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export interface RecordedReq {
  method: string
  /** 경로 + 쿼리 */
  url: string
  headers: IncomingMessage['headers']
  /** 요청 본문(없으면 빈 문자열) */
  body: string
}

export type Handler = (req: RecordedReq, res: ServerResponse, n: number) => void

export interface FakeJira {
  /** http://127.0.0.1:<port> */
  origin: string
  /** 같은 서버를 localhost 이름으로 본 주소(다른 호스트 역할) */
  altOrigin: string
  requests: RecordedReq[]
  close(): Promise<void>
}

export async function startFakeJira(handler: Handler): Promise<FakeJira> {
  const requests: RecordedReq[] = []
  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = []
    req.on('data', (c: Buffer) => chunks.push(c))
    req.on('end', () => {
      const rec: RecordedReq = { method: req.method ?? 'GET', url: req.url ?? '/', headers: req.headers, body: Buffer.concat(chunks).toString('utf8') }
      requests.push(rec)
      handler(rec, res, requests.length)
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as AddressInfo).port
  return {
    origin: `http://127.0.0.1:${port}`,
    altOrigin: `http://localhost:${port}`,
    requests,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections()
        server.close(() => resolve())
      })
  }
}

export function sendJson(res: ServerResponse, body: unknown, status = 200, headers: Record<string, string> = {}): void {
  res.writeHead(status, { 'content-type': 'application/json', ...headers })
  res.end(JSON.stringify(body))
}

// ── 워크로그 가짜 서버 ────────────────────────────────

export interface FakeWorklog {
  id: string
  /** Cloud 는 accountId, Data Center 는 name */
  author: string
  /** Data Center 의 옛 key */
  authorKey?: string
  started: string
  seconds: number
  /** Cloud 는 ADF, Data Center 는 문자열 */
  comment?: unknown
}

export interface FakeWorklogIssue {
  key: string
  summary?: string
  type?: string
  worklogs: FakeWorklog[]
  /** 검색 응답에 내장하는 worklog 수(실제 Jira 는 20개 안팎으로 자른다). 기본은 전부 */
  embed?: number
  /** 이슈별 worklog 엔드포인트가 이 상태 코드로 실패한다 */
  worklogStatus?: number
  /** 이슈별 worklog 응답을 이 시간(ms) 뒤에 보낸다 */
  worklogDelayMs?: number
  /** 이슈별 worklog 응답을 21MB 로 부풀린다(20MB 상한 초과) */
  worklogHuge?: boolean
}

export interface FakeWorklogSite {
  kind: 'cloud' | 'server'
  issues: FakeWorklogIssue[]
  users?: Record<string, unknown>[]
  /** JQL 에 이 id 가 따옴표로 들어 있으면 검색이 400 으로 실패한다(없는 사용자) */
  badUsers?: string[]
}

/** 워크로그 조회에 필요한 엔드포인트(myself·search·issue worklog·user/search)를 흉내 내는 핸들러 */
export function worklogHandler(site: FakeWorklogSite): Handler {
  const cloud = site.kind === 'cloud'
  const toWorklog = (w: FakeWorklog): Record<string, unknown> => ({
    id: w.id,
    author: cloud ? { accountId: w.author, displayName: `표시-${w.author}` } : { name: w.author, key: w.authorKey ?? w.author, displayName: `표시-${w.author}` },
    started: w.started,
    timeSpentSeconds: w.seconds,
    ...(w.comment !== undefined ? { comment: w.comment } : {})
  })
  return (req, res) => {
    const u = new URL(req.url, 'http://x')
    const path = u.pathname
    const q = u.searchParams
    if (path.endsWith('/myself')) return sendJson(res, cloud ? { accountId: 'acc-me', displayName: '나' } : { name: 'me', key: 'JIRAUSER1', displayName: '나' })
    if (path.endsWith('/user/search')) return sendJson(res, site.users ?? [])
    const wl = /\/issue\/([^/]+)\/worklog$/.exec(path)
    if (wl) {
      const issue = site.issues.find((i) => i.key === wl[1])
      if (!issue) return sendJson(res, {}, 404)
      const reply = (): void => {
        if (issue.worklogStatus) return sendJson(res, { errorMessages: ['x'] }, issue.worklogStatus)
        if (issue.worklogHuge) return sendJson(res, { worklogs: [], pad: 'x'.repeat(21 * 1024 * 1024) })
        let list = issue.worklogs
        if (cloud) {
          const after = Number(q.get('startedAfter') ?? -Infinity)
          const before = Number(q.get('startedBefore') ?? Infinity)
          list = list.filter((w) => Date.parse(w.started) >= after && Date.parse(w.started) < before)
          const startAt = Number(q.get('startAt') ?? 0)
          const max = Number(q.get('maxResults') ?? 5000)
          return sendJson(res, { startAt, maxResults: max, total: list.length, worklogs: list.slice(startAt, startAt + max).map(toWorklog) })
        }
        return sendJson(res, { worklogs: list.map(toWorklog) })
      }
      if (issue.worklogDelayMs) setTimeout(reply, issue.worklogDelayMs)
      else reply()
      return
    }
    if (path.endsWith('/search/jql') || path.endsWith('/search')) {
      const jql = q.get('jql') ?? ''
      if ((site.badUsers ?? []).some((b) => jql.includes(`"${b}"`))) return sendJson(res, { errorMessages: ['사용자가 없습니다'] }, 400)
      const max = Number(q.get('maxResults') ?? 50)
      const start = cloud ? Number(q.get('nextPageToken') ?? 0) : Number(q.get('startAt') ?? 0)
      const page = site.issues.slice(start, start + max)
      const issues = page.map((i) => {
        const embed = i.embed ?? i.worklogs.length
        return {
          key: i.key,
          fields: { summary: i.summary ?? `요약 ${i.key}`, issuetype: { name: i.type ?? 'Task' }, worklog: { startAt: 0, maxResults: embed, total: i.worklogs.length, worklogs: i.worklogs.slice(0, embed).map(toWorklog) } }
        }
      })
      const end = start + page.length
      return sendJson(res, cloud ? { issues, ...(end < site.issues.length ? { nextPageToken: String(end) } : { isLast: true }) } : { issues, startAt: start, total: site.issues.length })
    }
    return sendJson(res, { errorMessages: ['nf'] }, 404)
  }
}
