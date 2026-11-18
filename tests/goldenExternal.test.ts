import { EventEmitter } from 'node:events'
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { posix, win32 } from 'node:path'
import { describe, expect, it } from 'vitest'
import { allowedLinkUrl } from '../src/main/external/link'
import { ExternalOpener, checkChosenExe, findEditorExe, resolveEditorExe, terminalTools, type OpenerDeps } from '../src/main/external/opener'
import {
  absoluteTarget,
  buildEditorLaunch,
  buildRevealPlan,
  buildTerminalLaunch,
  checkRealContainment,
  clsidComponent,
  hasDotComponent,
  relSegments
} from '../src/main/external/plan'
import { itKeys } from './fixtures/itTitles'
import {
  ABS_CASES,
  CHOSEN_CASES,
  CLSID_CASES,
  CONTAIN_CASES,
  DOT_CASES,
  EDITOR_CASES,
  FIND_CASES,
  LINK_CASES,
  REL_CASES,
  REVEAL_CASES,
  SCENARIOS,
  TERMINAL_CASES,
  TOOLS_CASES,
  type ExeCase,
  type Plat
} from './fixtures/externalCases'

// 외부 도구로 열기(경로 검증·실행 인자 계획·실행기·링크 허용)의 골든. TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenExternal.test.ts`.
// 입력 표는 tests/fixtures/externalCases.ts, 검사하는 쪽은 src-tauri/tests/external.rs. 어떤 프로그램도 띄우지 않는다 — 실행기·파일 시스템은 가짜다.
// 시나리오의 가짜 파일 시스템은 "경로 → 푼 결과" 통째 대응이다(링크 구성 요소별 해석은 하지 않는다).

const GOLDEN = new URL('./golden/external/', import.meta.url)
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

const launchOf = (l: { file: string; args: string[]; cwd?: string }): unknown => ({ file: l.file, args: l.args, ...(l.cwd === undefined ? {} : { cwd: l.cwd }) })

// ── 가짜 의존성 ───────────────────────────────────────────────────────────────

interface FakeOpts {
  platform: Plat
  files?: string[]
  dirs?: string[]
  links?: Record<string, string>
  env?: Record<string, string>
  lstatAlways?: boolean
}

function fake(o: FakeOpts) {
  const files = new Set(o.files ?? [])
  const dirs = new Set(o.dirs ?? [])
  const links = o.links ?? {}
  const spawned: unknown[] = []
  const shown: unknown[] = []
  const real = (p: string): string => {
    const r = links[p] ?? p
    if (!files.has(r) && !dirs.has(r)) throw new Error('ENOENT')
    return r
  }
  const stat = (p: string) => {
    const r = real(p)
    return { isFile: () => files.has(r), isDirectory: () => dirs.has(r) }
  }
  const deps: OpenerDeps = {
    platform: o.platform,
    env: o.env ?? {},
    fs: { realpathSync: real, statSync: stat, lstatSync: o.lstatAlways ? () => ({ isFile: () => true, isDirectory: () => false }) : stat },
    spawn: (file, args, opts) => {
      spawned.push(launchOf({ file, args, ...(opts.cwd ? { cwd: opts.cwd } : {}) }))
      const ee = new EventEmitter() as EventEmitter & { unref(): void }
      ee.unref = () => {}
      setImmediate(() => ee.emit('spawn'))
      return ee as never
    },
    shell: {
      openPath: async (p) => {
        shown.push({ type: 'openPath', path: p })
        return ''
      },
      showItemInFolder: (p) => shown.push({ type: 'showItem', path: p })
    }
  }
  return { deps, spawned, shown }
}

// ── 골든 만들기 ───────────────────────────────────────────────────────────────

const toolsJson = (t: { systemRoot?: string; wt?: string | null; linuxTerminal?: string | null; macTerminal?: string | null }): unknown => ({
  systemRoot: t.systemRoot ?? null,
  wt: t.wt ?? null,
  linuxTerminal: t.linuxTerminal ?? null,
  macTerminal: t.macTerminal ?? null
})

async function build(): Promise<Record<string, string>> {
  const plan = {
    relSegments: REL_CASES.map((c) => ({ name: c.name, ...('rel' in c ? { rel: c.rel } : {}), target: c.target, platform: c.platform, out: run(() => relSegments(c.rel, c.target, c.platform)) })),
    absoluteTarget: ABS_CASES.map((c) => ({ name: c.name, target: c.target, segs: c.segs, platform: c.platform, out: run(() => absoluteTarget(c.target, c.segs, c.platform)) })),
    checkRealContainment: CONTAIN_CASES.map((c) => {
      const io = {
        realpath: (p: string): string => {
          const v = c.links[p]
          if (v === undefined) throw new Error('ENOENT')
          return v
        }
      }
      return { name: c.name, root: c.root, abs: c.abs, links: c.links, mod: c.mod, out: run(() => checkRealContainment(c.root, c.abs, io, c.mod === 'posix' ? posix : win32)) }
    }),
    buildEditorLaunch: EDITOR_CASES.map((c) => ({
      name: c.name,
      target: c.target,
      paths: c.paths,
      editor: c.editor,
      exe: c.exe,
      platform: c.platform,
      out: run(() => launchOf(buildEditorLaunch(c.target, c.paths, c.editor, c.exe, c.platform)))
    })),
    buildTerminalLaunch: TERMINAL_CASES.map((c) => ({
      name: c.name,
      target: c.target,
      dir: c.dir,
      platform: c.platform,
      tools: c.tools,
      out: run(() => launchOf(buildTerminalLaunch(c.target, c.dir, c.platform, c.tools)))
    })),
    hasDotComponent: DOT_CASES.map((c) => ({ ...c, out: run(() => hasDotComponent(c.root, c.p)) })),
    clsidComponent: CLSID_CASES.map((c) => ({ ...c, out: run(() => clsidComponent(c.root, c.p)) })),
    buildRevealPlan: REVEAL_CASES.map((c) => ({ name: c.name, target: c.target, state: c.state, platform: c.platform, out: run(() => buildRevealPlan(c.target, c.state, c.platform)) }))
  }

  const opener: Record<string, unknown> = {
    scenarios: [] as unknown[],
    find: FIND_CASES.map((c) => {
      const f = fake(c)
      return { ...c, out: { vscode: findEditorExe('vscode', f.deps), cursor: findEditorExe('cursor', f.deps) } }
    }),
    chosen: CHOSEN_CASES.map((c) => {
      const f = fake({ platform: c.platform, files: c.files ?? [] })
      return { ...c, out: run(() => checkChosenExe(c.p, c.platform, c.files ? f.deps : undefined)) }
    }),
    tools: TOOLS_CASES.map((c: ExeCase) => ({ ...c, out: toolsJson(terminalTools(fake(c).deps)) })),
    // 설정 화면에서 에디터 실행 파일을 해석하는 경로(resolveEditorExe)의 직접 사례
    resolve: [
      { name: 'JetBrains 는 파일을 먼저 고른다', editor: 'jetbrains' as const, ui: {}, platform: 'linux' as const, files: ['/opt/idea/bin/idea.sh'] },
      { name: '사용자 지정 파일', editor: 'custom' as const, ui: { editorExePath: '/opt/idea/bin/idea.sh' }, platform: 'linux' as const, files: ['/opt/idea/bin/idea.sh'] },
      { name: '사용자 지정 파일이 없다', editor: 'custom' as const, ui: { editorExePath: '/opt/gone' }, platform: 'linux' as const, files: ['/opt/idea/bin/idea.sh'] },
      { name: '사용자 지정 빈 경로', editor: 'custom' as const, ui: { editorExePath: '' }, platform: 'linux' as const, files: [] }
    ].map((c) => {
      const f = fake({ platform: c.platform, files: c.files })
      return { ...c, out: run(() => resolveEditorExe(c.editor, c.ui, f.deps)) }
    })
  }
  for (const s of SCENARIOS) {
    const f = fake(s)
    const o = new ExternalOpener(f.deps)
    const steps: unknown[] = []
    for (const st of s.steps) {
      const before = { spawned: f.spawned.length, shown: f.shown.length }
      let out: Out
      try {
        await o.open(st.target, st.action, st.rel, st.ui ?? {})
        out = { ok: { spawned: f.spawned.slice(before.spawned), shown: f.shown.slice(before.shown) } }
      } catch (e) {
        out = { err: (e as Error).message }
        // 거부는 실행·표시 전이어야 한다
        expect(f.spawned.length, `${s.name}: 오류인데 실행했다`).toBe(before.spawned)
        expect(f.shown.length, `${s.name}: 오류인데 표시했다`).toBe(before.shown)
      }
      steps.push({ ...st, out })
    }
    ;(opener['scenarios'] as unknown[]).push({ name: s.name, platform: s.platform, files: s.files ?? [], dirs: s.dirs ?? [], links: s.links ?? {}, env: s.env ?? {}, steps })
  }

  const link = { cases: LINK_CASES.map((c) => ({ name: c.name, raw: c.raw, out: allowedLinkUrl(c.raw) })) }
  return { 'plan.json': render(plan), 'opener.json': render(opener), 'link.json': render(link) }
}

// ── 대응표 ────────────────────────────────────────────────────────────────────

/** 이식 대상 TS 테스트 4개(외부 열기). 대응표의 모든 `it` 이 가리키는 Rust 테스트가 실제로 있는지 본다 */
const TARGET_TESTS = ['externalPlan', 'externalOpener', 'externalReveal', 'externalClsid']

/** 대응표 값의 `<파일>::<이름>` 이 가리키는 Rust 소스 */
const RUST_SOURCES: Record<string, string> = {
  'external.rs': '../src-tauri/tests/external.rs',
  'plan.rs': '../src-tauri/src/external/plan.rs',
  'opener.rs': '../src-tauri/src/external/opener.rs',
  'paths.rs': '../src-tauri/src/external/paths.rs',
  'link.rs': '../src-tauri/src/external/link.rs'
}

describe('외부 열기 골든(TS 기준)', () => {
  it('대상 테스트 4개의 모든 it 이 대응표(test-map.json)에 있고, 가리키는 Rust 테스트가 있다', () => {
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
      if (value.startsWith('N/A: ')) {
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(8)
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
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenExternal.test.ts 로 골든을 만든다').toEqual(Object.keys(built).sort())
    for (const [name, text] of Object.entries(built)) {
      expect(readFileSync(new URL(name, GOLDEN), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
