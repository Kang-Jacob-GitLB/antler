// Jira REST 응답 → 공유 타입 변환과 엔드포인트 호출. Cloud(API v3, ADF)와 Data Center/Server(API v2, 서버 렌더 HTML)의 차이는 여기서 흡수한다.
import type { JiraComment, JiraDevCommit, JiraDevInfo, JiraDevPull, JiraIssueDetail, JiraIssueSummary, JiraSearchResult } from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
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
