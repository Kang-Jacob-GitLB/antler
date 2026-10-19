// 현재 버전의 릴리스 노트(GitHub Release 본문, 마크다운)를 가져온다. 토큰 없이 공개 API 만 쓴다.
import type { ReleaseNotes } from '@shared/types'
import { REPO_NAME, REPO_OWNER, releaseNotesUrl } from './logic'

type FetchFn = typeof fetch

const API_BASE = `https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}`
const HEADERS = { 'User-Agent': 'Antler-updater', Accept: 'application/vnd.github+json' }
const MAX_API_BYTES = 2_000_000
/** 본문이 이보다 길면 자르고 '더보기'로 넘긴다 */
export const MAX_NOTES_CHARS = 100_000
export const RELEASES_PAGE = `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases`

interface ApiRelease {
  tag_name?: unknown
  name?: unknown
  body?: unknown
  html_url?: unknown
  published_at?: unknown
  draft?: unknown
}

async function getRelease(fetchFn: FetchFn, path: string, signal?: AbortSignal): Promise<ApiRelease | null> {
  const res = await fetchFn(`${API_BASE}${path}`, { headers: HEADERS, redirect: 'error', ...(signal ? { signal } : {}) })
  if (res.status === 404) return null
  if (res.status === 403 || res.status === 429) throw new Error('GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요')
  if (!res.ok) throw new Error(`릴리스 노트를 가져오지 못했습니다 (HTTP ${res.status})`)
  const len = Number(res.headers.get('content-length') ?? 0)
  const text = len > MAX_API_BYTES ? '' : await res.text()
  if (!text || text.length > MAX_API_BYTES) throw new Error('응답이 너무 큽니다')
  const json = JSON.parse(text) as unknown
  return json && typeof json === 'object' && !Array.isArray(json) ? (json as ApiRelease) : null
}

/**
 * 현재 버전(태그 v<버전>)의 릴리스 노트. 그 태그의 릴리스가 없으면(개발 빌드 등) 최신 정식 릴리스로 대신하고 exact=false.
 * 링크(url)는 항상 이 저장소의 릴리스 페이지로 만든다 — API 응답의 html_url 을 그대로 열지 않는다.
 */
export async function fetchReleaseNotes(fetchFn: FetchFn, current: string, signal?: AbortSignal): Promise<ReleaseNotes> {
  const version = current.replace(/^v/, '')
  let rel = /^[0-9A-Za-z.+-]{1,64}$/.test(version) ? await getRelease(fetchFn, `/releases/tags/v${encodeURIComponent(version)}`, signal) : null
  const exact = !!rel && rel.draft !== true
  if (!exact) rel = await getRelease(fetchFn, '/releases/latest', signal)
  if (!rel || typeof rel.tag_name !== 'string') return { version, exact: false, body: '', url: RELEASES_PAGE }
  const tagVersion = rel.tag_name.replace(/^v/, '')
  let body = typeof rel.body === 'string' ? rel.body : ''
  const truncated = body.length > MAX_NOTES_CHARS
  if (truncated) body = body.slice(0, MAX_NOTES_CHARS)
  return {
    version: tagVersion,
    exact,
    body,
    url: /^[0-9A-Za-z.+-]{1,64}$/.test(tagVersion) ? releaseNotesUrl(tagVersion) : RELEASES_PAGE,
    ...(typeof rel.name === 'string' && rel.name ? { title: rel.name.slice(0, 200) } : {}),
    ...(typeof rel.published_at === 'string' && !Number.isNaN(Date.parse(rel.published_at)) ? { publishedAt: rel.published_at } : {}),
    ...(truncated ? { truncated: true } : {})
  }
}
