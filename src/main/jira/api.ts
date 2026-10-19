// Jira REST 응답 → 공유 타입 변환과 엔드포인트 호출. Cloud(API v3, ADF)와 Data Center/Server(API v2, 서버 렌더 HTML)의 차이는 여기서 흡수한다.
import type { JiraComment, JiraIssueDetail, JiraIssueSummary, JiraSearchResult } from '@shared/types'
import { adfToHtml } from './adf'
import type { JiraClient, JiraCredential } from './client'
import { MAX_PAGES, PAGE_SIZE } from './validate'

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
  const me = obj(await client.get<unknown>(`${apiBase(c)}/myself`, { credential: c, ...(signal ? { signal } : {}) }))
  return str(me['displayName']) || str(me['name']) || str(me['emailAddress'])
}

export async function getIssue(client: JiraClient, c: JiraCredential, key: string, signal?: AbortSignal): Promise<JiraIssueDetail> {
  const raw = obj(
    await client.get<unknown>(`${apiBase(c)}/issue/${encodeURIComponent(key)}`, {
      query: { fields: DETAIL_FIELDS, expand: 'renderedFields' },
      ...(signal ? { signal } : {})
    })
  )
  const f = obj(raw['fields'])
  const summary = mapSummary(raw)
  return {
    ...summary,
    // 응답의 key 가 아니라 요청한 key 로 주소를 만든다(이동된 이슈는 새 key 가 올 수 있다)
    url: `${c.siteUrl}/browse/${encodeURIComponent(summary.key || key)}`,
    reporter: str(obj(f['reporter'])['displayName']),
    created: str(f['created']),
    descriptionHtml: bodyHtml(c, f['description'], obj(raw['renderedFields'])['description'])
  }
}

export async function getComments(client: JiraClient, c: JiraCredential, key: string, signal?: AbortSignal): Promise<JiraComment[]> {
  const raw = obj(
    await client.get<unknown>(`${apiBase(c)}/issue/${encodeURIComponent(key)}/comment`, {
      query: { orderBy: '-created', maxResults: COMMENT_COUNT, expand: 'renderedBody' },
      ...(signal ? { signal } : {})
    })
  )
  const list = Array.isArray(raw['comments']) ? (raw['comments'] as unknown[]) : []
  return list.slice(0, COMMENT_COUNT).map((x) => {
    const cm = obj(x)
    return { author: str(obj(cm['author'])['displayName']), created: str(cm['created']), bodyHtml: bodyHtml(c, cm['body'], cm['renderedBody']) }
  })
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
  const sig = signal ? { signal } : {}
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
