// renderer 가 보낸 Jira IPC 인자 검증. IPC 인자는 신뢰하지 않는다.
import type { JiraConnectInput, JiraKind, JiraSearchQuery } from '@shared/types'

const MAX_TOKEN_LEN = 1024
const MAX_EMAIL_LEN = 254
const MAX_URL_LEN = 2048
export const MAX_JQL_LEN = 2000
/** 목록은 한 번에 이 개수씩, 페이지는 이 횟수까지만 이어 받는다(최대 MAX_PAGES * PAGE_SIZE 건) */
export const PAGE_SIZE = 25
export const MAX_PAGES = 8
// eslint-disable-next-line no-control-regex
const CONTROL_RE = /[\u0000-\u001f\u007f]/
const KEY_RE = /^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$/
const CURSOR_RE = /^[!-~]{1,2000}$/

export interface SiteOptions {
  /** 테스트용 가짜 서버(127.0.0.1·localhost)에 한해 http 를 허용한다. 실제 사용에서는 항상 false */
  allowInsecureLoopback?: boolean
}

/**
 * 사이트 주소를 정규화한다: https 만(자격 증명·쿼리·해시 거부), 끝의 '/' 제거. Data Center 의 컨텍스트 경로(/jira)는 유지한다.
 * 반환값은 origin + 경로이며 API 경로를 그대로 이어 붙일 수 있다.
 */
export function parseSiteUrl(raw: unknown, opts: SiteOptions = {}): string {
  if (typeof raw !== 'string' || !raw.trim() || raw.length > MAX_URL_LEN || CONTROL_RE.test(raw)) throw new Error('Jira 사이트 주소가 올바르지 않습니다')
  let u: URL
  try {
    u = new URL(raw.trim())
  } catch {
    throw new Error('Jira 사이트 주소가 올바르지 않습니다(예: https://회사.atlassian.net)')
  }
  const loopback = u.hostname === '127.0.0.1' || u.hostname === 'localhost'
  if (u.protocol !== 'https:' && !(u.protocol === 'http:' && opts.allowInsecureLoopback === true && loopback)) {
    throw new Error('Jira 사이트 주소는 https:// 로 시작해야 합니다')
  }
  if (u.username || u.password) throw new Error('Jira 사이트 주소에 사용자 정보를 넣을 수 없습니다')
  if (u.search || u.hash) throw new Error('Jira 사이트 주소에 쿼리·해시를 넣을 수 없습니다')
  return u.origin + u.pathname.replace(/\/+$/, '')
}

export function parseConnectInput(v: unknown, opts: SiteOptions = {}): { kind: JiraKind; siteUrl: string; email: string; token: string } {
  const i = v as Partial<JiraConnectInput> | null
  if (!i || typeof i !== 'object' || (i.kind !== 'cloud' && i.kind !== 'server')) throw new Error('잘못된 연결 정보입니다')
  const siteUrl = parseSiteUrl(i.siteUrl, opts)
  if (typeof i.token !== 'string') throw new Error('토큰이 올바르지 않습니다')
  const token = i.token.trim()
  if (!token || token.length > MAX_TOKEN_LEN || !/^[\x21-\x7e]+$/.test(token)) throw new Error('토큰 형식이 올바르지 않습니다(공백 없이 1024자 이하)')
  let email = ''
  if (i.kind === 'cloud') {
    email = typeof i.email === 'string' ? i.email.trim() : ''
    // Basic 인증의 사용자 이름이므로 ':' 은 쓸 수 없다
    if (!email || email.length > MAX_EMAIL_LEN || CONTROL_RE.test(email) || email.includes(':')) throw new Error('이메일이 올바르지 않습니다')
  }
  return { kind: i.kind, siteUrl, email, token }
}

export function parseIssueKey(v: unknown): string {
  if (typeof v !== 'string' || !KEY_RE.test(v)) throw new Error('잘못된 이슈 키입니다')
  return v
}

export function parseSearchQuery(v: unknown): JiraSearchQuery & { page: number } {
  const q = v as Partial<JiraSearchQuery> | null
  if (!q || typeof q !== 'object' || typeof q.jql !== 'string') throw new Error('잘못된 조회 조건입니다')
  const jql = q.jql.trim()
  if (!jql || jql.length > MAX_JQL_LEN || CONTROL_RE.test(jql)) throw new Error('JQL 이 올바르지 않습니다(2000자 이하)')
  let page = 1
  if (q.page !== undefined) {
    if (typeof q.page !== 'number' || !Number.isInteger(q.page) || q.page < 1 || q.page > MAX_PAGES) throw new Error('잘못된 페이지입니다')
    page = q.page
  }
  if (q.cursor !== undefined && (typeof q.cursor !== 'string' || !CURSOR_RE.test(q.cursor))) throw new Error('잘못된 페이지 표지입니다')
  return { jql, page, ...(q.cursor ? { cursor: q.cursor } : {}) }
}

/** jira:openExternal 허용 대상: https(테스트에서는 허용된 루프백 http), origin 이 연결한 사이트와 정확히 같고 자격 증명이 없는 URL 만. 정규화된 href 를 돌려준다 */
export function allowedJiraUrl(raw: unknown, siteUrl: string | undefined, opts: SiteOptions = {}): string | null {
  if (typeof raw !== 'string' || raw.length > MAX_URL_LEN || !siteUrl) return null
  let u: URL
  let site: URL
  try {
    u = new URL(raw)
    site = new URL(siteUrl)
  } catch {
    return null
  }
  if (u.username || u.password || u.origin !== site.origin) return null
  if (u.protocol !== 'https:' && !(opts.allowInsecureLoopback === true && u.protocol === 'http:')) return null
  return u.href
}
