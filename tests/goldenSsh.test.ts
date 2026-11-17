import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { describe, expect, it, vi } from 'vitest'
import { listHosts, parseHostSpec, parseSshConfig, resolveHost, type ResolvedHost } from '../src/main/executors/ssh/config'
import {
  checkHostKey,
  fingerprint,
  knownHostKeyAlgorithms,
  knownHostsName,
  keyTypeOfBlob,
  parseKnownHosts,
  verifyHostKey
} from '../src/main/executors/ssh/knownHosts'
import { shQuote } from '../src/main/executors/ssh/quote'
import { itKeys } from './fixtures/itTitles'
import {
  CONFIG_CASES,
  HOME,
  KEYS,
  KEY_B64,
  KNOWN_HOSTS_CASES,
  LOCAL_USER,
  QUOTE_EXTRA,
  SPEC_CASES,
  TRICKY
} from './fixtures/sshCases'

// SSH 순수 로직(설정 파서·known_hosts·원격 명령 인용)의 골든. TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenSsh.test.ts`.
// 입력 표는 tests/fixtures/sshCases.ts, 검사하는 쪽은 src-tauri/tests/ssh_golden.rs. 원격 명령 바이트(buildRemoteCommand)는 tests/goldenExec.test.ts 가 exec/env.json 의 `ssh` 절로 만든다.
//
// 플랫폼과 무관하게 같은 골든이 나오도록 `node:path` 는 win32 판으로, `node:fs` 는 가상 파일 시스템(C:\golden 아래)으로 바꾼다 — Antler 는 Windows 용이라
// 설정 값의 경로 정규화는 win32 규칙이어야 하고(Rust 이식도 항상 win32), 임시 디렉터리에 기대지 않는다. 가상 파일 시스템은 대소문자를 구분하지 않는다(Windows).

const vfs = vi.hoisted(() => ({ files: [] as [string, string][] }))

vi.mock('node:path', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:path')>()
  return { ...actual.win32, default: actual.win32, win32: actual.win32, posix: actual.posix }
})

vi.mock('node:fs', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:fs')>()
  const { win32 } = await vi.importActual<typeof import('node:path')>('node:path')
  const isVirtual = (p: unknown): p is string => typeof p === 'string' && /^c:[\\/]+golden(?:[\\/]|$)/i.test(p)
  const key = (p: string): string => win32.normalize(p).toLowerCase().replace(/\\+$/, '')
  const find = (p: string): [string, string] | undefined => vfs.files.find(([f]) => key(f) === key(p))
  const enoent = (p: string): Error => Object.assign(new Error(`ENOENT: ${p}`), { code: 'ENOENT' })
  const patched = {
    ...actual,
    statSync: (p: unknown, ...rest: unknown[]) => {
      if (!isVirtual(p)) return (actual.statSync as (...a: unknown[]) => unknown)(p, ...rest)
      if (!find(p)) throw enoent(p)
      return { isFile: () => true }
    },
    readFileSync: (p: unknown, ...rest: unknown[]) => {
      if (!isVirtual(p)) return (actual.readFileSync as (...a: unknown[]) => unknown)(p, ...rest)
      const f = find(p)
      if (!f) throw enoent(p)
      return f[1]
    },
    readdirSync: (p: unknown, ...rest: unknown[]) => {
      if (!isVirtual(p)) return (actual.readdirSync as (...a: unknown[]) => unknown)(p, ...rest)
      const want = key(p)
      return vfs.files.filter(([f]) => key(win32.dirname(key(f))) === want).map(([f]) => win32.basename(f))
    }
  }
  return { ...patched, default: patched }
})

const GOLDEN = new URL('./golden/ssh/', import.meta.url)
const TEST_MAP = 'test-map.json'
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 사람이 읽는 2칸 들여쓰기 JSON + 끝 줄바꿈(undefined 필드는 JSON 이 버린다) */
const render = (v: unknown): string => `${JSON.stringify(v, null, 2)}\n`

// ── 설정 파서 ─────────────────────────────────────────────────────────────────

function renderResolved(r: ResolvedHost): Record<string, unknown> {
  return {
    alias: r.alias,
    hostName: r.hostName,
    user: r.user,
    port: r.port,
    identityFiles: r.identityFiles,
    proxyJump: r.proxyJump.map((h) => ({ spec: h.spec, user: h.user ?? null, host: h.host, port: h.port ?? null })),
    identitiesOnly: r.identitiesOnly,
    identityAgent: r.identityAgent === undefined ? { kind: 'default' } : r.identityAgent === null ? { kind: 'none' } : { kind: 'path', path: r.identityAgent },
    unsupported: r.unsupported ?? null,
    knownHostsFiles: r.knownHostsFiles ?? [],
    hostKeyAlias: r.hostKeyAlias ?? null
  }
}

function buildConfig(): unknown {
  const cases = CONFIG_CASES.map((c) => {
    vfs.files = Object.entries(c.files ?? {})
    const env = c.env ?? {}
    for (const [k, v] of Object.entries(env)) process.env[k] = v
    delete process.env.UNSET_VAR
    try {
      const cfg = parseSshConfig(c.text, { homeDir: HOME })
      const opt = { localUser: LOCAL_USER }
      return {
        name: c.name,
        text: c.text,
        files: c.files ?? {},
        env,
        queries: c.queries.map((q) => {
          const spec = typeof q === 'string' ? q : q.spec
          return { spec, ...(typeof q === 'string' ? {} : { rust: q.rust }), resolved: renderResolved(resolveHost(cfg, spec, opt)) }
        }),
        hosts: listHosts(cfg, opt)
      }
    } finally {
      for (const k of Object.keys(env)) delete process.env[k]
      vfs.files = []
    }
  })
  const specs = SPEC_CASES.map((spec) => {
    const s = parseHostSpec(spec)
    return { spec, out: { user: s.user ?? null, host: s.host, port: s.port ?? null } }
  })
  return { home: HOME, localUser: LOCAL_USER, cases, specs }
}

// ── known_hosts ───────────────────────────────────────────────────────────────

function buildKnownHosts(): unknown {
  const cases = KNOWN_HOSTS_CASES.map((c) => {
    const text = c.text(KEY_B64)
    return {
      name: c.name,
      text,
      entries: parseKnownHosts(text).map((e) => ({ marker: e.marker ?? null, hosts: e.hosts, keyType: e.keyType, keyBase64: e.keyBase64 })),
      queries: c.queries.map((q, i) => {
        const key = KEYS[q.key]!
        vfs.files = [['C:\\golden\\known_hosts', text]]
        const v = verifyHostKey('C:\\golden\\known_hosts', q.host, q.port, key)
        vfs.files = []
        return {
          host: q.host,
          port: q.port,
          key: q.key,
          keyBase64: KEY_B64[q.key],
          name: knownHostsName(q.host, q.port),
          type: keyTypeOfBlob(key),
          fingerprint: fingerprint(key),
          status: checkHostKey(text, q.host, q.port, key).status,
          algorithms: knownHostKeyAlgorithms(text, q.host, q.port),
          verify: v.ok ? { ok: true } : { ok: false, message: v.message },
          ...(c.rust?.[i] !== undefined ? { rustStatus: c.rust[i] } : {})
        }
      })
    }
  })
  return { cases }
}

// ── 인용 ──────────────────────────────────────────────────────────────────────

function buildQuote(): unknown {
  const quote = [...TRICKY, ...QUOTE_EXTRA].map((input) => {
    try {
      return { input, ok: shQuote(input) }
    } catch (e) {
      return { input, error: (e as Error).message }
    }
  })
  return { tricky: TRICKY, quote }
}

const build = (): Record<string, string> => ({
  'config.json': render(buildConfig()),
  'knownHosts.json': render(buildKnownHosts()),
  'quote.json': render(buildQuote())
})

// ── 이식 대응표 ────────────────────────────────────────────────────────────────

const TARGET_TESTS = ['sshConfig', 'sshKnownHosts', 'sshQuote', 'sshExecutor', 'sshRemoteDir']

/** 대응표 값의 `<파일>::<이름>` 이 가리키는 Rust 소스 */
const RUST_SOURCES: Record<string, string> = {
  'ssh_golden.rs': '../src-tauri/tests/ssh_golden.rs',
  'ssh.rs': '../src-tauri/tests/ssh.rs',
  'config.rs': '../src-tauri/src/git/exec/ssh/config.rs',
  'known_hosts.rs': '../src-tauri/src/git/exec/ssh/known_hosts.rs',
  'quote.rs': '../src-tauri/src/git/exec/ssh/quote.rs',
  'mod.rs': '../src-tauri/src/git/exec/ssh/mod.rs',
  'win32_path.rs': '../src-tauri/src/git/exec/ssh/win32_path.rs'
}

describe('SSH 순수 로직 골든', () => {
  it('대상 테스트 5개의 모든 it 이 대응표(test-map.json)에 있고, 가리키는 Rust 테스트가 있다', () => {
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
      if (/^N\/A: PR [2-9]$/.test(value)) continue
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

  it('골든 파일이 TS 의 현재 출력과 같다', () => {
    const built = build()
    if (UPDATE) {
      mkdirSync(GOLDEN, { recursive: true })
      for (const [name, text] of Object.entries(built)) writeFileSync(new URL(name, GOLDEN), text)
      return
    }
    const onDisk = readdirSync(GOLDEN).filter((n) => n.endsWith('.json') && n !== TEST_MAP).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenSsh.test.ts 로 골든을 만든다').toEqual(Object.keys(built).sort())
    for (const [name, text] of Object.entries(built)) {
      expect(readFileSync(new URL(name, GOLDEN), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
