import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import {
  cachePathMatchesLink,
  githubCachePath,
  githubCacheSlug,
  githubFullName,
  githubRemoteUrl,
  isGithubCachePath,
  isSafeBranch,
  sanitizeGithubLink,
  validateGithubLink
} from '../src/main/git/githubCache'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { mirrorOf } from '../src/sidecar/mirror'
import { fetchPrefsOf } from '../src/sidecar/fetchPrefs'
import { CACHE_BASES, LINK_INPUTS, BRANCH_INPUTS, MATCH_CASES, PATH_LINKS, SLUG_CASES, STORE_CASES, type ConfigCase, type Op } from './fixtures/configCases'
import { itKeys } from './fixtures/itTitles'

// ConfigStore(설정 저장소 코어)와 GitHub 링크 순수 함수의 골든(Phase 6b PR 1). TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenConfig.test.ts`.
// 입력 표는 tests/fixtures/configCases.ts, 검사하는 쪽은 src-tauri/tests/config.rs.
//
// **실제 TS ConfigStore** 를 임시 파일로 돌려 연산 결과·던진 문구·단계별 설정·**최종 파일 바이트**를 남긴다. 결정적이게 하려고:
// - 저장소 id(무작위 UUID)는 `node:crypto` 의 randomUUID 를 순번(`id-1`…)으로 바꾼다. Rust 는 같은 순번을 주입한다.
// - 손상 파일 백업 이름의 시각(`Date.now()`)은 고정 시각으로 둔다.
// - github 캐시 기준 폴더는 가상 경로(C:\golden\cache)다 — ConfigStore 는 이 문자열로 계산만 하고 디스크에 닿지 않는다. 그래서 골든에 실제 경로(임시 폴더·CI 의 8.3 짧은 이름)가 없다.
// - `node:path` 는 win32 판이다(Antler 는 Windows 용이고 Rust 이식도 항상 win32 규칙). 설정 파일 경로는 `/` 로 직접 이어 붙여 만들어 Linux 에서도 같게 돈다.

const FIXED_NOW = 1_700_000_000_000
const CACHE = 'C:\\golden\\cache'

const ids = vi.hoisted(() => ({ n: 0 }))

vi.mock('node:path', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:path')>()
  return { ...actual.win32, default: actual.win32, win32: actual.win32, posix: actual.posix }
})

vi.mock('node:crypto', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:crypto')>()
  const patched = { ...actual, randomUUID: () => `id-${++ids.n}` }
  return { ...patched, default: patched }
})

const GOLDEN = new URL('./golden/config/', import.meta.url)
const TEST_MAP = '../repos/test-map.json'
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 최상위 값이 배열이면 항목 하나를 한 줄에 적는다(사례 하나의 diff 가 한 줄로 보이게 하고 파일이 부풀지 않게). 나머지는 한 줄 JSON. 끝 줄바꿈 포함 */
function render(v: Record<string, unknown>): string {
  const rows = Object.entries(v).map(([k, x]) => (Array.isArray(x) ? `${JSON.stringify(k)}: [\n${x.map((e) => JSON.stringify(e)).join(',\n')}\n]` : `${JSON.stringify(k)}: ${JSON.stringify(x)}`))
  return `{\n${rows.join(',\n')}\n}\n`
}

/** JSON 으로 오가는 값으로 만든다(undefined 필드는 버려진다) */
const norm = (v: unknown): unknown => (v === undefined ? null : JSON.parse(JSON.stringify(v)))

function errText(e: unknown): string {
  // V8 의 TypeError 문구는 옮기지 않는다 — 종류만 비교한다
  return e instanceof TypeError ? 'TypeError' : (e as Error).message
}

function apply(store: ConfigStore, op: Op): unknown {
  switch (op.op) {
    case 'setUi':
      return store.setUi(op.patch as never)
    case 'setEditorExePath':
      return store.setEditorExePath(op.path)
    case 'setSkippedUpdateVersion':
      return store.setSkippedUpdateVersion(op.version)
    case 'addRepo':
      return store.addRepo(op.input as never, op.source as never)
    case 'updateRepo':
      return store.updateRepo(op.id, op.patch as never)
    case 'removeRepo':
      return store.removeRepo(op.id)
    case 'setGithubDefaultBranch':
      return store.setGithubDefaultBranch(op.id, op.branch)
    case 'setTrustedConfig':
      return store.setTrustedConfig(op.id, op.fingerprint)
    case 'setFetchApproval':
      return store.setFetchApproval(op.id, op.fingerprint)
  }
}

/** 설정을 돌려주는 연산 — 결과가 그 직후 설정과 같다(골든에는 `=config` 로만 적는다) */
const RETURNS_CONFIG = new Set<Op['op']>(['setUi', 'setEditorExePath', 'setSkippedUpdateVersion', 'updateRepo', 'removeRepo'])

function runCase(c: ConfigCase, dir: string): Record<string, unknown> {
  ids.n = 0
  mkdirSync(dir)
  const file = `${dir}/antler.config.json`
  if (c.fileIsDir) mkdirSync(file)
  else if (c.fileHex !== undefined) writeFileSync(file, Buffer.from(c.fileHex, 'hex'))
  else if (c.file !== undefined) writeFileSync(file, c.file, 'utf8')
  const store = new ConfigStore(file, c.cacheDir ? { cacheDir: CACHE } : {})
  let notified = 0
  store.onChange = () => {
    notified++
  }
  const initial = norm(store.get())
  // 단계 뒤 설정은 직전과 달라졌을 때만 적는다(없으면 직전과 같다 — 골든이 부풀지 않게)
  let prev = JSON.stringify(initial)
  const after = (): { config?: unknown } => {
    const config = norm(store.get())
    const text = JSON.stringify(config)
    if (text === prev) return {}
    prev = text
    return { config }
  }
  const steps = (c.ops ?? []).map((op) => {
    try {
      const out = apply(store, op)
      const changed = after()
      if (RETURNS_CONFIG.has(op.op)) {
        expect(norm(out), `${c.name}: ${op.op} 의 반환값이 직후 설정과 다르다`).toEqual(norm(store.get()))
        return { result: '=config', ...changed }
      }
      return { result: norm(out), ...changed }
    } catch (e) {
      return { error: errText(e), ...after() }
    }
  })
  const final = norm(store.get()) as never
  const text = existsSync(file) && !c.fileIsDir ? readFileSync(file, 'utf8') : null
  return {
    name: c.name,
    input: { file: c.file ?? null, fileHex: c.fileHex ?? null, fileIsDir: c.fileIsDir ?? false, cacheDir: c.cacheDir ?? false, ops: c.ops ?? [] },
    loadFailed: store.loadFailed,
    notified,
    initial,
    steps,
    // 최종 파일 바이트(없으면 null)
    file: text,
    files: readdirSync(dir).sort(),
    mirror: norm(mirrorOf(final)),
    fetchPrefs: norm(fetchPrefsOf(final))
  }
}

function buildStore(root: string): string {
  const names = new Set<string>()
  const cases = STORE_CASES.map((c, i) => {
    expect(names.has(c.name), `사례 이름이 겹친다: ${c.name}`).toBe(false)
    names.add(c.name)
    return runCase(c, `${root}/c${i}`)
  })
  return render({ now: FIXED_NOW, cacheDir: CACHE, cases })
}

function attempt<T>(f: () => T): { ok: T } | { error: string } {
  try {
    return { ok: f() }
  } catch (e) {
    return { error: errText(e) }
  }
}

function buildLink(): string {
  return render({
    validate: LINK_INPUTS.map((input) => ({ input, result: norm(attempt(() => validateGithubLink(input))) })),
    sanitize: LINK_INPUTS.map((input) => ({ input, result: norm(sanitizeGithubLink(input)) })),
    branches: BRANCH_INPUTS.map((input) => ({ input, safe: isSafeBranch(input) })),
    slugs: SLUG_CASES.map(([base, path]) => ({ base, path, slug: norm(githubCacheSlug(base, path)), isCachePath: isGithubCachePath(base, path) })),
    matches: MATCH_CASES.map(([path, link]) => ({ path, link, matches: cachePathMatchesLink(CACHE, path, link) })),
    paths: CACHE_BASES.flatMap((base) => PATH_LINKS.map((link) => ({ base, link, path: githubCachePath(base, link) }))),
    urls: PATH_LINKS.flatMap((link) =>
      [undefined, 'file:///C:/golden/origin', 'https://example.test'].map((origin) => ({ link, origin: origin ?? null, url: githubRemoteUrl(link, origin), fullName: githubFullName(link) }))
    )
  })
}

function build(): Record<string, string> {
  const root = mkdtempSync(`${tmpdir()}/antler-goldencfg-`.replace(/\\/g, '/'))
  try {
    return { 'store.json': buildStore(root), 'link.json': buildLink() }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

/** 이식 대응표의 Rust 파일 이름 → 소스 경로(이 파일 기준) */
const RUST_SOURCES: Record<string, string> = {
  'config.rs': '../src-tauri/tests/config.rs',
  // GitHub 캐시 실행 기반(PR 2): 골든·실제 git 시험 둘과 실행기의 단위 시험
  'github_cache_exec.rs': '../src-tauri/tests/github_cache_exec.rs',
  'github_cache_isolation.rs': '../src-tauri/tests/github_cache_isolation.rs',
  // github kind 의 git 조회(PR 6)
  'github_route.rs': '../src-tauri/tests/github_route.rs',
  // github kind 의 fetch·감시(PR 7)
  'github_fetch.rs': '../src-tauri/tests/github_fetch.rs',
  // 설정 소유 전환(PR 9): 채널 → 변경 연산·삭제 순서·고아 정리·external 연결
  'config_route.rs': '../src-tauri/tests/config_route.rs',
  'github.rs': '../src-tauri/src/git/exec/github.rs',
  'github_link.rs': '../src-tauri/src/git/github_link.rs',
  'load.rs': '../src-tauri/src/config/load.rs',
  'model.rs': '../src-tauri/src/config/model.rs',
  'save.rs': '../src-tauri/src/config/save.rs',
  'validate.rs': '../src-tauri/src/config/validate.rs'
}

/** 대응표가 덮는 TS 테스트 파일(6b 전용 표 — tests/golden/repos/test-map.json) */
const TARGET_TESTS = ['configStore', 'windowState', 'githubRemote']

describe('설정 저장소 골든', () => {
  let fixtures: Record<string, string> = {}
  beforeAll(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(FIXED_NOW)
    fixtures = build()
  })
  afterAll(() => {
    vi.useRealTimers()
  })

  it('TS 테스트 이식 대응표(tests/golden/repos/test-map.json)가 대상 테스트의 모든 it 을 덮고 가리키는 Rust 테스트가 있다', () => {
    const map = JSON.parse(readFileSync(new URL(TEST_MAP, GOLDEN), 'utf8')) as Record<string, string>
    const keys: string[] = []
    for (const f of TARGET_TESTS) {
      const seen = new Map<string, number>()
      for (const k of itKeys(readFileSync(new URL(`./${f}.test.ts`, import.meta.url), 'utf8'))) {
        const n = (seen.get(k) ?? 0) + 1
        seen.set(k, n)
        // 같은 파일 안에서 제목이 겹치면 두 번째부터 ` #n` 을 붙인다(원본 제목은 고치지 않는다)
        keys.push(`${f} > ${k}${n > 1 ? ` #${n}` : ''}`)
      }
    }
    expect(new Set(keys).size).toBe(keys.length)
    expect(keys.filter((k) => !(k in map)), '대응표에 없는 it (대상 테스트에 it 을 더했으면 test-map.json 에도 적는다)').toEqual([])
    expect(Object.keys(map).filter((k) => !keys.includes(k)), '원본에 없는 대응표 항목').toEqual([])
    const sources = new Map<string, string>()
    for (const [title, value] of Object.entries(map)) {
      // 뒤 PR 로 미루는 항목(`N/A: PR n`)은 없다 — Phase 6b 의 마지막 PR(10)에서 0 개로 고정했다
      expect(value.startsWith('N/A: PR '), `${title}: 'N/A: PR n' 은 남길 수 없다(옮기거나 사유를 적는다) → ${value}`).toBe(false)
      if (value.startsWith('N/A: ')) {
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(8)
        continue
      }
      const m = /^([a-z0-9_]+\.rs)::(\S+)$/.exec(value)
      expect(m, `${title}: 'N/A: PR n'·'N/A: 사유' 또는 '<파일>.rs::<테스트 이름>' 이어야 한다 → ${value}`).not.toBeNull()
      const rel = RUST_SOURCES[m![1]!]
      expect(rel, `${title}: 알 수 없는 Rust 파일 ${m![1]}`).toBeDefined()
      if (!sources.has(rel!)) sources.set(rel!, readFileSync(new URL(rel!, import.meta.url), 'utf8'))
      expect(new RegExp(String.raw`\bfn ${m![2]!}\(`).test(sources.get(rel!)!), `${title}: ${m![1]} 에 fn ${m![2]} 이 없다`).toBe(true)
    }
  })

  it('골든이 비어 있지 않고 손상·검증 거부·쓰기 사례가 고루 들어 있다', () => {
    const store = JSON.parse(fixtures['store.json']!) as { cases: { loadFailed: boolean; steps: { error?: string }[]; file: string | null; files: string[] }[] }
    expect(store.cases.length).toBeGreaterThan(200)
    expect(store.cases.some((c) => c.loadFailed && c.files.some((f) => f.includes('.corrupt-1700000000000.bak')))).toBe(true)
    expect(store.cases.some((c) => c.steps.some((s) => s.error === 'TypeError'))).toBe(true)
    expect(store.cases.some((c) => c.steps.some((s) => s.error?.startsWith('잘못된')))).toBe(true)
    expect(store.cases.filter((c) => c.file !== null).length).toBeGreaterThan(100)
    // 임시 파일이 남지 않는다
    expect(store.cases.flatMap((c) => c.files).filter((f) => f.endsWith('.tmp'))).toEqual([])
  })

  it('골든 파일이 TS 의 현재 출력과 같다', () => {
    if (UPDATE) {
      mkdirSync(GOLDEN, { recursive: true })
      for (const [name, text] of Object.entries(fixtures)) writeFileSync(new URL(name, GOLDEN), text)
      return
    }
    const onDisk = readdirSync(GOLDEN).filter((n) => n.endsWith('.json')).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenConfig.test.ts 로 골든을 만든다').toEqual(Object.keys(fixtures).sort())
    for (const [name, text] of Object.entries(fixtures)) {
      expect(readFileSync(new URL(name, GOLDEN), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
