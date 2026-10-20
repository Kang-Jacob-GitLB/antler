// Jira REST 응답 → 공유 타입 변환과 엔드포인트 호출. Cloud(API v3, ADF)와 Data Center/Server(API v2, 서버 렌더 HTML)의 차이는 여기서 흡수한다.
import type { JiraComment, JiraDevCommit, JiraDevInfo, JiraDevPull, JiraIssueDetail, JiraIssueSummary, JiraSearchResult, JiraUser, JiraWorklog, JiraWorklogQuery, JiraWorklogResult } from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
import { adfToHtml } from './adf'
import { JIRA_TOO_BIG, JiraApiError, type JiraClient, type JiraCredential } from './client'
import { jqlQuote, MAX_PAGES, PAGE_SIZE, USER_ID_RE } from './validate'

const LIST_FIELDS = 'summary,status,issuetype,priority,assignee,updated'
const DETAIL_FIELDS = `${LIST_FIELDS},reporter,created,description`
const COMMENT_COUNT = 5

type Json = Record<string, unknown>
const obj = (v: unknown): Json => (v && typeof v === 'object' ? (v as Json) : {})
const str = (v: unknown): string => (typeof v === 'string' ? v : '')

const apiBase = (c: JiraCredential): string => (c.kind === 'cloud' ? '/rest/api/3' : '/rest/api/2')

export function mapSummary(issue: unknown): JiraIssueSummary {
  const i = obj(issue)
  const f = obj(i['fields'])
  const status = obj(f['status'])
  return {
    key: str(i['key']),
    summary: str(f['summary']),
    status: str(status['name']),
    statusCategory: str(obj(status['statusCategory'])['key']),
    issueType: str(obj(f['issuetype'])['name']),
    priority: str(obj(f['priority'])['name']),
    assignee: str(obj(f['assignee'])['displayName']),
    updated: str(f['updated'])
  }
}

/** 서버가 줄 수 있는 평문(위키 마크업)을 문단 HTML 로. 렌더 HTML 이 없을 때의 대체 */
function plainToHtml(text: string): string {
  const esc = (s: string): string => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  return text.trim() ? `<p>${esc(text).replace(/\r?\n/g, '<br>')}</p>` : ''
}

/** 본문 하나의 HTML. Cloud 는 ADF 를 변환하고, Server 는 expand 로 받은 서버 렌더 HTML(없으면 평문) */
function bodyHtml(c: JiraCredential, raw: unknown, rendered: unknown): string {
  if (c.kind === 'cloud') return typeof raw === 'string' ? plainToHtml(raw) : adfToHtml(raw)
  return typeof rendered === 'string' && rendered ? rendered : plainToHtml(str(raw))
}

export async function myself(client: JiraClient, c: JiraCredential, signal?: AbortSignal): Promise<string> {
  const me = obj(await client.get<unknown>(`${apiBase(c)}/myself`, {
      credential: c,
      notFound: '이 주소에서 Jira API 를 찾을 수 없습니다(사이트 주소와 Cloud/Server 종류를 확인하세요)',
      ...(signal ? { signal } : {})
    })
  )
  return str(me['displayName']) || str(me['name']) || str(me['emailAddress'])
}

export async function getIssue(client: JiraClient, c: JiraCredential, key: string, signal?: AbortSignal): Promise<JiraIssueDetail> {
  const raw = obj(
    await client.get<unknown>(`${apiBase(c)}/issue/${encodeURIComponent(key)}`, {
      // Cloud 는 ADF 를 직접 변환하므로 서버 렌더 HTML 이 필요 없다
      query: { fields: DETAIL_FIELDS, expand: c.kind === 'server' ? 'renderedFields' : undefined },
      notFound: '이슈를 찾을 수 없거나 볼 권한이 없습니다',
      ...(signal ? { signal } : {})
    })
  )
  const f = obj(raw['fields'])
  const summary = mapSummary(raw)
  return {
    ...summary,
    // 응답의 key 가 아니라 요청한 key 로 주소를 만든다(이동된 이슈는 Jira 가 /browse/옛키 를 새 키로 연결한다)
    url: `${c.siteUrl}/browse/${encodeURIComponent(key)}`,
    reporter: str(obj(f['reporter'])['displayName']),
    created: str(f['created']),
    descriptionHtml: bodyHtml(c, f['description'], obj(raw['renderedFields'])['description'])
  }
}

export async function getComments(client: JiraClient, c: JiraCredential, key: string, signal?: AbortSignal): Promise<JiraComment[]> {
  const path = `${apiBase(c)}/issue/${encodeURIComponent(key)}/comment`
  const common = { notFound: '이슈를 찾을 수 없거나 볼 권한이 없습니다', ...(signal ? { signal } : {}) }
  let list: unknown[]
  if (c.kind === 'cloud') {
    // Cloud 는 orderBy 로 최신순을 직접 받는다
    const raw = obj(await client.get<unknown>(path, { query: { orderBy: '-created', maxResults: COMMENT_COUNT }, ...common }))
    list = Array.isArray(raw['comments']) ? (raw['comments'] as unknown[]) : []
  } else {
    // Data Center 는 정렬 인자가 문서화되어 있지 않다 — 첫 페이지의 total 로 마지막 페이지(최신 N개)를 받아 뒤집는다
    const query = { maxResults: COMMENT_COUNT, expand: 'renderedBody' }
    let raw = obj(await client.get<unknown>(path, { query: { ...query, startAt: 0 }, ...common }))
    const total = typeof raw['total'] === 'number' ? raw['total'] : 0
    if (total > COMMENT_COUNT) raw = obj(await client.get<unknown>(path, { query: { ...query, startAt: Math.max(0, total - COMMENT_COUNT) }, ...common }))
    list = (Array.isArray(raw['comments']) ? (raw['comments'] as unknown[]) : []).reverse()
  }
  return list.slice(0, COMMENT_COUNT).map((x) => {
    const cm = obj(x)
    return { author: str(obj(cm['author'])['displayName']), created: str(cm['created']), bodyHtml: bodyHtml(c, cm['body'], cm['renderedBody']) }
  })
}

const DEV_MAX = 30
const DEV_INSTANCE_RE = /^[A-Za-z0-9_.-]{1,40}$/
const NO_DEV: JiraDevInfo = { available: false, pulls: [], commits: [] }

const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : [])
const httpsUrl = (v: unknown): string => (typeof v === 'string' && /^https:\/\/[^\s]+$/.test(v) ? v : '')
/** dev-status 의 시각은 ISO 문자열 또는 epoch ms 로 온다 */
function isoTime(v: unknown): string {
  if (typeof v === 'number' && Number.isFinite(v)) return new Date(v).toISOString()
  return typeof v === 'string' && Number.isFinite(Date.parse(v)) ? v : ''
}
function pullState(v: unknown): JiraDevPull['state'] {
  const s = str(v).toUpperCase()
  return s === 'OPEN' ? 'open' : s === 'MERGED' ? 'merged' : 'closed'
}

/**
 * 개발 정보(dev-status). Jira 화면의 '개발' 칸이 쓰는 API 로, Jira 에 GitHub 연동(GitHub for Jira · DVCS)이 있어야 데이터가 있다.
 * 이슈 숫자 id → 요약에서 연동 종류(byInstanceType) → 종류별 PR · 커밋 상세. 연동이 없거나 API 가 막혀 있으면 available:false.
 */
export async function getDevInfo(client: JiraClient, c: JiraCredential, key: string, signal?: AbortSignal): Promise<JiraDevInfo> {
  const sig = signal ? { signal } : {}
  const issue = obj(
    await client.get<unknown>(`${apiBase(c)}/issue/${encodeURIComponent(key)}`, {
      query: { fields: 'issuetype' },
      notFound: '이슈를 찾을 수 없거나 볼 권한이 없습니다',
      ...sig
    })
  )
  const id = str(issue['id'])
  if (!/^\d{1,18}$/.test(id)) return NO_DEV

  // 연동이 없는 사이트는 404·403 을 줄 수 있다 — 오류가 아니라 '정보 없음'으로 본다(취소만 그대로 던진다)
  const soft = async (path: string, query: Record<string, string>): Promise<Json | null> => {
    try {
      return obj(await client.get<unknown>(path, { query, ...sig }))
    } catch (err) {
      if (err instanceof AbortError) throw err
      return null
    }
  }
  const summaryRaw = await soft('/rest/dev-status/latest/issue/summary', { issueId: id })
  if (!summaryRaw) return NO_DEV
  const summary = obj(summaryRaw['summary'])
  const instances = (part: string): string[] => Object.keys(obj(obj(summary[part])['byInstanceType'])).filter((t) => DEV_INSTANCE_RE.test(t)).slice(0, 4)
  const details = async (dataType: string, types: string[]): Promise<Json[]> => {
    const out: Json[] = []
    for (const applicationType of types) {
      const raw = await soft('/rest/dev-status/latest/issue/detail', { issueId: id, applicationType, dataType })
      if (raw) out.push(...arr(raw['detail']).map(obj))
    }
    return out
  }

  const pulls = new Map<string, JiraDevPull>()
  for (const d of await details('pullrequest', instances('pullrequest'))) {
    for (const x of arr(d['pullRequests'])) {
      const p = obj(x)
      const url = httpsUrl(p['url'])
      const pull: JiraDevPull = {
        title: str(p['name']),
        url,
        state: pullState(p['status']),
        author: str(obj(p['author'])['name']),
        repo: str(p['repositoryName']),
        branch: str(obj(p['source'])['branch']),
        updated: isoTime(p['lastUpdate'])
      }
      pulls.set(url || `${pull.repo}#${pull.title}`, pull)
    }
  }

  const commits = new Map<string, JiraDevCommit>()
  for (const d of await details('repository', instances('repository'))) {
    for (const r of arr(d['repositories'])) {
      const repo = obj(r)
      for (const x of arr(repo['commits'])) {
        const cm = obj(x)
        const sha = str(cm['id'])
        if (!/^[0-9a-f]{7,64}$/i.test(sha)) continue
        commits.set(sha, {
          sha,
          message: str(cm['message']).split(/\r?\n/)[0]!.trim(),
          url: httpsUrl(cm['url']),
          author: str(obj(cm['author'])['name']),
          repo: str(repo['name']),
          date: isoTime(cm['authorTimestamp'])
        })
      }
    }
  }

  const byTime = <T>(at: (x: T) => string) => (a: T, b: T): number => (Date.parse(at(b)) || 0) - (Date.parse(at(a)) || 0)
  return {
    available: true,
    pulls: [...pulls.values()].sort(byTime((p) => p.updated)).slice(0, DEV_MAX),
    commits: [...commits.values()].sort(byTime((x) => x.date)).slice(0, DEV_MAX)
  }
}

/**
 * JQL 검색. 한 번에 PAGE_SIZE 건, 이어 받기는 MAX_PAGES 페이지까지.
 * Cloud 는 /search/jql 의 nextPageToken, Data Center 는 /search 의 startAt 을 표지로 쓴다.
 */
export async function search(
  client: JiraClient,
  c: JiraCredential,
  q: { jql: string; cursor?: string | undefined; page: number },
  signal?: AbortSignal
): Promise<JiraSearchResult> {
  const sig = { notFound: '검색 API 를 찾을 수 없습니다(Jira 버전과 사이트 주소를 확인하세요)', ...(signal ? { signal } : {}) }
  if (c.kind === 'cloud') {
    const raw = obj(
      await client.get<unknown>('/rest/api/3/search/jql', {
        query: { jql: q.jql, fields: LIST_FIELDS, maxResults: PAGE_SIZE, nextPageToken: q.cursor },
        ...sig
      })
    )
    const items = (Array.isArray(raw['issues']) ? (raw['issues'] as unknown[]) : []).map(mapSummary)
    const next = str(raw['nextPageToken'])
    return { items, ...(next && raw['isLast'] !== true && q.page < MAX_PAGES ? { nextCursor: next } : {}) }
  }
  const startAt = q.cursor ? Number(q.cursor) : 0
  if (!Number.isInteger(startAt) || startAt < 0 || startAt > MAX_PAGES * PAGE_SIZE) throw new Error('잘못된 페이지 표지입니다')
  const raw = obj(
    await client.get<unknown>('/rest/api/2/search', {
      query: { jql: q.jql, fields: LIST_FIELDS, maxResults: PAGE_SIZE, startAt },
      ...sig
    })
  )
  const items = (Array.isArray(raw['issues']) ? (raw['issues'] as unknown[]) : []).map(mapSummary)
  const end = startAt + items.length
  const total = typeof raw['total'] === 'number' ? raw['total'] : end
  return { items, ...(items.length > 0 && end < total && q.page < MAX_PAGES ? { nextCursor: String(end) } : {}) }
}

// ── 워크로그 ────────────────────────────────────────────

const WORKLOG_ISSUE_MAX = 300
const WORKLOG_MAX = 20_000
/** 워크로그를 내장해 받으므로 페이지가 크면 응답이 커진다 */
const WORKLOG_ISSUE_PAGE = 25
/** 이슈별 worklog 엔드포인트만 허용하는 더 큰 응답 상한 */
const WORKLOG_BODY_MAX_BYTES = 20 * 1024 * 1024
const WORKLOG_FETCH_CONCURRENCY = 3
/** 이슈 하나의 워크로그를 이어 받는 최대 횟수(진전이 없으면 그 전에 멈춘다) */
const WORKLOG_FETCH_PAGES = 20
const COMMENT_MAX = 500
const DAY_MS = 86_400_000
/** ConfigStore team.name 상한과 맞춘다 */
const USER_NAME_MAX = 200
// eslint-disable-next-line no-control-regex
const CONTROL_RE = /[\u0000-\u001f\u007f]/

/** /myself 의 내 id: Cloud 는 accountId, Data Center 는 사용자 이름(name). 워크로그 작성자와 같은 체계다 */
export async function getMyId(client: JiraClient, c: JiraCredential, signal?: AbortSignal): Promise<JiraUser> {
  const me = obj(
    await client.get<unknown>(`${apiBase(c)}/myself`, {
      notFound: '이 주소에서 Jira API 를 찾을 수 없습니다(사이트 주소와 Cloud/Server 종류를 확인하세요)',
      ...(signal ? { signal } : {})
    })
  )
  const id = c.kind === 'cloud' ? str(me['accountId']) : str(me['name'])
  // JQL 에는 직접 넣지 않고 작성자 비교에 쓰므로 USER_ID_RE 보다 느슨하게(Data Center 이름은 공백·한글도 가능) 길이·제어문자만 본다
  if (id.length < 1 || id.length > 255 || CONTROL_RE.test(id)) throw new Error('Jira 계정 정보를 읽을 수 없습니다')
  return { id, name: (str(me['displayName']) || id).slice(0, USER_NAME_MAX) }
}

/** 사용자 검색(팀원 추가용). 최대 20명. Cloud 는 사람 계정(atlassian)만 */
export async function searchUsers(client: JiraClient, c: JiraCredential, text: string, signal?: AbortSignal): Promise<JiraUser[]> {
  const sig = signal ? { signal } : {}
  const raw =
    c.kind === 'cloud'
      ? await client.get<unknown>('/rest/api/3/user/search', { query: { query: text, maxResults: 20 }, ...sig })
      : await client.get<unknown>('/rest/api/2/user/search', { query: { username: text, maxResults: 20 }, ...sig })
  const out: JiraUser[] = []
  for (const x of arr(raw)) {
    const u = obj(x)
    if (c.kind === 'cloud' && u['accountType'] !== 'atlassian') continue
    const id = c.kind === 'cloud' ? str(u['accountId']) : str(u['name'])
    if (USER_ID_RE.test(id)) out.push({ id, name: (str(u['displayName']) || id).slice(0, USER_NAME_MAX) })
  }
  return out.slice(0, 20)
}

/** 워크로그 comment 를 평문으로: Cloud 는 ADF 의 텍스트 노드를 이어 붙이고, Data Center 는 문자열 그대로 */
function commentText(v: unknown): string {
  if (typeof v === 'string') return v.slice(0, COMMENT_MAX)
  const parts: string[] = []
  let len = 0
  const walk = (node: unknown, depth: number): void => {
    if (depth > 20 || len > COMMENT_MAX || !node || typeof node !== 'object') return
    const n = node as Json
    if (typeof n['text'] === 'string') {
      parts.push(n['text'])
      len += n['text'].length
    } else if (n['type'] === 'hardBreak') {
      parts.push(' ')
    }
    for (const ch of arr(n['content'])) walk(ch, depth + 1)
    if (n['type'] === 'paragraph') parts.push(' ')
  }
  walk(v, 0)
  return parts.join('').trim().slice(0, COMMENT_MAX)
}

/** 'YYYY-MM-DD' 에 일수를 더한다(UTC 달력) */
const shiftDay = (ymd: string, days: number): string => new Date(Date.parse(`${ymd}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10)

interface WorklogIssue {
  key: string
  summary: string
  type: string
  /** 내장 fields.worklog 의 항목 */
  embedded: unknown[]
  /** 내장 worklog 가 전부인지. total 이 더 크면 이슈별로 다시 받아야 한다 */
  complete: boolean
}

function mapIssueForWorklog(x: unknown): WorklogIssue {
  const i = obj(x)
  const f = obj(i['fields'])
  const w = obj(f['worklog'])
  const embedded = arr(w['worklogs'])
  const total = typeof w['total'] === 'number' ? w['total'] : embedded.length
  return { key: str(i['key']), summary: str(f['summary']), type: str(obj(f['issuetype'])['name']), embedded, complete: total <= embedded.length }
}

/** worklogAuthor·worklogDate JQL 로 이슈를 최대 WORKLOG_ISSUE_MAX 건 모은다(키로 중복 제거). 더 있으면 truncated */
async function findWorklogIssues(client: JiraClient, c: JiraCredential, jql: string, signal?: AbortSignal): Promise<{ issues: WorklogIssue[]; truncated: boolean }> {
  const sig = { notFound: '검색 API 를 찾을 수 없습니다(Jira 버전과 사이트 주소를 확인하세요)', ...(signal ? { signal } : {}) }
  const issues: WorklogIssue[] = []
  const seen = new Set<string>()
  const fields = 'summary,issuetype,worklog'
  let token: string | undefined
  let startAt = 0
  for (let page = 0; page <= Math.ceil(WORKLOG_ISSUE_MAX / WORKLOG_ISSUE_PAGE); page++) {
    let items: unknown[]
    let more: boolean
    if (c.kind === 'cloud') {
      const raw = obj(await client.get<unknown>('/rest/api/3/search/jql', { query: { jql, fields, maxResults: WORKLOG_ISSUE_PAGE, nextPageToken: token }, ...sig }))
      items = arr(raw['issues'])
      token = str(raw['nextPageToken']) || undefined
      more = token !== undefined && raw['isLast'] !== true
    } else {
      const raw = obj(await client.get<unknown>('/rest/api/2/search', { query: { jql, fields, maxResults: WORKLOG_ISSUE_PAGE, startAt }, ...sig }))
      items = arr(raw['issues'])
      startAt += items.length
      more = typeof raw['total'] === 'number' && startAt < raw['total']
    }
    for (const it of items) {
      const issue = mapIssueForWorklog(it)
      // 페이지 경계에서 같은 이슈가 다시 오면(정렬이 흔들릴 때) 한 번만 쓴다
      if (!issue.key || seen.has(issue.key)) continue
      seen.add(issue.key)
      issues.push(issue)
    }
    if (items.length === 0 || !more) return { issues: issues.slice(0, WORKLOG_ISSUE_MAX), truncated: issues.length > WORKLOG_ISSUE_MAX }
    if (issues.length >= WORKLOG_ISSUE_MAX) return { issues: issues.slice(0, WORKLOG_ISSUE_MAX), truncated: true }
  }
  return { issues: issues.slice(0, WORKLOG_ISSUE_MAX), truncated: true }
}

/** 이슈의 워크로그를 받는 대로 accept 에 넘긴다(원본을 쌓지 않는다). Cloud 는 범위로 좁히고, startAt 페이징은 진전이 없으면 멈춘다 */
async function fetchIssueWorklogs(client: JiraClient, c: JiraCredential, key: string, lo: number, hi: number, accept: (w: unknown) => void, signal?: AbortSignal): Promise<void> {
  const path = `${apiBase(c)}/issue/${encodeURIComponent(key)}/worklog`
  const seen = new Set<string>()
  let startAt = 0
  for (let page = 0; page < WORKLOG_FETCH_PAGES; page++) {
    // Data Center 는 쿼리 파라미터를 받지 않고 전부 돌려준다 — 거르는 것은 호출자의 몫
    const query = c.kind === 'cloud' ? { startAt, maxResults: 1000, startedAfter: lo, startedBefore: hi } : undefined
    const raw = obj(
      await client.get<unknown>(path, { ...(query ? { query } : {}), notFound: '이슈를 찾을 수 없거나 볼 권한이 없습니다', maxBytes: WORKLOG_BODY_MAX_BYTES, ...(signal ? { signal } : {}) })
    )
    const list = arr(raw['worklogs'])
    let fresh = 0
    for (const w of list) {
      const id = str(obj(w)['id'])
      if (id && seen.has(id)) continue
      if (id) seen.add(id)
      accept(w)
      fresh++
    }
    startAt += list.length
    const total = typeof raw['total'] === 'number' ? raw['total'] : 0
    if (fresh === 0 || startAt >= total) break
  }
}

/** 그 이슈만 건너뛰어도 되는 오류: 이슈가 없거나(404) 볼 수 없거나(403) 워크로그 응답이 너무 큰 경우 */
function skippableIssueError(err: unknown): boolean {
  if (!(err instanceof JiraApiError)) return false
  return err.code === 'JIRA_NOT_FOUND' || err.code === 'JIRA_FORBIDDEN' || (err.code === 'JIRA_HTTP' && err.message === JIRA_TOO_BIG)
}

/** JQL 작성자 절. 내 id 가 JQL 에 들어갈 수 없는 모양이면 currentUser() 로 대신한다 */
function authorClause(team: string[], me: JiraUser, includeMe: boolean): string {
  if (team.length === 0) return 'worklogAuthor = currentUser()'
  const list = (ids: string[]): string => `worklogAuthor in (${ids.map(jqlQuote).join(',')})`
  if (!includeMe) return list(team)
  if (USER_ID_RE.test(me.id)) return list([...team, me.id])
  return `(worklogAuthor = currentUser() OR ${list(team)})`
}

/** 검색이 400 일 때: 팀원을 한 명씩 가볍게 조회해 Jira 가 거부하는 id 를 골라낸다 */
async function findInvalidUsers(client: JiraClient, c: JiraCredential, team: string[], signal?: AbortSignal): Promise<string[]> {
  const bad: string[] = []
  const path = c.kind === 'cloud' ? '/rest/api/3/search/jql' : '/rest/api/2/search'
  for (const id of team) {
    try {
      await client.get<unknown>(path, { query: { jql: `worklogAuthor = ${jqlQuote(id)}`, fields: 'key', maxResults: 1 }, ...(signal ? { signal } : {}) })
    } catch (err) {
      if (err instanceof JiraApiError && err.status === 400) bad.push(id)
      else throw err
    }
  }
  return bad
}

/**
 * 범위 안의 워크로그(GET 만). JQL 날짜는 사용자 프로필 시간대 기준일 수 있어 범위를 이틀씩 넓혀 이슈를 찾고,
 * started 가 [from-2일 00:00Z, to+2일 00:00Z+14h) 인 것만 남긴다(UTC+14 ~ UTC-12 를 덮는다) — 정확한 로컬 날짜 필터는 화면의 몫이다.
 */
export async function getWorklogs(
  client: JiraClient,
  c: JiraCredential,
  q: JiraWorklogQuery,
  me: JiraUser,
  signal?: AbortSignal
): Promise<JiraWorklogResult> {
  const includeMe = q.users.length > 0 && q.includeMe === true
  const lo = Date.parse(`${q.from}T00:00:00Z`) - 2 * DAY_MS
  const hi = Date.parse(`${q.to}T00:00:00Z`) + 2 * DAY_MS + 14 * 3_600_000
  const dates = `worklogDate >= ${jqlQuote(shiftDay(q.from, -2))} AND worklogDate < ${jqlQuote(shiftDay(q.to, 2))} ORDER BY key ASC`

  let team = q.users
  let invalidUsers: string[] = []
  let found: { issues: WorklogIssue[]; truncated: boolean }
  try {
    found = await findWorklogIssues(client, c, `${authorClause(team, me, includeMe)} AND ${dates}`, signal)
  } catch (err) {
    // 400: 없는 사용자(이름 변경·삭제)가 섞였을 수 있다. 팀원이 있을 때만 하나씩 확인해 빼고 다시 조회한다
    if (!(err instanceof JiraApiError) || err.status !== 400 || team.length === 0) throw err
    invalidUsers = await findInvalidUsers(client, c, team, signal)
    if (invalidUsers.length === 0) throw err
    team = team.filter((u) => !invalidUsers.includes(u))
    if (team.length === 0 && !includeMe) return { worklogs: [], me, truncated: false, invalidUsers }
    found = await findWorklogIssues(client, c, `${authorClause(team, me, includeMe)} AND ${dates}`, signal)
  }
  const { issues } = found
  let truncated = found.truncated

  const wanted = new Set(q.users.length > 0 ? team : [me.id])
  if (includeMe) wanted.add(me.id)

  const worklogs: JiraWorklog[][] = issues.map(() => [])
  let count = 0
  /** 워크로그 하나를 작성자·범위로 거르고 JiraWorklog 로 바꿔 담는다. 원본은 남기지 않는다 */
  const take = (idx: number, x: unknown): void => {
    const w = obj(x)
    const a = obj(w['author'])
    // Cloud 는 accountId, Data Center 는 name(옛 key 로도 맞춘다)
    const authorId = c.kind === 'cloud' ? str(a['accountId']) : str(a['name'])
    if (!authorId || !(wanted.has(authorId) || (c.kind === 'server' && wanted.has(str(a['key']))))) return
    const started = str(w['started'])
    const t = Date.parse(started)
    if (!Number.isFinite(t) || t < lo || t >= hi) return
    if (count >= WORKLOG_MAX) {
      truncated = true
      return
    }
    count++
    const issue = issues[idx]!
    worklogs[idx]!.push({
      id: str(w['id']),
      issueKey: issue.key,
      issueSummary: issue.summary,
      issueType: issue.type,
      authorId,
      author: str(a['displayName']) || authorId,
      started,
      seconds: typeof w['timeSpentSeconds'] === 'number' ? w['timeSpentSeconds'] : 0,
      comment: commentText(w['comment'])
    })
  }

  // 내장 worklog 가 전부인 이슈는 바로 쓰고, 잘린 이슈만 이슈별로 다시 받는다(동시 3개)
  const todo: number[] = []
  for (const [idx, issue] of issues.entries()) {
    if (!issue.complete) todo.push(idx)
    else for (const x of issue.embedded) take(idx, x)
    issue.embedded = []
  }
  // 전체 실패가 확정되는 첫 오류에서 나머지 워커를 멈춘다
  const local = new AbortController()
  const wsig = signal ? AbortSignal.any([signal, local.signal]) : local.signal
  let next = 0
  const worker = async (): Promise<void> => {
    while (next < todo.length) {
      if (wsig.aborted) throw new AbortError()
      const idx = todo[next++]!
      try {
        await fetchIssueWorklogs(client, c, issues[idx]!.key, lo, hi, (x) => take(idx, x), wsig)
      } catch (err) {
        if (skippableIssueError(err)) {
          // 이 이슈만 건너뛰고 부분 결과로 돌려준다
          count -= worklogs[idx]!.length
          worklogs[idx] = []
          truncated = true
          continue
        }
        local.abort()
        throw err
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(WORKLOG_FETCH_CONCURRENCY, todo.length) }, worker))

  return { worklogs: worklogs.flat(), me, truncated, ...(invalidUsers.length > 0 ? { invalidUsers } : {}) }
}
