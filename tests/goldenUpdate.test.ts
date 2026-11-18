import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { UpdatePolicy, compareSemver, isAllowedDownloadUrl, isOfferable, isReleaseAssetUrl, isSafeFileName, parseDismiss, parseSemver, releaseNotesUrl, sanitizeError } from '../src/main/update/logic'
import { fetchReleaseNotes } from '../src/main/update/releaseNotes'
import { itKeys } from './fixtures/itTitles'
import { runScenarios, type Scenario } from './fixtures/updateScenarios'
import {
  ALLOWED_URLS,
  ASSET_URLS,
  DISMISS_CASES,
  MANIFEST_CASES,
  NOTES_CASES,
  NOTES_URL_VERSIONS,
  POLICY_SCENARIOS,
  SAFE_NAMES,
  SANITIZE_INPUTS,
  SEMVER_INPUTS,
  VERSION_LIST
} from './fixtures/updateCases'

// 업데이트 판정(버전 비교·URL 허용 목록·정책·입력 검증·오류 정화·릴리스 노트)의 골든(Phase 6c PR 1). TS 가 기준이다: `UPDATE_GOLDEN=1 pnpm exec vitest run tests/goldenUpdate.test.ts`.
// 입력 표는 tests/fixtures/updateCases.ts, 검사하는 쪽은 src-tauri/tests/update.rs. 네트워크를 쓰지 않는다 — 릴리스 노트는 모의 fetch 로 돈다.
// 매니페스트(latest.json) 검사는 TS 에 짝이 없다: 골든에는 TS 의 부품 판정(isOfferable·isReleaseAssetUrl)만 담고, Rust 가 "허용하면 TS 부품도 허용한다"를 단언한다.

const GOLDEN = new URL('./golden/update/', import.meta.url)
const TEST_MAP = 'test-map.json'
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 사람이 읽는 2칸 들여쓰기 JSON + 끝 줄바꿈(undefined 필드는 JSON 이 버린다) */
const render = (v: unknown): string => `${JSON.stringify(v, null, 2)}\n`

type Out = { ok: unknown } | { err: string }

function run(f: () => unknown): Out {
  try {
    return { ok: f() ?? null }
  } catch (e) {
    return { err: (e as Error).message }
  }
}

const srcOf = (file: string): string => readFileSync(new URL(`../src/main/update/${file}`, import.meta.url), 'utf8').replace(/\r\n/g, '\n')

/** Rust 가 손으로 옮긴 정규식(src-tauri/src/update/logic.rs 의 HAND_WRITTEN 과 글자 그대로 같아야 한다) */
const HAND_WRITTEN: [string, string, string][] = [
  ['SEMVER_RE', 'logic.ts', String.raw`/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z.-]+)?$/`],
  ['SAFE_FILE_NAME', 'logic.ts', String.raw`/^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$/`],
  ['ENCODED_SEPARATOR', 'logic.ts', String.raw`/%(2e|2f|5c)/i`],
  ['NUMERIC_IDENTIFIER', 'logic.ts', String.raw`/^\d+$/`],
  ['SANITIZE_URL', 'logic.ts', String.raw`/https?:\/\/\S+/g`],
  ['SANITIZE_GITHUB_TOKEN', 'logic.ts', String.raw`/\b(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g`],
  ['SANITIZE_SECRET', 'logic.ts', String.raw`/(token|authorization|bearer)\s*[:=]?\s*\S+/gi`],
  ['SANITIZE_PATH', 'logic.ts', String.raw`/[A-Za-z]:\\[^\s'"]+|\/(?:home|Users|tmp|var)\/[^\s'"]+/g`],
  ['NOTES_TAG_VERSION', 'releaseNotes.ts', String.raw`/^[0-9A-Za-z.+-]{1,64}$/`]
]

const cmp = (a: string, b: string): number | null => compareSemver(a, b)

const SCENARIOS = (JSON.parse(readFileSync(new URL('./fixtures/update.scenarios.json', import.meta.url), 'utf8')) as { scenarios: Scenario[] }).scenarios

// ── 릴리스 노트 모의 fetch ────────────────────────────────────────────

const API = 'https://api.github.com/repos/Kang-Jacob-GitLB/antler'

/** 큰 본문·제목은 길이와 머리만 남긴다(골든이 커지지 않게). Rust 쪽 시험도 같은 규칙으로 줄여 비교한다 */
const brief = (s: string): string => (s.length > 200 ? `len:${s.length}:${s.slice(0, 20)}` : s)

async function notesOut(c: (typeof NOTES_CASES)[number]): Promise<{ requests: string[]; out: Out }> {
  const requests: string[] = []
  const f = (async (input: RequestInfo | URL) => {
    const path = String(input).slice(API.length)
    requests.push(path)
    const r = c.routes[path]
    if (!r) return new Response('not found', { status: 404 })
    const status = r.status ?? 200
    const text = typeof r.body === 'string' ? r.body : r.body === undefined ? '' : JSON.stringify(r.body)
    return new Response([204, 205, 304].includes(status) ? null : text, { status })
  }) as typeof fetch
  try {
    const n = await fetchReleaseNotes(f, c.current)
    return { requests, out: { ok: { ...n, body: brief(n.body), ...(n.title === undefined ? {} : { title: brief(n.title) }) } } }
  } catch (e) {
    return { requests, out: { err: (e as Error).message } }
  }
}

// ── 골든 만들기 ───────────────────────────────────────────────────────

async function build(): Promise<Record<string, string>> {
  const semver = {
    parse: SEMVER_INPUTS.map((input) => ({ input, out: parseSemver(input) })),
    versions: VERSION_LIST,
    // 행 = versions[i], 열 = versions[j]. compare: -1·0·1·n(null), offerable(행 = 현재, 열 = 후보): 1·0
    compare: VERSION_LIST.map((a) => VERSION_LIST.map((b) => String(cmp(a, b) ?? 'n')).join(' ')),
    offerable: VERSION_LIST.map((current) => VERSION_LIST.map((candidate) => (isOfferable(current, candidate) ? '1' : '0')).join(''))
  }
  const url = {
    allowed: ALLOWED_URLS.map((input) => ({ input, out: isAllowedDownloadUrl(input) })),
    asset: ASSET_URLS.map((input) => ({ input, out: isReleaseAssetUrl(input) })),
    notesUrl: NOTES_URL_VERSIONS.map((input) => ({ input, out: releaseNotesUrl(input) })),
    safeName: SAFE_NAMES.map((input) => ({ input, out: isSafeFileName(input) }))
  }
  const policy = POLICY_SCENARIOS.map((s) => {
    let skipped: string | undefined
    let p = new UpdatePolicy(() => skipped)
    const steps = s.steps.map((st) => {
      if (st.op === 'snooze') p.snooze(st.version)
      else if (st.op === 'skip') skipped = st.value ?? undefined
      else if (st.op === 'restart') p = new UpdatePolicy(() => skipped)
      else return { ...st, out: p.shouldNotify(st.version, st.manual) }
      return { ...st }
    })
    return { name: s.name, steps }
  })
  const dismiss = DISMISS_CASES.map((c) => ({ name: c.name, action: c.action, version: c.version, out: run(() => parseDismiss(c.action, c.version)) }))
  const sanitize = SANITIZE_INPUTS.map((input) => ({ input, out: sanitizeError(new Error(input)) }))
  const manifest = MANIFEST_CASES.map((c) => {
    // TS 의 부품 판정: 매니페스트가 JSON 이고 version·platforms 가 보이면 그 값으로
    let parts: { version: string; offerable: boolean; urlOk: boolean | null } | null = null
    try {
      const m = JSON.parse(c.text) as { version?: unknown; platforms?: Record<string, { url?: unknown }> }
      if (typeof m.version === 'string') {
        const e = m.platforms?.['windows-x86_64-nsis'] ?? m.platforms?.['windows-x86_64']
        parts = { version: m.version, offerable: isOfferable(c.current, m.version.replace(/^v/, '')), urlOk: typeof e?.url === 'string' ? isReleaseAssetUrl(e.url) : null }
      }
    } catch {
      parts = null
    }
    return { name: c.name, current: c.current, text: c.text, expect: c.expect, parts }
  })
  const notes = []
  for (const c of NOTES_CASES) notes.push({ name: c.name, current: c.current, routes: c.routes, ...(await notesOut(c)) })

  const handWritten = HAND_WRITTEN.map(([name, file, snippet]) => {
    expect(srcOf(file).includes(snippet), `${file} 에 ${name} 의 정규식 ${snippet} 이 없다 — Rust 구현(손 판정)을 다시 본다`).toBe(true)
    return { name, file, snippet }
  })
  return {
    'semver.json': render({ handWritten, ...semver }),
    'url.json': render(url),
    'policy.json': render({ scenarios: policy }),
    'dismiss.json': render({ cases: dismiss }),
    'sanitize.json': render({ cases: sanitize }),
    'manifest.json': render({ cases: manifest }),
    'notes.json': render({ cases: notes }),
    // 서비스 상태 순서(PR 4): 같은 입력(tests/fixtures/update.scenarios.json)을 Rust 서비스(src-tauri/tests/update.rs)도 돌려 이 출력과 비교한다
    'scenarios.json': render({ scenarios: await runScenarios(SCENARIOS) })
  }
}

// ── 대응표 ────────────────────────────────────────────────────────────

/** 대응표 값의 `<파일>::<이름>` 이 가리키는 Rust 소스 */
const RUST_SOURCES: Record<string, string> = {
  'update.rs': '../src-tauri/tests/update.rs',
  'update_security.rs': '../src-tauri/tests/update_security.rs',
  'logic.rs': '../src-tauri/src/update/logic.rs',
  'config.rs': '../src-tauri/tests/config.rs'
}

describe('업데이트 판정 골든(TS 기준)', () => {
  it('update.test.ts 의 모든 it 이 대응표(test-map.json)에 있고, 가리키는 Rust 테스트가 있다', () => {
    const map = JSON.parse(readFileSync(new URL(TEST_MAP, GOLDEN), 'utf8')) as Record<string, string>
    const seen = new Map<string, number>()
    const keys: string[] = []
    for (const k of itKeys(readFileSync(new URL('./update.test.ts', import.meta.url), 'utf8'))) {
      const n = (seen.get(k) ?? 0) + 1
      seen.set(k, n)
      // 같은 파일 안에서 제목이 겹치면 두 번째부터 ` #n` 을 붙인다(원본 제목은 고치지 않는다)
      keys.push(`${k}${n > 1 ? ` #${n}` : ''}`)
    }
    expect(new Set(keys).size).toBe(keys.length)
    expect(keys.filter((k) => !(k in map)), '대응표에 없는 it (update.test.ts 에 it 을 더했으면 test-map.json 에도 적는다)').toEqual([])
    expect(Object.keys(map).filter((k) => !keys.includes(k)), '원본에 없는 대응표 항목').toEqual([])
    const sources = new Map<string, string>()
    for (const [title, value] of Object.entries(map)) {
      if (value.startsWith('N/A: ')) {
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(12)
        continue
      }
      const m = /^([a-z0-9_]+\.rs)::(\S+)$/.exec(value)
      expect(m, `${title}: 'N/A: 사유' 또는 '<파일>.rs::<테스트 이름>' 이어야 한다 → ${value}`).not.toBeNull()
      const rel = RUST_SOURCES[m![1]!]
      expect(rel, `${title}: 알 수 없는 Rust 파일 ${m![1]}`).toBeDefined()
      if (!sources.has(rel!)) sources.set(rel!, readFileSync(new URL(rel!, import.meta.url), 'utf8'))
      expect(new RegExp(String.raw`\bfn ${m![2]!}\(`).test(sources.get(rel!)!), `${title}: ${m![1]} 에 fn ${m![2]} 이 없다`).toBe(true)
    }
  })

  it('골든 파일이 TS 의 현재 출력과 같다', async () => {
    const built = await build()
    if (UPDATE) {
      mkdirSync(GOLDEN, { recursive: true })
      for (const [name, text] of Object.entries(built)) writeFileSync(new URL(name, GOLDEN), text)
      return
    }
    const onDisk = readdirSync(GOLDEN).filter((n) => n.endsWith('.json') && n !== TEST_MAP).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 pnpm exec vitest run tests/goldenUpdate.test.ts 로 골든을 만든다').toEqual(Object.keys(built).sort())
    for (const [name, text] of Object.entries(built)) {
      expect(readFileSync(new URL(name, GOLDEN), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
