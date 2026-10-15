// GitHub REST 클라이언트. 토큰은 tokenProvider 로만 읽고 요청 헤더에만 싣는다(반환값·에러 메시지·로그에 넣지 않는다).
import { createHash } from 'node:crypto'
import { AbortError } from '../executors/GitExecutor'
import { LruCache } from '../services/lru'

export const API_BASE = 'https://api.github.com'
export const API_VERSION = '2022-11-28'
const ACCEPT_JSON = 'application/vnd.github+json'
/** body_html(GitHub 가 렌더링·정화한 본문)를 함께 받는다 */
const ACCEPT_HTML = 'application/vnd.github.html+json'
const REQUEST_TIMEOUT_MS = 15_000
/** ETag 캐시 항목 수와 총 크기(추정 바이트) */
const CACHE_ENTRIES = 500
const CACHE_BYTES = 32 * 1024 * 1024

export type GhErrorCode = 'GH_AUTH' | 'GH_RATE_LIMIT' | 'GH_NOT_FOUND' | 'GH_FORBIDDEN' | 'GH_HTTP' | 'GH_NETWORK' | 'GH_TIMEOUT'

/** GitHub API 실패. 메시지는 그대로 사용자에게 보여 줘도 되는 한국어 문장이다 */
export class GitHubApiError extends Error {
  constructor(
    message: string,
    readonly code: GhErrorCode,
    readonly status?: number
  ) {
    super(message)
    this.name = 'GitHubApiError'
  }
}

export interface RateInfo {
  remaining: number
  limit: number
  /** epoch seconds */
  reset: number
}

export interface GetOptions {
  query?: Record<string, string | number | undefined>
  /** true 면 application/vnd.github.html+json 으로 받아 body_html 을 얻는다 */
  html?: boolean
  signal?: AbortSignal
  /** 저장된 토큰 대신 이 토큰으로 요청한다(로그인 전 검증용). null 이면 익명 */
  token?: string | null
}

export interface GetResult<T> {
  data: T
  /** Link 헤더에 rel="next" 가 있는지 */
  hasMore: boolean
}

interface CacheEntry {
  etag: string
  data: unknown
  hasMore: boolean
}

export type TokenProvider = () => string | null

export class GitHubClient {
  private readonly cache = new LruCache<string, CacheEntry>(CACHE_ENTRIES, CACHE_BYTES)
  /** 마지막으로 본 core 한도(search 는 별도 한도라 섞지 않는다) */
  rate: RateInfo | null = null

  constructor(
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly tokenProvider: TokenProvider = () => null,
    private readonly onRate?: (rate: RateInfo) => void
  ) {}

  hasToken(): boolean {
    return this.tokenProvider() !== null
  }

  clearCache(): void {
    this.cache.clear()
  }

  /**
   * REST GET. path 는 '/' 로 시작하는 API 경로(세그먼트는 호출자가 인코딩한다).
   * ETag 캐시 키는 (토큰 해시 + 전체 URL + Accept) — 계정이 바뀌면 다른 사용자의 비공개 응답이 섞이지 않는다.
   * 304 는 속도 제한에 포함되지 않으므로 같은 요청을 If-None-Match 로 다시 보내는 것이 싸다.
   */
  async get<T>(path: string, opts: GetOptions = {}): Promise<GetResult<T>> {
    if (!path.startsWith('/')) throw new Error('invalid api path')
    const url = new URL(API_BASE + path)
    if (url.origin !== API_BASE) throw new Error('invalid api path')
    for (const [k, v] of Object.entries(opts.query ?? {})) if (v !== undefined) url.searchParams.set(k, String(v))

    const token = opts.token !== undefined ? opts.token : this.tokenProvider()
    const accept = opts.html ? ACCEPT_HTML : ACCEPT_JSON
    const key = `${token ? createHash('sha256').update(token).digest('hex').slice(0, 16) : 'anon'}\0${url.href}\0${accept}`
    const cached = this.cache.get(key)

    const headers: Record<string, string> = {
      Accept: accept,
      'X-GitHub-Api-Version': API_VERSION,
      'User-Agent': 'Antler'
    }
    if (token) headers['Authorization'] = `Bearer ${token}`
    if (cached) headers['If-None-Match'] = cached.etag

    const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    const signal = opts.signal ? AbortSignal.any([timeout, opts.signal]) : timeout
    let res: Response
    try {
      res = await this.fetchImpl(url.href, { method: 'GET', headers, signal })
    } catch (err) {
      if (opts.signal?.aborted) throw new AbortError()
      if (timeout.aborted) throw new GitHubApiError(`GitHub 응답이 ${REQUEST_TIMEOUT_MS / 1000}초 안에 오지 않았습니다`, 'GH_TIMEOUT')
      // fetch 에러 객체에는 요청 헤더가 없지만, 원인 문자열만 남기고 나머지는 버린다
      const cause = err instanceof Error && err.cause instanceof Error ? err.cause.message : err instanceof Error ? err.message : ''
      throw new GitHubApiError(`GitHub 에 연결할 수 없습니다${cause ? `(${cause})` : ''}`, 'GH_NETWORK')
    }
    this.trackRate(res.headers)

    if (res.status === 304) {
      if (!cached) throw new GitHubApiError('GitHub 가 예상치 못한 304 응답을 보냈습니다', 'GH_HTTP', 304)
      return { data: cached.data as T, hasMore: cached.hasMore }
    }
    const text = await readText(res, opts.signal)
    if (!res.ok) throw httpError(res, text, token !== null)

    let data: unknown
    try {
      data = JSON.parse(text)
    } catch {
      throw new GitHubApiError('GitHub 응답을 해석할 수 없습니다', 'GH_HTTP', res.status)
    }
    const hasMore = hasNextLink(res.headers.get('link'))
    const etag = res.headers.get('etag')
    if (etag) this.cache.set(key, { etag, data, hasMore }, text.length * 2)
    return { data: data as T, hasMore }
  }

  private trackRate(h: Headers): void {
    // search 등 다른 resource 의 한도는 core 표시를 덮어쓰지 않는다
    const resource = h.get('x-ratelimit-resource')
    if (resource && resource !== 'core') return
    const rate = parseRate(h)
    if (!rate) return
    this.rate = rate
    this.onRate?.(rate)
  }
}

async function readText(res: Response, signal?: AbortSignal): Promise<string> {
  try {
    return await res.text()
  } catch (err) {
    if (signal?.aborted) throw new AbortError()
    throw new GitHubApiError('GitHub 응답을 읽는 중 연결이 끊겼습니다', 'GH_NETWORK', res.status)
  }
}

function parseRate(h: Headers): RateInfo | null {
  const remaining = Number(h.get('x-ratelimit-remaining'))
  const limit = Number(h.get('x-ratelimit-limit'))
  const reset = Number(h.get('x-ratelimit-reset'))
  if (h.get('x-ratelimit-remaining') === null || !Number.isFinite(remaining) || !Number.isFinite(limit) || !Number.isFinite(reset)) return null
  return { remaining, limit, reset }
}

/** Link 헤더에서 rel="next" 유무 */
export function hasNextLink(link: string | null): boolean {
  return link !== null && /<[^>]*>\s*;\s*rel="?next"?/i.test(link)
}

function formatTime(epochSec: number): string {
  return new Date(epochSec * 1000).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' })
}

/** 응답 본문의 message(있다면). 비밀이 아니지만 길이는 제한한다 */
function bodyMessage(text: string): string {
  try {
    const m = (JSON.parse(text) as { message?: unknown }).message
    return typeof m === 'string' ? m.slice(0, 200) : ''
  } catch {
    return ''
  }
}

function httpError(res: Response, text: string, signedIn: boolean): GitHubApiError {
  const s = res.status
  if (s === 401) return new GitHubApiError('GitHub 인증이 만료되었거나 잘못되었습니다. 다시 로그인하세요', 'GH_AUTH', s)
  if (s === 403 || s === 429) {
    const rate = parseRate(res.headers)
    const retryAfter = Number(res.headers.get('retry-after'))
    if (rate && rate.remaining === 0) {
      const hint = signedIn ? '' : ' 로그인하면 한도가 크게 늘어납니다.'
      return new GitHubApiError(`GitHub API 호출 한도를 초과했습니다. ${formatTime(rate.reset)} 이후에 다시 시도하세요.${hint}`, 'GH_RATE_LIMIT', s)
    }
    // 짧은 시간에 요청이 몰렸을 때의 보조(secondary) 한도
    if (Number.isFinite(retryAfter) && retryAfter > 0) {
      return new GitHubApiError(`GitHub 요청이 너무 잦습니다. ${Math.ceil(retryAfter)}초 후에 다시 시도하세요`, 'GH_RATE_LIMIT', s)
    }
    if (s === 429) return new GitHubApiError('GitHub 요청이 너무 잦습니다. 잠시 후 다시 시도하세요', 'GH_RATE_LIMIT', s)
    const msg = bodyMessage(text)
    return new GitHubApiError(`GitHub 접근이 거부되었습니다${msg ? `: ${msg}` : ''}`, 'GH_FORBIDDEN', s)
  }
  if (s === 404) return new GitHubApiError('저장소를 찾을 수 없거나 접근 권한이 없습니다(비공개 저장소면 로그인 필요)', 'GH_NOT_FOUND', s)
  const msg = bodyMessage(text)
  return new GitHubApiError(`GitHub API 오류(${s})${msg ? `: ${msg}` : ''}`, 'GH_HTTP', s)
}
