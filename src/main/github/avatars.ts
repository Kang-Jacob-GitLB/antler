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
/** 비로그인에서 성공 없이 404 가 이만큼 쌓이면 비공개 저장소로 본다 */
const ANON_NOT_FOUND_LIMIT = 3

export interface AvatarBlock {
  reason: 'rate-limited' | 'needs-login' | 'session-cap'
  /** epoch seconds (rate-limited) */
  resetAt?: number
  signedIn: boolean
}

/** 고른 커밋이 404(push 안 됨)일 때 같은 작성자의 push 된 커밋으로 더 시도하는 횟수 */
export const FALLBACK_SHAS = 2

/** 404 때 같은 작성자의 다른(push 된) 커밋 sha 를 찾아 주는 함수. 이메일은 GitHub 로 보내지 않고 로컬 git 에서만 쓴다 */
export type ShaFallback = (email: string) => Promise<string[]>

const LOGIN_RE =/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})(?:\[bot\])?$/

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
  /** pausedUntil 이 호출 한도 소진 때문인지(그렇다면 안내에 풀리는 시각을 보인다) */
  private pausedByRate = false
  /** 비로그인 조회 결과를 저장소별로 센다: 404 만 쌓이고 성공이 없으면 비공개 저장소다(메모리에만 — 디스크 캐시에 부정 결과로 남기지 않는다) */
  private readonly anonProbe = new Map<string, { notFound: number; ok: number; checking: boolean; private: boolean }>()
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
  async resolve(
    slug: { owner: string; repo: string } | null,
    authors: GhAvatarQuery[],
    signal: AbortSignal,
    fallback?: ShaFallback
  ): Promise<Record<string, GhAuthorAvatar | null>> {
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
      if (!slug || (this.retryAt.get(key) ?? 0) > now || this.needsLogin(slug)) continue
      const pending = this.inflight.get(key)
      if (pending) waits.push(pending)
      else if (todo.length < PER_CALL_CAP) {
        // 동시 요청이 같은 작성자를 중복 조회하지 않도록 시작 즉시 등록한다(실제 호출은 동시성 슬롯을 받은 뒤)
        const base = `/repos/${encodeURIComponent(slug.owner)}/${encodeURIComponent(slug.repo)}/commits`
        const p = this.limited(() => this.lookupWithFallback(base, a, key, signal, fallback), signal).finally(() =>
          this.inflight.delete(key)
        )
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

  /** 비로그인인데 이 저장소의 조회가 계속 404 — 비공개 저장소로 보고 로그인 전까지 더 묻지 않는다(404 도 한도를 쓴다) */
  private needsLogin(slug: { owner: string; repo: string }): boolean {
    if (this.client.hasToken()) return false
    const p = this.anonProbe.get(`${slug.owner}/${slug.repo}`.toLowerCase())
    return !!p && p.private
  }

  /**
   * 사진 조회가 막혀 있는 이유(막혀 있지 않으면 null). 화면이 안내를 띄우는 데 쓴다.
   * 한도 소진 → 비공개 저장소(로그인 필요) → 이번 실행의 조회 상한 순으로 본다.
   */
  blockedFor(slug: { owner: string; repo: string } | null): AvatarBlock | null {
    const now = this.now()
    const signedIn = this.client.hasToken()
    if (now < this.pausedUntil && this.pausedByRate) return { reason: 'rate-limited', resetAt: Math.ceil(this.pausedUntil / 1000), signedIn }
    const rate = this.client.rate
    if (rate && rate.remaining < MIN_REMAINING && now < rate.reset * 1000) return { reason: 'rate-limited', resetAt: rate.reset, signedIn }
    if (slug && this.needsLogin(slug)) return { reason: 'needs-login', signedIn }
    if (this.calls >= (signedIn ? SESSION_CAP_AUTH : SESSION_CAP_ANON)) return { reason: 'session-cap', signedIn }
    return null
  }

  /** 로그인·로그아웃으로 조건이 달라졌을 때: 메모리의 "나중에/로그인 필요/한도" 표시와 세션 호출 수를 비운다. 확인된 결과(entries)는 남긴다 */
  resetSession(): void {
    this.retryAt.clear()
    this.anonProbe.clear()
    this.pausedUntil = 0
    this.pausedByRate = false
    this.calls = 0
  }

  /** 호출해도 되는 상태인지: 한도 여유·일시 중지·세션 상한 */
  private canCall(): boolean {
    const now = this.now()
    if (now < this.pausedUntil) return false
    const rate = this.client.rate
    if (rate && rate.remaining < MIN_REMAINING && now < rate.reset * 1000) {
      this.pausedUntil = rate.reset * 1000
      this.pausedByRate = true
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
   * 고른 커밋으로 묻고, GitHub 가 모르는 커밋(404 — 로컬 브랜치에만 있는 커밋)이면 같은 작성자의 push 된 커밋으로
   * FALLBACK_SHAS 번까지 다시 묻는다. 비공개 저장소를 비로그인으로 묻는 경우처럼 저장소 자체가 막혔으면 더 묻지 않는다.
   */
  private async lookupWithFallback(base: string, a: GhAvatarQuery, key: string, signal: AbortSignal, fallback?: ShaFallback): Promise<void> {
    const first = a.sha.toLowerCase()
    if ((await this.lookup(base, first, key, signal)) !== 'unknown-sha' || !fallback) return
    const sk = AvatarResolver.slugKeyOfBase(base)
    let candidates: string[]
    try {
      candidates = (await fallback(a.email)).filter((s) => s !== first).slice(0, FALLBACK_SHAS)
    } catch {
      return
    }
    for (const sha of candidates) {
      if (signal.aborted || !this.canCall()) return
      if (sk && !this.client.hasToken() && this.anonProbe.get(sk)?.private) return
      if ((await this.lookup(base, sha, key, signal)) !== 'unknown-sha') return
    }
  }

  /**
   * sha 한 건 조회: 그 커밋 한 개만 가져오는 목록 호출(GET /commits?sha=<sha>&per_page=1)로 author 를 읽는다.
   * GET /commits/{sha} 는 변경 파일 목록까지 실어 응답이 커질 수 있어 이쪽을 쓴다.
   */
  /** 결과: 'resolved' 기록함(계정 있음/없음) · 'unknown-sha' GitHub 가 모르는 커밋(다른 커밋으로 더 물어볼 만함) · 'other' 그 밖 */
  private async lookup(base: string, sha: string, key: string, signal: AbortSignal): Promise<'resolved' | 'unknown-sha' | 'other'> {
    // 같은 묶음에서 먼저 간 조회들이 이 저장소를 비공개로 판정했다면, 이미 대기열에 들어온 나머지도 보내지 않는다
    const sk = AvatarResolver.slugKeyOfBase(base)
    if (sk && !this.client.hasToken() && this.anonProbe.get(sk)?.private) {
      this.retryAt.set(key, this.now() + UNKNOWN_SHA_RETRY_MS)
      return 'other'
    }
    this.calls++
    try {
      const { data } = await this.client.get<CommitListItem[]>(base, { query: { sha, per_page: 1 }, signal })
      const c = Array.isArray(data) ? data[0] : undefined
      if (!c || typeof c.sha !== 'string' || c.sha.toLowerCase() !== sha) {
        this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
        return 'other'
      }
      void this.probe(base, 'ok', signal)
      const a = c.author
      if (!a) {
        this.store(key, null)
        return 'resolved'
      }
      const login = typeof a.login === 'string' && LOGIN_RE.test(a.login) ? a.login : null
      const id = typeof a.id === 'number' && Number.isSafeInteger(a.id) && a.id > 0 ? a.id : undefined
      this.store(key, login ? { login, url: normalizeAvatarUrl(a.avatar_url, login, id) ?? avatarBaseUrl(login, id) } : null)
      return 'resolved'
    } catch (err) {
      if (err instanceof AbortError) return 'other'
      if (err instanceof GitHubApiError) {
        if (err.code === 'GH_RATE_LIMIT') {
          const reset = this.client.rate?.reset
          this.pausedUntil = Math.max(this.now() + NETWORK_RETRY_MS, reset ? reset * 1000 : 0)
          this.pausedByRate = true
        } else if (err.code === 'GH_AUTH') {
          this.pausedUntil = this.now() + NETWORK_RETRY_MS
          this.pausedByRate = false
        } else if (err.code === 'GH_NOT_FOUND' || err.status === 422 || err.status === 409) {
          // 아직 push 하지 않은 커밋 등 — 다른 커밋으로 다시 시도할 수 있도록 기록은 남기지 않는다
          this.retryAt.set(key, this.now() + UNKNOWN_SHA_RETRY_MS)
          if (err.code === 'GH_NOT_FOUND') await this.probe(base, 'notFound', signal)
          return 'unknown-sha'
        } else {
          this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
        }
        return 'other'
      }
      this.retryAt.set(key, this.now() + NETWORK_RETRY_MS)
      return 'other'
    }
  }

  /** base(/repos/o/r/commits)로 저장소를 식별해 비로그인 조회 결과를 센다 */
  /** base(/repos/o/r/commits) → 'o/r'(소문자) */
  private static slugKeyOfBase(base: string): string | null {
    const m = /^\/repos\/([^/]+)\/([^/]+)\/commits$/.exec(base)
    return m ? `${decodeURIComponent(m[1]!)}/${decodeURIComponent(m[2]!)}`.toLowerCase() : null
  }

  private async probe(base: string, kind: 'ok' | 'notFound', signal: AbortSignal): Promise<void> {
    if (this.client.hasToken()) return
    const k = AvatarResolver.slugKeyOfBase(base)
    if (!k) return
    const p = this.anonProbe.get(k) ?? { notFound: 0, ok: 0, checking: false, private: false }
    this.anonProbe.set(k, p)
    p[kind]++
    // 404 만 쌓였다고 비공개로 단정하지 않는다(공개 저장소의 미푸시 커밋도 404). 저장소 자체를 한 번 확인해 404 면 비공개다
    if (kind === 'notFound' && p.ok === 0 && p.notFound >= ANON_NOT_FOUND_LIMIT && !p.checking && !p.private) {
      p.checking = true
      this.calls++
      try {
        await this.client.get(base.slice(0, -'/commits'.length), { signal })
        p.ok++
      } catch (err) {
        if (err instanceof GitHubApiError && err.code === 'GH_NOT_FOUND') p.private = true
        else p.checking = false
      }
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
