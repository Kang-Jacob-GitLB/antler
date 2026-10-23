// Jira REST 클라이언트(GET · 상태 전환용 POST). 자격 증명은 credProvider 로만 읽고 요청 헤더에만 싣는다(반환값·에러 메시지·로그에 넣지 않는다).
// 리다이렉트는 따라가지 않는다 — 다른 호스트(또는 같은 호스트의 http)로 인증 헤더가 새어 나가는 일을 원천 차단한다.
import { AbortError } from '../executors/GitExecutor'

const REQUEST_TIMEOUT_MS = 15_000
const MAX_BODY_BYTES = 5 * 1024 * 1024
/** 응답이 상한을 넘었을 때의 메시지(호출자가 '이 요청만 건너뛰기'를 가려내는 데도 쓴다) */
export const JIRA_TOO_BIG = 'Jira 응답이 너무 큽니다'
/** 429 의 Retry-After 가 이 이하면 한 번만 기다렸다 다시 시도한다 */
const AUTO_RETRY_MAX_SEC = 3

export type JiraErrorCode = 'JIRA_AUTH' | 'JIRA_NOT_FOUND' | 'JIRA_FORBIDDEN' | 'JIRA_RATE_LIMIT' | 'JIRA_HTTP' | 'JIRA_NETWORK' | 'JIRA_TIMEOUT' | 'JIRA_REDIRECT'

/** Jira API 실패. 메시지는 그대로 사용자에게 보여 줘도 되는 한국어 문장이다 */
export class JiraApiError extends Error {
  constructor(
    message: string,
    readonly code: JiraErrorCode,
    readonly status?: number
  ) {
    super(message)
    this.name = 'JiraApiError'
  }
}

export interface JiraCredential {
  kind: 'cloud' | 'server'
  /** 정규화된 사이트 주소(경로 포함, 끝 '/' 없음) */
  siteUrl: string
  email: string
  token: string
}

export type CredentialProvider = () => JiraCredential | null
export type SleepFn = (ms: number, signal?: AbortSignal) => Promise<void>

export interface GetOptions {
  query?: Record<string, string | number | undefined>
  /** 404 일 때 보여 줄 문장(엔드포인트별: 이슈 / 검색 / 연결 확인) */
  notFound?: string
  signal?: AbortSignal
  /** 저장된 자격 증명 대신 이 값으로 요청한다(연결 전 검증용) */
  credential?: JiraCredential
  /** 응답 본문 상한(기본 5MB). 큰 응답이 정상인 엔드포인트만 올린다 */
  maxBytes?: number
}

export const defaultSleep: SleepFn = (ms, signal) =>
  new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new AbortError())
    const t = setTimeout(resolve, ms)
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(t)
        reject(new AbortError())
      },
      { once: true }
    )
  })

/** 프로필 사진 응답 상한과 받는 형식(SVG 는 받지 않는다) */
const IMAGE_MAX_BYTES = 256 * 1024
const IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp'])
/** 사이트 밖에서 받아도 되는 사진 호스트: Cloud avatarUrls 의 gravatar·Atlassian 사진 저장소, gravatar 가 기본 사진을 넘겨주는 wp.com 프록시 */
const PUBLIC_AVATAR_HOST_RE = /^(?:secure\.gravatar\.com|i[0-3]\.wp\.com|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.atl-paas\.net)$/
const IMAGE_REDIRECTS = 3

export interface JiraImage {
  /** IMAGE_TYPES 중 하나 */
  type: string
  bytes: Buffer
}

/** 주소가 연결한 사이트 안(같은 origin, 사이트 경로 아래)인가 */
function inSite(url: URL, siteUrl: string): boolean {
  const site = new URL(siteUrl)
  const base = site.pathname.replace(/\/+$/, '')
  return url.origin === site.origin && (base === '' || url.pathname === base || url.pathname.startsWith(`${base}/`))
}

/** Authorization 헤더 값. Cloud 는 이메일:API 토큰 Basic, Data Center 는 PAT Bearer */
export function authHeader(c: JiraCredential): string {
  return c.kind === 'cloud' ? `Basic ${Buffer.from(`${c.email}:${c.token}`, 'utf8').toString('base64')}` : `Bearer ${c.token}`
}

export class JiraClient {
  constructor(
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly credProvider: CredentialProvider = () => null,
    private readonly sleep: SleepFn = defaultSleep
  ) {}

  /** REST GET. path 는 '/rest/api/…' 처럼 '/' 로 시작하는 사이트 상대 경로(세그먼트는 호출자가 인코딩한다) */
  async get<T>(path: string, opts: GetOptions = {}): Promise<T> {
    const { cred, url } = this.target(path, opts)

    for (let attempt = 0; ; attempt++) {
      const { res, timeout } = await this.request('GET', url.href, cred, opts.signal)
      if (res.status === 429 && attempt === 0) {
        const wait = parseRetryAfter(res.headers.get('retry-after'))
        if (wait !== null && wait <= AUTO_RETRY_MAX_SEC) {
          // 본문은 필요 없지만 상한을 지켜 비운다(연결 재사용·메모리). 취소만 전파한다
          await readText(res, opts.signal, timeout, opts.maxBytes ?? MAX_BODY_BYTES).catch((e: unknown) => {
            if (e instanceof AbortError) throw e
          })
          await this.sleep(wait * 1000, opts.signal)
          continue
        }
      }
      const text = await readText(res, opts.signal, timeout, opts.maxBytes ?? MAX_BODY_BYTES)
      if (!res.ok) throw httpError(res, text, cred.kind, opts.notFound)
      try {
        return JSON.parse(text) as T
      } catch {
        throw new JiraApiError('Jira 응답을 해석할 수 없습니다', 'JIRA_HTTP', res.status)
      }
    }
  }

  /**
   * REST POST(JSON 본문). 쓰기이므로 429 자동 재시도를 하지 않는다 — 요청이 이미 적용됐을 수 있어 중복 실행을 피한다.
   * 2xx 의 빈 본문(204)은 undefined. X-Atlassian-Token 은 Data Center 의 XSRF 검사를 통과시킨다
   */
  async post<T = undefined>(path: string, body: unknown, opts: GetOptions = {}): Promise<T> {
    const { cred, url } = this.target(path, opts)
    const { res, timeout } = await this.request('POST', url.href, cred, opts.signal, JSON.stringify(body))
    const text = await readText(res, opts.signal, timeout, opts.maxBytes ?? MAX_BODY_BYTES)
    if (!res.ok) throw httpError(res, text, cred.kind, opts.notFound)
    if (!text.trim()) return undefined as T
    try {
      return JSON.parse(text) as T
    } catch {
      throw new JiraApiError('Jira 응답을 해석할 수 없습니다', 'JIRA_HTTP', res.status)
    }
  }

  /**
   * 프로필 사진(avatarUrls) 한 장. 연결한 사이트 안의 주소면 인증을 붙이고 리다이렉트를 따라가지 않는다(Data Center 는 인증해야 사진을 준다).
   * 사이트 밖은 Cloud 의 공개 사진 호스트만 https·인증 없이 받고, 그 호스트들 사이의 리다이렉트만 따라간다(gravatar 의 기본 사진).
   * 자격 증명은 처음 요청한 주소가 사이트 안일 때만 실린다. 받을 수 없거나 허용한 이미지 형식이 아니면 null — 취소만 AbortError 로 던진다
   */
  async image(href: string, signal?: AbortSignal): Promise<JiraImage | null> {
    const cred = this.credProvider()
    if (!cred) return null
    let url: URL
    try {
      url = new URL(href)
    } catch {
      return null
    }
    for (let hop = 0; hop <= IMAGE_REDIRECTS; hop++) {
      if (url.username || url.password) return null
      // 리다이렉트로 온 주소에는 인증을 붙이지 않는다(사이트 안이라도)
      const own = hop === 0 && inSite(url, cred.siteUrl)
      if (!own && !(url.protocol === 'https:' && url.port === '' && PUBLIC_AVATAR_HOST_RE.test(url.hostname))) return null
      const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS)
      const sig = signal ? AbortSignal.any([timeout, signal]) : timeout
      let res: Response
      try {
        res = await this.fetchImpl(url.href, {
          method: 'GET',
          headers: { Accept: 'image/*', 'User-Agent': 'Antler', ...(own ? { Authorization: authHeader(cred) } : {}) },
          redirect: 'manual',
          signal: sig
        })
      } catch {
        if (signal?.aborted) throw new AbortError()
        return null
      }
      if (res.status >= 300 && res.status < 400) {
        await res.body?.cancel().catch(() => undefined)
        // 사이트 안의 리다이렉트는 대개 로그인 화면이다 — 따라가지 않는다
        const loc = res.headers.get('location')
        if (own || !loc) return null
        try {
          url = new URL(loc, url)
        } catch {
          return null
        }
        continue
      }
      const type = (res.headers.get('content-type') ?? '').split(';')[0]!.trim().toLowerCase()
      if (!res.ok || !IMAGE_TYPES.has(type)) {
        await res.body?.cancel().catch(() => undefined)
        return null
      }
      try {
        const bytes = await readBytes(res, signal, timeout, IMAGE_MAX_BYTES)
        return bytes.length > 0 ? { type, bytes } : null
      } catch (err) {
        if (err instanceof AbortError) throw err
        return null
      }
    }
    return null
  }

  /** 경로 검증(사이트 상대 경로, 같은 origin)과 자격 증명·쿼리를 합친 요청 대상 */
  private target(path: string, opts: GetOptions): { cred: JiraCredential; url: URL } {
    if (!path.startsWith('/') || path.startsWith('//')) throw new Error('invalid api path')
    const cred = opts.credential ?? this.credProvider()
    if (!cred) throw new JiraApiError('Jira 에 연결되어 있지 않습니다', 'JIRA_AUTH')
    const site = new URL(cred.siteUrl)
    const url = new URL(cred.siteUrl + path)
    if (url.origin !== site.origin) throw new Error('invalid api path')
    for (const [k, v] of Object.entries(opts.query ?? {})) if (v !== undefined) url.searchParams.set(k, String(v))
    return { cred, url }
  }

  private async request(
    method: 'GET' | 'POST',
    href: string,
    cred: JiraCredential,
    signal: AbortSignal | undefined,
    body?: string
  ): Promise<{ res: Response; timeout: AbortSignal }> {
    const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    const sig = signal ? AbortSignal.any([timeout, signal]) : timeout
    let res: Response
    try {
      res = await this.fetchImpl(href, {
        method,
        headers: {
          Accept: 'application/json',
          Authorization: authHeader(cred),
          'User-Agent': 'Antler',
          ...(body !== undefined ? { 'Content-Type': 'application/json', 'X-Atlassian-Token': 'no-check' } : {})
        },
        ...(body !== undefined ? { body } : {}),
        redirect: 'manual',
        signal: sig
      })
    } catch (err) {
      if (signal?.aborted) throw new AbortError()
      if (timeout.aborted) throw new JiraApiError(`Jira 응답이 ${REQUEST_TIMEOUT_MS / 1000}초 안에 오지 않았습니다`, 'JIRA_TIMEOUT')
      // fetch 에러 객체에는 요청 헤더가 없지만, 원인 문자열만 남기고 나머지는 버린다
      const cause = err instanceof Error && err.cause instanceof Error ? err.cause.message : err instanceof Error ? err.message : ''
      throw new JiraApiError(`Jira 에 연결할 수 없습니다${cause ? `(${cause})` : ''}`, 'JIRA_NETWORK')
    }
    if (res.status >= 300 && res.status < 400) {
      await res.body?.cancel().catch(() => undefined)
      throw new JiraApiError('Jira 가 다른 주소로 이동시키려 해 요청을 중단했습니다(사이트 주소를 확인하세요)', 'JIRA_REDIRECT', res.status)
    }
    return { res, timeout }
  }
}

async function readText(res: Response, signal: AbortSignal | undefined, timeout: AbortSignal, limit: number): Promise<string> {
  return (await readBytes(res, signal, timeout, limit)).toString('utf8')
}

/** 본문을 읽으며 바이트를 센다. 상한을 넘으면 읽기를 중단(reader.cancel)하고 오류 — chunked 거대 응답도 메모리에 다 받지 않는다 */
async function readBytes(res: Response, signal: AbortSignal | undefined, timeout: AbortSignal, limit: number): Promise<Buffer> {
  const tooBig = (): JiraApiError => new JiraApiError(JIRA_TOO_BIG, 'JIRA_HTTP', res.status)
  const len = Number(res.headers.get('content-length'))
  if (Number.isFinite(len) && len > limit) {
    await res.body?.cancel().catch(() => undefined)
    throw tooBig()
  }
  if (!res.body) return Buffer.alloc(0)
  const reader = res.body.getReader()
  const chunks: Uint8Array[] = []
  let total = 0
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      total += value.byteLength
      if (total > limit) {
        await reader.cancel().catch(() => undefined)
        throw tooBig()
      }
      chunks.push(value)
    }
  } catch (err) {
    if (err instanceof JiraApiError) throw err
    if (signal?.aborted) throw new AbortError()
    if (timeout.aborted) throw new JiraApiError(`Jira 응답이 ${REQUEST_TIMEOUT_MS / 1000}초 안에 오지 않았습니다`, 'JIRA_TIMEOUT')
    throw new JiraApiError('Jira 응답을 읽는 중 연결이 끊겼습니다', 'JIRA_NETWORK', res.status)
  }
  return Buffer.concat(chunks)
}

/** Retry-After: 초 또는 HTTP-date → 남은 초(올림). 해석할 수 없으면 null */
export function parseRetryAfter(v: string | null, now = Date.now()): number | null {
  if (!v) return null
  const sec = Number(v)
  if (Number.isFinite(sec) && sec >= 0) return Math.ceil(sec)
  const at = Date.parse(v)
  return Number.isFinite(at) ? Math.max(0, Math.ceil((at - now) / 1000)) : null
}

/** Jira 오류 본문의 errorMessages[0] 또는 message, 없으면 필드별 오류(errors)의 값들(있다면). 길이는 제한한다 */
function bodyMessage(text: string): string {
  try {
    const b = JSON.parse(text) as { errorMessages?: unknown; errors?: unknown; message?: unknown }
    let m: unknown = Array.isArray(b.errorMessages) && typeof b.errorMessages[0] === 'string' ? b.errorMessages[0] : b.message
    if (typeof m !== 'string' || !m) {
      const fields = b.errors && typeof b.errors === 'object' ? Object.values(b.errors).filter((v): v is string => typeof v === 'string') : []
      if (fields.length) m = fields.join(' / ')
    }
    // 공백 정리가 개행·탭 등 제어문자도 없앤다
    return typeof m === 'string' ? m.replace(/[\s\u0000-\u001f\u007f]+/g, ' ').trim().slice(0, 200) : ''
  } catch {
    return ''
  }
}

function httpError(res: Response, text: string, kind: JiraCredential['kind'], notFound?: string): JiraApiError {
  const s = res.status
  if (s === 401) {
    const how = kind === 'cloud' ? '이메일·API 토큰을 확인하고' : 'Personal Access Token 을 확인하고'
    return new JiraApiError(`Jira 인증이 거부되었습니다. ${how} 다시 연결하세요`, 'JIRA_AUTH', s)
  }
  if (s === 403) {
    const msg = bodyMessage(text)
    return new JiraApiError(`Jira 접근이 거부되었습니다${msg ? `: ${msg}` : ''}`, 'JIRA_FORBIDDEN', s)
  }
  if (s === 404) return new JiraApiError(notFound ?? '요청한 항목을 찾을 수 없거나 볼 권한이 없습니다', 'JIRA_NOT_FOUND', s)
  if (s === 429) {
    const wait = parseRetryAfter(res.headers.get('retry-after'))
    return new JiraApiError(wait !== null && wait > 0 ? `Jira 요청이 너무 잦습니다. ${wait}초 후에 다시 시도하세요` : 'Jira 요청이 너무 잦습니다. 잠시 후 다시 시도하세요', 'JIRA_RATE_LIMIT', s)
  }
  const msg = bodyMessage(text)
  return new JiraApiError(`Jira API 오류(${s})${msg ? `: ${msg}` : ''}`, 'JIRA_HTTP', s)
}
