import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { domainToUnicode } from 'node:url'
import { describe, expect, it } from 'vitest'
import { DeviceFlow } from '../src/main/github/deviceFlow'
import { ghEnv } from '../src/main/github/ghCli'
import { GitHubClient, bodyMessage as ghBodyMessage, hasNextLink, httpError as ghHttpError, parseRate } from '../src/main/github/client'
import {
  allowedExternalUrl,
  parseAvatarAuthors,
  parseCheckShas,
  parseId,
  parseListQuery,
  parseNumber,
  parsePage,
  parsePat,
  parseRunId,
  parseSha
} from '../src/main/github/validate'
import { redactToken, usableToken } from '../src/main/git/githubCache'
import { JiraClient, PUBLIC_AVATAR_HOST_RE, authHeader, bodyMessage as jiraBodyMessage, httpError as jiraHttpError, inSite, parseRetryAfter } from '../src/main/jira/client'
import {
  MAX_JQL_LEN,
  MAX_PAGES,
  MAX_WORKLOG_DAYS,
  MAX_WORKLOG_USERS,
  PAGE_SIZE,
  allowedJiraUrl,
  jqlQuote,
  parseConnectInput,
  parseIssueKey,
  parseSearchQuery,
  parseSiteUrl,
  parseTransitionId,
  parseUserSearch,
  parseWorklogQuery
} from '../src/main/jira/validate'
import { itKeys } from './fixtures/itTitles'
import {
  AUTH_CASES,
  AVATAR_AUTHORS,
  AVATAR_HOSTS,
  BODY_TEXTS,
  CHECK_SHAS,
  CONNECT_INPUTS,
  DEVICE_POLLS,
  DEVICE_STARTS,
  ENV_CASES,
  EXTERNAL_URLS,
  GH_REQUESTS,
  IDNA_SWEEP_RANGES,
  IDS,
  IMAGE_CASES,
  ISSUE_KEYS,
  JIRA_REQUESTS,
  JQL_VALUES,
  JSON_EXTRA,
  LINK_HEADERS,
  LIST_QUERIES,
  NEST_CASES,
  NOT_FOUNDS,
  NOW_MS,
  NUMBERS,
  PAGES,
  PATS,
  RATE_LIMIT,
  RATE_REMAINING,
  RATE_RESET,
  RATE_RETRY,
  REDACT_CASES,
  RETRY_AFTER_VALUES,
  RUN_IDS,
  SEARCH_QUERIES,
  SHAS,
  SITE_A,
  SITE_B,
  SITE_L,
  SITE_RAWS,
  STATUSES_GH,
  STATUSES_JIRA,
  TRANSITION_IDS,
  USABLE_TOKENS,
  USER_SEARCHES,
  WORKLOG_QUERIES,
  nestText,
  urlGrid,
  type DevicePollCase,
  type DeviceStartCase
} from './fixtures/netCases'

// GitHub·Jira 순수 로직(검증·URL 판정·응답 해석·Device Flow·ghEnv·토큰 가리기)을 골든 JSON 으로 고정한다.
// Rust 이식(src-tauri/src/{net,github,jira}, 검사: src-tauri/tests/net.rs)이 같은 골든을 읽어 같은 결과를 내는지 본다.
// 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`. 입력 표는 tests/fixtures/netCases.ts.
// 이 테스트는 네트워크를 쓰지 않는다(가짜 fetch). 시간대에 따라 달라지는 값(로컬 시간으로 해석되는 날짜·ko-KR 시각 표시)은 골든에 넣지 않는다.

const GOLDEN_DIR = join(__dirname, 'golden', 'net')
/** 대상 테스트 파일의 각 it 이 Rust 의 어느 테스트로 옮겨졌는지(또는 왜 N/A 인지) 적은 손글씨 표. 골든이 아니다 */
const TEST_MAP = 'test-map.json'
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 이식 대응표가 덮는 TS 테스트 파일(Phase 4 의 대상 15개). 각 PR 이 자기 몫의 `N/A: PR n` 을 Rust 테스트로 바꾼다 */
const TARGET_TESTS = [
  'avatars',
  'ghAccessibleRepos',
  'ghApi',
  'ghChecks',
  'ghClient',
  'ghCliImport',
  'ghDevice',
  'ghService',
  'jiraAdf',
  'jiraAvatar',
  'jiraClient',
  'jiraHardening',
  'jiraService',
  'launchpadQuery',
  'launchpadService'
]

// ── 직렬화 ────────────────────────────────────────────────────────────────

/** 최상위 키마다 한 줄(배열은 항목마다 한 줄)로 쓴다 */
function render(raw: Record<string, unknown>): string {
  // 짝 없는 서로게이트가 든 출력(오류 문구·토큰)은 U+FFFD 로 바꿔 적는다 — serde_json 은 짝 없는 서로게이트 이스케이프를 거부한다
  const file = wellFormed(raw)
  const entries = Object.entries(file).map(([k, v]) => {
    let body: string
    if (Array.isArray(v) && v.length > 0 && typeof v[0] === 'object' && v[0] !== null) {
      body = `[\n${v.map((x) => `  ${JSON.stringify(x)}`).join(',\n')}\n ]`
    } else if (v && typeof v === 'object' && !Array.isArray(v)) {
      const inner = Object.entries(v).map(([ik, iv]) => `  ${JSON.stringify(ik)}: ${JSON.stringify(iv)}`)
      body = `{\n${inner.join(',\n')}\n }`
    } else {
      body = JSON.stringify(v)
    }
    return ` ${JSON.stringify(k)}: ${body}`
  })
  return `{\n${entries.join(',\n')}\n}\n`
}

/** 짝 없는 서로게이트를 U+FFFD 로(String.prototype.toWellFormed 와 같다 — Rust 문자열은 짝 없는 서로게이트를 담지 못한다) */
function toWellFormed(s: string): string {
  return s.replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '�')
}

function wellFormed<T>(v: T): T {
  if (typeof v === 'string') return toWellFormed(v) as T
  if (Array.isArray(v)) return v.map(wellFormed) as T
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [toWellFormed(k), wellFormed(x)])) as T
  return v
}

/** JSON 으로 담을 수 없는 숫자(NaN·무한대·-0)는 문자열로 */
function enc(n: number): number | string {
  if (Number.isNaN(n)) return 'NaN'
  if (n === Infinity) return 'Infinity'
  if (n === -Infinity) return '-Infinity'
  if (Object.is(n, -0)) return '-0'
  return n
}

/** JSON 왕복(IPC 를 거친 값과 같게): undefined 필드는 사라진다 */
function rt<T>(v: T): T {
  return (v === undefined ? null : JSON.parse(JSON.stringify(v))) as T
}

type Out = { ok: unknown } | { err: string }
function run(fn: () => unknown): Out {
  try {
    return { ok: rt(fn()) }
  } catch (e) {
    return { err: (e as Error).message }
  }
}

/** 시간대(UTC·Asia/Seoul)마다 결과가 다르면 'tz' — 골든이 머신의 시간대에 달라지지 않게 한다 */
function tzStable<T>(fn: () => T): T | 'tz' {
  const saved = process.env.TZ
  try {
    process.env.TZ = 'UTC'
    const a = JSON.stringify(fn())
    process.env.TZ = 'Asia/Seoul'
    const b = JSON.stringify(fn())
    return a === b ? (JSON.parse(a) as T) : 'tz'
  } finally {
    if (saved === undefined) delete process.env.TZ
    else process.env.TZ = saved
  }
}

/** 요청 헤더로 보낼 수 있고 Headers 가 값을 바꾸지 않는 문자열인가 */
function headerSafe(v: string | null): boolean {
  if (v === null) return true
  try {
    return new Headers({ a: v }).get('a') === v
  } catch {
    return false
  }
}
const hdr = (o: Record<string, string | null | undefined>): Headers => {
  const h = new Headers()
  for (const [k, v] of Object.entries(o)) if (v !== null && v !== undefined) h.set(k, v)
  return h
}

// ── 손으로 옮긴 정규식·분기(Rust 상수와 같아야 한다) ──────────────────────

const REPO_ROOT = join(__dirname, '..')
/** [이름, TS 파일, 소스에 그대로 있어야 하는 조각] */
const HAND_WRITTEN: [string, string, string][] = [
  ['PUBLIC_AVATAR_HOST_RE', 'src/main/jira/client.ts', String.raw`/^(?:secure\.gravatar\.com|i[0-3]\.wp\.com|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.atl-paas\.net)$/`],
  ['IN_SITE_ENCODED', 'src/main/jira/client.ts', String.raw`/%(?:2f|5c|2e)/i`],
  ['LINK_NEXT', 'src/main/github/client.ts', String.raw`/<[^>]*>\s*;\s*rel="?next"?/i`],
  ['CONTROL_RE', 'src/main/github/validate.ts', String.raw`/[\u0000-\u001f\u007f]/`],
  ['CONTROL_RE(jira)', 'src/main/jira/validate.ts', String.raw`/[\u0000-\u001f\u007f]/`],
  ['PAT_CHARS', 'src/main/github/validate.ts', String.raw`/^[\x21-\x7e]+$/`],
  ['TOKEN_CHARS', 'src/main/jira/validate.ts', String.raw`/^[\x21-\x7e]+$/`],
  ['SHA_RE', 'src/main/github/validate.ts', String.raw`/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i`],
  ['KEY_RE', 'src/main/jira/validate.ts', String.raw`/^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$/`],
  ['CURSOR_RE', 'src/main/jira/validate.ts', String.raw`/^[!-~]{1,2000}$/`],
  ['TRANSITION_ID', 'src/main/jira/validate.ts', String.raw`/^\d{1,10}$/`],
  ['USER_ID_RE', 'src/main/jira/validate.ts', String.raw`/^[A-Za-z0-9._@:+-]{1,128}$/`],
  ['DATE_RE', 'src/main/jira/validate.ts', String.raw`/^(\d{4})-(\d{2})-(\d{2})$/`],
  ['JIRA_BODY_SPACES', 'src/main/jira/client.ts', String.raw`/[\s\u0000-\u001f\u007f]+/g`],
  ['USABLE_TOKEN', 'src/main/git/githubCache.ts', String.raw`/[\s\0]/`],
  ['TRAILING_SLASH', 'src/main/jira/validate.ts', String.raw`/\/+$/`]
]

// ── 골든 만들기 ───────────────────────────────────────────────────────────

/** 기준 TS 구현이 만든 검증 결과 */
function buildValidate(): Record<string, unknown> {
  const L = [false, true]
  const cases = <T>(inputs: unknown[], fn: (x: never) => T): { in: unknown; out: Out }[] =>
    inputs.map((i) => {
      const x = rt(i)
      return { in: x, out: run(() => fn(x as never)) }
    })
  const jiraUrlRaws = EXTERNAL_URLS.filter((x) => typeof x === 'string')
  return {
    consts: { PAGE_SIZE, MAX_PAGES, MAX_JQL_LEN, MAX_WORKLOG_USERS, MAX_WORKLOG_DAYS },
    ghParseNumber: cases(NUMBERS, parseNumber),
    ghParseId: cases(IDS, (x) => parseId(x, '저장소')),
    ghParseListQuery: cases(LIST_QUERIES, parseListQuery),
    ghParsePat: cases(PATS, parsePat),
    ghParseAvatarAuthors: cases(AVATAR_AUTHORS, parseAvatarAuthors),
    ghParseRunId: cases(RUN_IDS, parseRunId),
    ghParseSha: cases(SHAS, parseSha),
    ghParseCheckShas: cases(CHECK_SHAS, parseCheckShas),
    ghParsePage: cases(PAGES, parsePage),
    ghAllowedExternalUrl: cases(EXTERNAL_URLS, allowedExternalUrl),
    jiraParseSiteUrl: L.flatMap((loop) =>
      SITE_RAWS.map((raw) => ({ in: rt(raw), loop, out: run(() => parseSiteUrl(rt(raw), { allowInsecureLoopback: loop })) }))
    ),
    jiraParseConnectInput: L.flatMap((loop) => CONNECT_INPUTS.map((i) => ({ in: rt(i), loop, out: run(() => parseConnectInput(rt(i), { allowInsecureLoopback: loop })) }))),
    jiraParseIssueKey: cases(ISSUE_KEYS, parseIssueKey),
    jiraParseTransitionId: cases(TRANSITION_IDS, parseTransitionId),
    jiraParseSearchQuery: cases(SEARCH_QUERIES, parseSearchQuery),
    jiraParseWorklogQuery: cases(WORKLOG_QUERIES, parseWorklogQuery),
    jiraJqlQuote: JQL_VALUES.map((v) => ({ in: v, out: run(() => jqlQuote(v)) })),
    jiraParseUserSearch: cases(USER_SEARCHES, parseUserSearch),
    jiraAllowedJiraUrl: L.flatMap((loop) =>
      [SITE_B, SITE_A, SITE_L, null, ''].flatMap((site) =>
        [...jiraUrlRaws, ...SITE_RAWS].map((raw) => ({
          in: rt(raw),
          site,
          loop,
          out: rt(allowedJiraUrl(rt(raw), site === null ? undefined : site, { allowInsecureLoopback: loop }))
        }))
      )
    )
  }
}

/** Rust 가 받는 IDN 문자 범위(IDNA_SAFE_RANGES)의 모든 코드 포인트를 `new URL` 에 넣어 받는 것과 풀린 호스트를 적는다 */
function buildIdna(): Record<string, unknown> {
  const accepted: [number, number][] = []
  const mapped: [number, string][] = []
  for (const [lo, hi] of IDNA_SWEEP_RANGES) {
    for (let cp = lo; cp <= hi; cp++) {
      const ch = String.fromCodePoint(cp)
      let host: string | null = null
      try {
        host = domainToUnicode(new URL(`https://a${ch}b.example.com/`).hostname)
      } catch {
        host = null
      }
      if (host === null) continue
      const last = accepted[accepted.length - 1]
      if (last && last[1] === cp - 1) last[1] = cp
      else accepted.push([cp, cp])
      if (host !== `a${ch}b.example.com`) mapped.push([cp, host])
    }
  }
  return { ranges: IDNA_SWEEP_RANGES, accepted, mapped }
}

function buildUrls(): Record<string, unknown> {
  const attempt = (f: () => string): string | { e: string } => {
    try {
      return f()
    } catch (e) {
      return { e: (e as Error).message }
    }
  }
  const ins = (raw: string, site: string): boolean | 'throw' => {
    try {
      return inSite(new URL(raw), site)
    } catch {
      return 'throw'
    }
  }
  const href = (raw: string): string | null => {
    try {
      return new URL(raw).href
    } catch {
      return null
    }
  }
  const grid = urlGrid().map((r) => ({
    r,
    h: href(r),
    s: attempt(() => parseSiteUrl(r)),
    l: attempt(() => parseSiteUrl(r, { allowInsecureLoopback: true })),
    x: allowedExternalUrl(r),
    a: ins(r, SITE_A),
    b: ins(r, SITE_B),
    j: allowedJiraUrl(r, SITE_B),
    k: allowedJiraUrl(r, SITE_L, { allowInsecureLoopback: true })
  }))

  const captured = async (client: { get: (p: string, o: { query: Record<string, string | number | undefined> }) => Promise<unknown> }, path: string, q: [string, string | number | null][]): Promise<boolean> => {
    const query: Record<string, string | number | undefined> = {}
    for (const [k, v] of q) query[k] = v === null ? undefined : v
    try {
      await client.get(path, { query })
      return false
    } catch {
      return true
    }
  }
  const requests = async (): Promise<unknown[]> => {
    const out: unknown[] = []
    for (const [path, q] of GH_REQUESTS) {
      let url: string | null = null
      const client = new GitHubClient((async (u: string) => {
        url = u
        return new Response('{}', { status: 200 })
      }) as unknown as typeof fetch)
      const threw = await captured(client as never, path, q)
      out.push({ path, q, url: threw ? null : url })
    }
    return out
  }
  const jiraRequests = async (): Promise<unknown[]> => {
    const out: unknown[] = []
    for (const [site, path, q] of JIRA_REQUESTS) {
      let url: string | null = null
      const client = new JiraClient(
        (async (u: string) => {
          url = u
          return new Response('{}', { status: 200 })
        }) as unknown as typeof fetch,
        () => ({ kind: 'server', siteUrl: site, email: '', token: 't' })
      )
      const threw = await captured(client as never, path, q)
      out.push({ site, path, q, url: threw ? null : url })
    }
    return out
  }
  const images = async (): Promise<unknown[]> => {
    const out: unknown[] = []
    for (const c of IMAGE_CASES) {
      const calls: [string, boolean][] = []
      const client = new JiraClient(
        (async (u: string, init: RequestInit) => {
          calls.push([u, new Headers(init.headers).has('authorization')])
          const [status, loc] = c.script[u] ?? [404, null]
          if (status >= 300 && status < 400) return new Response(null, { status, headers: loc === null ? {} : { location: loc } })
          return new Response('abc', { status, headers: { 'content-type': 'image/png' } })
        }) as unknown as typeof fetch,
        () => ({ kind: 'server', siteUrl: c.site, email: '', token: 't' })
      )
      const got = await client.image(c.href)
      out.push({ site: c.site, href: c.href, script: c.script, calls, ok: got !== null })
    }
    return out
  }
  return { __async: { requests, jiraRequests, images }, idna: buildIdna(), grid, avatarHosts: AVATAR_HOSTS.map((h) => ({ h, ok: PUBLIC_AVATAR_HOST_RE.test(h) })) }
}

const NUMBER_STRINGS: string[] = [
  ...RETRY_AFTER_VALUES.filter((v): v is string => v !== null),
  ...RATE_REMAINING.filter((v): v is string => v !== null),
  ...RATE_RESET.filter((v): v is string => v !== null),
  '1e2', '1E2', '1e+2', '1e-2', '.', '+', '-', '+.5', '-.5', '5.e1', '0.0e0', '00', '007', '0x', '0X1F', '0b102', '0o8', '0B11', '0O17', '1 2', '1,5', '٣', '  7 　', '᠎5', '​5', '\u00855', 'Infinityx', 'infinity', '1e', 'e5',
  '9'.repeat(400), '0.' + '0'.repeat(400) + '1', '1'.padEnd(40, '0'), '-0.0', '+0', '4294967296', '123456789012345678', '0.1e1', '-5', '-1e-7'
]
const NUMBER_VALUES: number[] = [0, -0, 1, -1, 1.5, 100, 1e20, 1e21, 1.5e21, 1e-6, 1e-7, 1.5e-7, 123456.789, 2 ** 53, 1e300, 5e-324, 1.7976931348623157e308, 0.000001, 12345678901234567890, 4.35, 0.1 + 0.2]

function buildHttp(): Record<string, unknown> {
  // Retry-After·rate: Headers 가 바꾸지 않는 값만 헤더로 보낸다
  const retrySafe = RETRY_AFTER_VALUES.filter(headerSafe)
  const retryDet = retrySafe.filter((v) => parseRetryAfter(v, 0) === parseRetryAfter(v, 4e12))
  const fmt = (reset: string | null): string => new Date(Number(reset) * 1000).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' })

  const jiraErrors: unknown[] = []
  const addJira = (status: number, retry: string | null, body: string, kind: 'cloud' | 'server', notFound: string | null): void => {
    const res = new Response(null, { status, headers: hdr({ 'retry-after': retry }) })
    const e = jiraHttpError(res, body, kind, notFound === null ? undefined : notFound)
    jiraErrors.push({ status, retry, body, kind, notFound, out: { code: e.code, status: e.status, message: e.message } })
  }
  for (const kind of ['cloud', 'server'] as const) addJira(401, null, '{"errorMessages":["x"]}', kind, null)
  for (const status of [403, 400, 500]) for (const body of BODY_TEXTS) addJira(status, null, body, 'cloud', null)
  for (const nf of NOT_FOUNDS) for (const body of ['', '{"message":"m"}']) addJira(404, null, body, 'server', nf)
  for (const retry of retryDet) addJira(429, retry, '', 'cloud', null)
  for (const status of STATUSES_JIRA) for (const body of ['', '{"message":"m"}', '{"errors":{"a":"b"}}']) addJira(status, '5', body, 'cloud', null)

  const ghErrors: unknown[] = []
  const addGh = (status: number, h: { remaining: string | null; limit: string | null; reset: string | null; retry: string | null }, body: string, signed: boolean): void => {
    const res = new Response(null, { status, headers: hdr({ 'x-ratelimit-remaining': h.remaining, 'x-ratelimit-limit': h.limit, 'x-ratelimit-reset': h.reset, 'retry-after': h.retry }) })
    const e = ghHttpError(res, body, signed)
    // 한도가 풀리는 시각은 ko-KR 로컬 시각이라 시간대마다 다르다 — 표시를 자리표시자로 바꿔 적는다
    const message = e.code === 'GH_RATE_LIMIT' && e.message.includes('이후에') ? e.message.replace(fmt(h.reset), '{TIME}') : e.message
    ghErrors.push({ status, h, body, signed, out: { code: e.code, status: e.status, message } })
  }
  let n = 0
  const remainings = RATE_REMAINING.filter(headerSafe).slice(0, 9)
  const resets = RATE_RESET.filter(headerSafe).filter((v) => v === null || v === '1700000000' || v === 'abc')
  const retries = RATE_RETRY.filter(headerSafe).slice(0, 8)
  for (const status of [403, 429]) {
    for (const remaining of remainings) {
      for (const reset of resets) {
        for (const retry of retries) addGh(status, { remaining, limit: '60', reset, retry }, '{"message":"m"}', n++ % 2 === 0)
      }
    }
    for (const limit of RATE_LIMIT.filter(headerSafe)) addGh(status, { remaining: '0', limit, reset: '1700000000', retry: null }, '', n++ % 2 === 0)
  }
  const none = { remaining: null, limit: null, reset: null, retry: null }
  for (const status of [403, 500, 422]) for (const body of BODY_TEXTS) addGh(status, none, body, true)
  for (const status of STATUSES_GH) addGh(status, none, '{"message":"m"}', false)
  addGh(401, none, '', true)
  addGh(404, none, '', false)

  const rates: unknown[] = []
  for (const remaining of RATE_REMAINING.filter(headerSafe)) {
    for (const limit of RATE_LIMIT.filter(headerSafe)) {
      for (const reset of RATE_RESET.filter(headerSafe)) {
        const r = parseRate(hdr({ 'x-ratelimit-remaining': remaining, 'x-ratelimit-limit': limit, 'x-ratelimit-reset': reset }))
        rates.push({ remaining, limit, reset, out: r === null ? null : { remaining: enc(r.remaining), limit: enc(r.limit), reset: enc(r.reset) } })
      }
    }
  }

  const retryRows = RETRY_AFTER_VALUES.map((v) => {
    const out = tzStable(() => {
      const r = parseRetryAfter(v, NOW_MS)
      return r === null ? null : enc(r)
    })
    return { v, out }
  })

  const jsonTexts = [...BODY_TEXTS, ...JSON_EXTRA]
  return {
    number: NUMBER_STRINGS.map((s) => ({ s, out: enc(Number(s)) })),
    numberString: NUMBER_VALUES.map((n) => ({ n: enc(n), s: String(n) })),
    retryAfter: retryRows,
    linkNext: LINK_HEADERS.map((v) => ({ v, out: hasNextLink(v) })),
    rate: rates,
    bodyGh: BODY_TEXTS.map((t) => ({ t, out: toWellFormed(ghBodyMessage(t)) })),
    bodyJira: BODY_TEXTS.map((t) => ({ t, out: toWellFormed(jiraBodyMessage(t)) })),
    jiraError: jiraErrors,
    ghError: ghErrors,
    auth: AUTH_CASES.map((c) => ({ ...c, out: authHeader({ ...c, siteUrl: SITE_A }) })),
    json: jsonTexts.map((t) => ({ t, out: parseTree(t) })),
    nest: NEST_CASES.map((c) => {
      const t = nestText(c)
      return { ...c, json: parses(t) ? 'ok' : 'err', gh: toWellFormed(ghBodyMessage(t)), jira: toWellFormed(jiraBodyMessage(t)) }
    })
  }
}

function parses(text: string): boolean {
  try {
    JSON.parse(text)
    return true
  } catch {
    return false
  }
}

/** JSON.parse 결과를 모양 태그 트리로(Rust 가 같은 트리를 만들어 비교한다). 실패는 null */
function parseTree(text: string): unknown {
  let v: unknown
  try {
    v = JSON.parse(text)
  } catch {
    return null
  }
  const tree = (x: unknown): unknown => {
    if (x === null) return ['null']
    if (typeof x === 'boolean') return ['bool', x]
    if (typeof x === 'number') return ['num', enc(x)]
    if (typeof x === 'string') return ['str', toWellFormed(x)]
    if (Array.isArray(x)) return ['arr', x.map(tree)]
    return ['obj', Object.entries(x as object).map(([k, e]) => [toWellFormed(k), tree(e)])]
  }
  return tree(v)
}

async function buildDevice(): Promise<Record<string, unknown>> {
  const start = async (c: DeviceStartCase): Promise<unknown> => {
    const flow = new DeviceFlow(
      (async () => new Response(c.text, { status: c.status })) as unknown as typeof fetch,
      async () => {},
      () => 1000
    )
    try {
      const r = await flow.start(c.clientId)
      return { ok: { userCode: r.code.userCode, verificationUri: r.code.verificationUri, expiresIn: enc(r.code.expiresIn), interval: enc(r.code.interval), deviceCode: r.session.deviceCode, expiresAt: enc(r.session.expiresAt) } }
    } catch (e) {
      return e instanceof TypeError ? { typeError: true } : { err: (e as Error).message }
    }
  }
  const poll = async (c: DevicePollCase): Promise<unknown> => {
    let t = 0
    const sleeps: number[] = []
    const steps = [...c.steps]
    const flow = new DeviceFlow(
      (async () => {
        const s = steps.shift()
        if (!s) throw new Error('exhausted')
        if ('net' in s) throw new Error('net')
        return new Response(s.text, { status: s.status })
      }) as unknown as typeof fetch,
      async (ms: number) => {
        sleeps.push(ms)
        t += ms
      },
      () => t
    )
    let out: unknown
    try {
      out = { token: await flow.poll('cid', { deviceCode: 'dc', interval: c.interval, expiresAt: c.expiresIn * 1000 }) }
    } catch (e) {
      out = e instanceof TypeError ? { typeError: true } : { err: (e as Error).message }
    }
    if (JSON.stringify(out).includes('exhausted')) throw new Error(`폴링 사례의 단계가 모자라다: ${JSON.stringify(c).slice(0, 120)}`)
    return { ...c, sleeps: sleeps.map(enc), out }
  }
  const starts = []
  for (const c of DEVICE_STARTS) starts.push({ ...c, out: await start(c) })
  const polls = []
  for (const c of DEVICE_POLLS) polls.push(await poll(c))
  return { starts, polls }
}

function buildEnv(): Record<string, unknown> {
  return {
    env: ENV_CASES.map((pairs) => ({ in: pairs, out: Object.entries(ghEnv(Object.fromEntries(pairs) as NodeJS.ProcessEnv)) })),
    usable: USABLE_TOKENS.map((t) => ({ t, out: usableToken(t) })),
    redact: REDACT_CASES.map((c) => ({ ...c, out: redactToken(c.text, c.token) }))
  }
}

async function buildAll(): Promise<Record<string, string>> {
  const urls = buildUrls() as { __async: { requests: () => Promise<unknown[]>; jiraRequests: () => Promise<unknown[]>; images: () => Promise<unknown[]> } } & Record<string, unknown>
  const { __async, ...urlRest } = urls
  const handWritten = HAND_WRITTEN.map(([name, file, snippet]) => {
    const src = readFileSync(join(REPO_ROOT, file), 'utf8')
    expect(src.includes(snippet), `${file} 에 ${name} 의 정규식 ${snippet} 이 없다 — Rust 구현(손 판정)을 다시 본다`).toBe(true)
    return { name, file, snippet }
  })
  const runtime = { unicode: process.versions.unicode }
  return {
    'validate.json': render({ runtime, handWritten, ...buildValidate() }),
    'urls.json': render({ ...urlRest, requests: await __async.requests(), jiraRequests: await __async.jiraRequests(), images: await __async.images() }),
    'http.json': render({ runtime, ...buildHttp() }),
    'device.json': render(await buildDevice()),
    'env.json': render(buildEnv())
  }
}

describe('GitHub·Jira 순수 로직 골든', () => {
  it('대상 테스트 15개의 모든 it 이 대응표(test-map.json)에 있고, 가리키는 Rust 테스트가 있다', () => {
    const map = JSON.parse(readFileSync(join(GOLDEN_DIR, TEST_MAP), 'utf8')) as Record<string, string>
    const keys: string[] = []
    for (const f of TARGET_TESTS) {
      const seen = new Map<string, number>()
      for (const k of itKeys(readFileSync(join(__dirname, `${f}.test.ts`), 'utf8'))) {
        const n = (seen.get(k) ?? 0) + 1
        seen.set(k, n)
        // 같은 파일 안에서 제목이 겹치면 두 번째부터 ` #n` 을 붙인다(원본 제목은 고치지 않는다)
        keys.push(`${f} > ${k}${n > 1 ? ` #${n}` : ''}`)
      }
    }
    expect(new Set(keys).size).toBe(keys.length)
    expect(keys.filter((k) => !(k in map)), '대응표에 없는 it (대상 테스트에 it 을 더했으면 test-map.json 에도 적는다)').toEqual([])
    expect(Object.keys(map).filter((k) => !keys.includes(k)), '원본에 없는 대응표 항목').toEqual([])
    const rust = readFileSync(join(__dirname, '..', 'src-tauri', 'tests', 'net.rs'), 'utf8')
    for (const [title, value] of Object.entries(map)) {
      if (/^N\/A: PR [2-9]$/.test(value)) continue
      if (value.startsWith('N/A: ')) {
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(8)
        continue
      }
      const m = /^net\.rs::(\S+)$/.exec(value)
      expect(m, `${title}: 'N/A: PR n'·'N/A: 사유' 또는 'net.rs::<테스트 이름>' 이어야 한다 → ${value}`).not.toBeNull()
      expect(new RegExp(String.raw`\bfn ${m![1]!}\(`).test(rust), `${title}: net.rs 에 fn ${m![1]} 이 없다`).toBe(true)
    }
  })

  it('골든 파일이 TS 의 현재 출력과 같다', async () => {
    const built = await buildAll()
    if (UPDATE) {
      mkdirSync(GOLDEN_DIR, { recursive: true })
      for (const [name, text] of Object.entries(built)) writeFileSync(join(GOLDEN_DIR, name), text)
      return
    }
    const onDisk = readdirSync(GOLDEN_DIR).filter((n) => n.endsWith('.json') && n !== TEST_MAP).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts 로 골든을 만든다').toEqual(Object.keys(built).sort())
    for (const [name, text] of Object.entries(built)) {
      expect(existsSync(join(GOLDEN_DIR, name))).toBe(true)
      expect(readFileSync(join(GOLDEN_DIR, name), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
