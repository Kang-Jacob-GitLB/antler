// 커밋 작성자 → GitHub 계정(프로필 사진) 해석.
// 1) noreply 이메일은 네트워크 없이 이메일에서 바로 읽는다. 2) 그 밖의 작성자는 그 작성자의 커밋 sha 하나로
// GitHub 가 연결해 둔 계정을 조회한다 — 이메일은 GitHub 로 보내지 않는다. 결과는 메모리 + userData 의 작은 JSON 에 캐시한다.
import { createHash } from 'node:crypto'
import { chmodSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import type { GhAuthorAvatar, GhAvatarQuery } from '@shared/types'
import { avatarBaseUrl, normalizeAvatarUrl, parseNoreply } from '@shared/avatar'
import { AbortError } from '../executors/GitExecutor'
import { GitHubApiError, type GitHubClient } from './client'

export const AVATAR_CACHE_FILE = 'avatar-cache.json'
const DAY_MS = 86_400_000
/** 계정이 확인된 항목 / "GitHub 계정과 연결되지 않음" 항목의 유효 기간 */
const POSITIVE_TTL_MS = 30 * DAY_MS
const NEGATIVE_TTL_MS = 7 * DAY_MS
/** 디스크 캐시 최대 항목 수(넘으면 오래된 것부터 버린다) */
const MAX_ENTRIES = 5000
/** 남은 호출이 이보다 적으면 한도가 풀릴 때까지 해석을 멈춘다(PR·이슈 화면이 쓸 몫을 남긴다) */
const MIN_REMAINING = 10
const CONCURRENCY = 3
/** 앱을 켠 동안 보낼 수 있는 조회 수: 익명은 시간당 60회뿐이라 작게, 로그인하면 넉넉히 */
const SESSION_CAP_ANON = 40
const SESSION_CAP_AUTH = 400
/** 한 번의 요청에서 새로 조회할 작성자 수 */
const PER_CALL_CAP = 60
/** 미푸시 커밋 등 GitHub 가 모르는 sha(404/422) / 네트워크 실패 후 같은 작성자를 다시 묻기까지의 간격 */
const UNKNOWN_SHA_RETRY_MS = 10 * 60_000
const NETWORK_RETRY_MS = 60_000
const SAVE_DELAY_MS = 2000

const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})(?:\[bot\])?$/

interface Entry {
  /** 해석된 계정(null=GitHub 계정과 연결되지 않음) */
  v: GhAuthorAvatar | null
  /** epoch ms */
  t: number
}

interface CommitListItem {
  sha?: unknown
  author?: { login?: unknown; id?: unknown; avatar_url?: unknown } | null
}

const keyOf = (email: string): string => createHash('sha1').update(email.trim().toLowerCase()).digest('hex')

export class AvatarResolver {
  private readonly entries = new Map<string, Entry>()
  private loaded = false
  private saveTimer: NodeJS.Timeout | null = null
  private dirty = false
  /** 메모리에만 두는 "나중에 다시" 표시(key → 다시 물어도 되는 시각) */
  private readonly retryAt = new Map<string, number>()
  private readonly inflight = new Map<string, Promise<void>>()
  private active = 0
  private readonly waiting: (() => void)[] = []
  private pausedUntil = 0
  /** 이번 실행에서 보낸 API 호출 수(테스트·진단용) */
  calls = 0

  constructor(
    private readonly client: GitHubClient,
    private readonly cacheFile?: string,
    private readonly now: () => number = Date.now
  ) {}

  /**
   * authors 를 해석한다. 결과에 없는 키는 "지금은 모름"(한도·네트워크·GitHub 가 모르는 커밋)이라 나중에 다시 물어도 된다.
   * slug 가 null(GitHub 저장소 아님)이면 noreply 이메일과 캐시만 쓴다.
   */
  async resolve(slug: { owner: string; repo: string } | null, authors: GhAvatarQuery[], signal: AbortSignal): Promise<Record<string, GhAuthorAvatar | null>> {
    this.load()
    const out: Record<string, GhAuthorAvatar | null> = {}
    const todo: Promise<void>[] = []
    const waits: Promise<void>[] = []
    const now = this.now()

    for (const a of authors) {
      const n = parseNoreply(a.email)
      if (n) {
        out[a.email] = { login: n.login, url: avatarBaseUrl(n.login, n.id) }
        continue
      }
      const key = keyOf(a.email)
      const hit = this.fresh(key, now)
      if (hit) {
        out[a.email] = hit.v
        continue
      }
      if (!slug || (this.retryAt.get(key) ?? 0) > now) continue
      const pending = this.inflight.get(key)
      if (pending) waits.push(pending)
      else if (todo.length < PER_CALL_CAP) {
        // 동시 요청이 같은 작성자를 중복 조회하지 않도록 시작 즉시 등록한다(실제 호출은 동시성 슬롯을 받은 뒤)
        const base = `/repos/${encodeURIComponent(slug.owner)}/${encodeURIComponent(slug.repo)}/commits`
        const p = this.limited(() => this.lookup(base, a.sha, key, signal), signal).finally(() => this.inflight.delete(key))
        this.inflight.set(key, p)
        todo.push(p)
      }
    }
    await Promise.all([...todo, ...waits])

    // 이번 요청에서 새로 알게 된 것(다른 요청의 조회를 기다린 것 포함)을 모은다
    const after = this.now()
    for (const a of authors) {
      if (a.email in out) continue
      const hit = this.fresh(keyOf(a.email), after)
      if (hit) out[a.email] = hit.v
    }
    return out
  }

  /** 모든 요청이 공유하는 동시성 제한(CONCURRENCY). 슬롯을 받은 뒤 취소·한도를 다시 확인하고 실행한다 */
  private async limited(fn: () => Promise<void>, signal: AbortSignal): Promise<void> {
    if (this.active >= CONCURRENCY) await new Promise<void>((res) => this.waiting.push(res))
    else this.active++
    try {
      if (signal.aborted || !this.canCall()) return
      await fn()
    } finally {
      // 대기 중인 다음 작업에 슬롯을 그대로 넘긴다
      const next = this.waiting.shift()
      if (next) next()
      else this.active--
    }
  }

  /** 호출해도 되는 상태인지: 한도 여유·일시 중지·세션 상한 */
  private canCall(): boolean {
    const now = this.now()
    if (now < this.pausedUntil) return false
    const rate = this.client.rate
    if (rate && rate.remaining < MIN_REMAINING && now < rate.reset * 1000) {
      this.pausedUntil = rate.reset * 1000
      return false
    }
    return this.calls < (this.client.hasToken() ? SESSION_CAP_AUTH : SESSION_CAP_ANON)
  }

  private fresh(key: string, now: number): Entry | null {
    const e = this.entries.get(key)
    if (!e) return null
    if (now - e.t > (e.v ? POSITIVE_TTL_MS : NEGATIVE_TTL_MS)) return null
    return e
  }

  /**
   * sha 한 건 조회: 그 커밋 한 개만 가져오는 목록 호출(GET /commits?sha=<sha>&per_page=1)로 author 를 읽는다.
   * GET /commits/{sha} 는 변경 파일 목록까지 실어 응답이 커질 수 있어 이쪽을 쓴다.
   */
  private async lookup(base: string, sha: string, key: string, signal: AbortSignal): Promise<void> {
    this.calls++
    try {
      const { data } = await this.client.get<CommitListItem[]>(base, { query: { sha, per_page: 1 }, signal })
      const c = Array.isArray(data) ? data[0] : undefined
      if (!c || typeof c.sha !== 'string' || c.sha.toLowerCase() !== sha) {
        this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
        return
      }
      const a = c.author
      if (!a) return this.store(key, null)
      const login = typeof a.login === 'string' && LOGIN_RE.test(a.login) ? a.login : null
      const id = typeof a.id === 'number' && Number.isSafeInteger(a.id) && a.id > 0 ? a.id : undefined
      if (!login) return this.store(key, null)
      this.store(key, { login, url: normalizeAvatarUrl(a.avatar_url, login, id) ?? avatarBaseUrl(login, id) })
    } catch (err) {
      if (err instanceof AbortError) return
      if (err instanceof GitHubApiError) {
        if (err.code === 'GH_RATE_LIMIT') {
          const reset = this.client.rate?.reset
          this.pausedUntil = Math.max(this.now() + NETWORK_RETRY_MS, reset ? reset * 1000 : 0)
        } else if (err.code === 'GH_AUTH') {
          this.pausedUntil = this.now() + NETWORK_RETRY_MS
        } else if (err.code === 'GH_NOT_FOUND' || err.status === 422 || err.status === 409) {
          // 아직 push 하지 않은 커밋 등 — 다른 커밋으로 다시 시도할 수 있도록 기록은 남기지 않는다
          this.retryAt.set(key, this.now() + UNKNOWN_SHA_RETRY_MS)
        } else {
          this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
        }
        return
      }
      this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
    }
  }

  private store(key: string, v: GhAuthorAvatar | null): void {
    this.entries.set(key, { v, t: this.now() })
    this.retryAt.delete(key)
    this.scheduleSave()
  }

  // ── 디스크 캐시 ─────────────────────────────────────

  private load(): void {
    if (this.loaded) return
    this.loaded = true
    if (!this.cacheFile) return
    try {
      const raw = JSON.parse(readFileSync(this.cacheFile, 'utf8')) as { v?: unknown; e?: Record<string, { l?: unknown; i?: unknown; u?: unknown; t?: unknown }> }
      if (raw.v !== 1 || !raw.e || typeof raw.e !== 'object') return
      for (const [key, e] of Object.entries(raw.e)) {
        if (!/^[0-9a-f]{40}$/.test(key) || typeof e.t !== 'number') continue
        if (e.l === null) {
          this.entries.set(key, { v: null, t: e.t })
          continue
        }
        if (typeof e.l !== 'string' || !LOGIN_RE.test(e.l)) continue
        const id = typeof e.i === 'number' && Number.isSafeInteger(e.i) && e.i > 0 ? e.i : undefined
        const url = normalizeAvatarUrl(e.u, e.l, id)
        if (url) this.entries.set(key, { v: { login: e.l, url }, t: e.t })
      }
    } catch {
      // 없거나 깨진 캐시는 비어 있는 것으로 본다
    }
  }

  private scheduleSave(): void {
    if (!this.cacheFile) return
    this.dirty = true
    if (this.saveTimer) return
    this.saveTimer = setTimeout(() => this.flush(), SAVE_DELAY_MS)
    this.saveTimer.unref?.()
  }

  /** 대기 중인 캐시 쓰기를 바로 끝낸다(종료 시 호출) */
  flush(): void {
    if (this.saveTimer) clearTimeout(this.saveTimer)
    this.saveTimer = null
    if (!this.dirty || !this.cacheFile) return
    this.dirty = false
    const now = this.now()
    const live = [...this.entries].filter(([, e]) => now - e.t <= (e.v ? POSITIVE_TTL_MS : NEGATIVE_TTL_MS))
    live.sort((a, b) => b[1].t - a[1].t)
    const e: Record<string, { l: string | null; i?: number; u?: string; t: number }> = {}
    for (const [key, v] of live.slice(0, MAX_ENTRIES)) {
      if (!v.v) e[key] = { l: null, t: v.t }
      else {
        const id = /^\/u\/(\d+)$/.exec(new URL(v.v.url).pathname)
        e[key] = { l: v.v.login, ...(id ? { i: Number(id[1]) } : {}), u: v.v.url, t: v.t }
      }
    }
    try {
      mkdirSync(dirname(this.cacheFile), { recursive: true })
      // 임시 파일에 쓴 뒤 rename — 쓰는 도중 끊겨도 기존 캐시가 깨지지 않는다
      const tmp = `${this.cacheFile}.${process.pid}.tmp`
      writeFileSync(tmp, JSON.stringify({ v: 1, e }), { encoding: 'utf8', mode: 0o600 })
      try {
        chmodSync(tmp, 0o600)
      } catch {
        // Windows 등 chmod 가 의미 없는 환경
      }
      renameSync(tmp, this.cacheFile)
    } catch {
      // 캐시는 부가 기능이라 쓰기 실패는 무시한다
    }
  }
}
