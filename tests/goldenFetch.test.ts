import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { AbortError, type GitExecutor } from '../src/main/executors/GitExecutor'
import { display, fetchConfirmOptions, formatItem, MAX_CONFIRM_ITEMS, quoteForDisplay } from '../src/main/ipc/confirmText'
import { itKeys } from './fixtures/itTitles'
import { bindHost, dialog, resolveHost } from '../src/sidecar/electronShim'
import type { FromSidecar, MessageBoxArg } from '../src/sidecar/protocol'
import {
  ALLOWED_KEYS,
  ALLOWED_VALUES,
  classifyFailure,
  FETCH_ARGS,
  FILE_TRANSPORT_MIN_GIT,
  fetchConfigArgs,
  fetchEnv,
  fetchRefspecOk,
  fingerprintOf,
  HTTP_USERINFO,
  inspectConfig,
  inspectEntries,
  isAllowedConfig,
  nextDelayMs,
  NT_NAMESPACE,
  parseConfigList,
  parseGitVersion,
  parseScopedConfigList,
  readConfigEntries,
  REMOTE_URL_KEY,
  resolveSshCommand,
  runFetch,
  SUBMODULE_URL_KEY,
  unsafeRemoteUrl,
  versionAtLeast,
  WORKTREE_CONFIG_ABSENT
} from '../src/main/git/fetch'
import {
  ALLOWED_PAIRS,
  BACKOFF_CASES,
  CLASSIFY_CASES,
  CONFIG_LIST_OUT,
  DISPLAY_CODEPOINTS,
  HELPER_URLS,
  OPTION_LIKE_URLS,
  REFSPEC_CASES,
  SAFE_URLS,
  SCOPED_LIST_OUT,
  SSH_CASES,
  SSH_EDGE_CASES,
  UNC_ALLOW,
  UNC_REJECT,
  UNKNOWN_KEYS,
  VALUE_CASES,
  VERSION_AT_LEAST_CASES,
  VERSION_PARSE_CASES
} from './fixtures/fetchGuardCases'

// fetch 가드(허용 목록·URL 판정·설정 파싱·확인 창 표시·오류 분류 …)의 순수 함수를 골든 JSON 으로 고정한다.
// Rust 이식(src-tauri/src/git/fetch, 검사: src-tauri/tests/fetch_guard.rs)이 같은 골든을 읽어 같은 결과를 내는지 본다.
// 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts` 로 다시 쓰고,
// special_table.rs·lowercase_table.rs 는 이어서 `node scripts/gen-fetch-special.mjs` 로 만든다(골든의 confirm.json·config.json 을 읽는다).
// 이 테스트는 git 을 실행하지 않는다(실행기는 가짜). 입력 표는 tests/fixtures/fetchGuardCases.ts 를 tests/fetch.test.ts 와 함께 쓴다.
//
// 골든이 기대는 것: 정규식·toLowerCase·\p{…} 결과는 Node 의 Unicode 판에 따라 달라진다(lists.json 의 runtime 에 적는다).
// 짝 없는 서로게이트는 JSON 으로 쓸 수 없어(Rust 문자열도 못 담는다) 확인 창 입력은 UTF-16 코드 단위 배열로 두고,
// 오류 문구를 UTF-16 으로 자르다 생긴 반쪽은 U+FFFD 로 바꿔 적는다(toWellFormed — Rust 의 from_utf16_lossy 와 같다).

const GOLDEN_DIR = join(__dirname, 'golden', 'fetch')
/** tests/fetch.test.ts 의 각 it 이 Rust 의 어느 테스트로 옮겨졌는지(또는 왜 N/A 인지) 적은 손글씨 표. 골든이 아니다 */
const TEST_MAP = 'test-map.json'
const UPDATE = process.env.UPDATE_GOLDEN === '1'

// ── 골든 직렬화 ───────────────────────────────────────────────────────────

/** 최상위 키마다 한 줄(배열은 항목마다 한 줄)로 쓴다. 키 순서는 삽입 순서 그대로(env 는 [키, 값] 배열로 순서를 보존한다) */
function render(file: Record<string, unknown>): string {
  const entries = Object.entries(file).map(([k, v]) => {
    let body: string
    if (Array.isArray(v) && v.length > 0 && typeof v[0] === 'object') {
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

/** 짝 없는 서로게이트를 U+FFFD 로(String.prototype.toWellFormed 와 같다 — tsconfig 의 lib 이 ES2023 이라 직접 쓴다) */
function toWellFormed(s: string): string {
  return s.replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '�')
}

function wellFormed<T>(v: T): T {
  if (typeof v === 'string') return toWellFormed(v) as T
  if (Array.isArray(v)) return v.map(wellFormed) as T
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, wellFormed(x)])) as T
  return v
}

const u16 = (s: string): number[] => Array.from({ length: s.length }, (_, i) => s.charCodeAt(i))
const bit = (b: boolean): '1' | '0' => (b ? '1' : '0')

// ── 닫힌 정규식 문법 ──────────────────────────────────────────────────────
// Rust 는 regex 크레이트 없이 ALLOWED_KEYS 의 소스 문자열을 작은 매처로 해석한다(src-tauri/src/git/fetch/pattern.rs).
// 그 매처가 아는 문법: `^` `$`(양 끝만), `\.`, `.+`, 리터럴 [A-Za-z0-9_-], `( 대안 | 대안 )` 와 `?` 가 붙은 그룹(중첩 없음).
// 문법 밖 토큰이 들어오면 여기서 실패한다 — 그 정규식은 Rust 에 새로 옮겨야 한다.

/** 문법에 맞으면 null, 아니면 사유 */
function grammarError(source: string, flags = ''): string | null {
  if (flags !== '') return `플래그 ${flags}`
  if (!source.startsWith('^') || !source.endsWith('$') || source.length < 2) return '^…$ 가 아님'
  const body = source.slice(1, -1)
  let i = 0
  let inGroup = false
  let sawAlt = false
  let groupLen = 0
  while (i < body.length) {
    const c = body[i]!
    if (c === '\\') {
      if (body[i + 1] !== '.') return `이스케이프 \\${body[i + 1] ?? ''}`
      i += 2
      groupLen++
    } else if (c === '.') {
      if (body[i + 1] !== '+') return '.+ 가 아닌 .'
      i += 2
      groupLen++
    } else if (c === '(') {
      if (inGroup) return '중첩 그룹'
      if (body[i + 1] === '?') return '(? 확장 그룹'
      inGroup = true
      sawAlt = false
      groupLen = 0
      i++
    } else if (c === ')') {
      if (!inGroup) return '짝 없는 )'
      if (groupLen === 0) return '빈 대안'
      inGroup = false
      i++
      if (body[i] === '?') {
        if (sawAlt) return '|가 든 그룹의 ?'
        i++
      }
    } else if (c === '|') {
      if (!inGroup) return '그룹 밖의 |'
      if (groupLen === 0) return '빈 대안'
      sawAlt = true
      groupLen = 0
      i++
    } else if (/[A-Za-z0-9_-]/.test(c)) {
      i++
      groupLen++
    } else {
      return `문법 밖 토큰 ${c}`
    }
  }
  return inGroup ? '닫히지 않은 그룹' : null
}

// ── 가짜 실행기 ───────────────────────────────────────────────────────────

interface Res {
  code: number
  stdout?: string
  stderr?: string
}

function fakeExec(handler: (args: string[]) => Res): GitExecutor {
  return {
    kind: 'local',
    run: async (_cwd: string, args: string[]) => {
      const r = handler(args)
      return { code: r.code, stdout: Buffer.from(r.stdout ?? '', 'utf8'), stderr: r.stderr ?? '' }
    },
    stream: () => {
      throw new Error('사용하지 않음')
    },
    dispose: async () => {}
  } as unknown as GitExecutor
}

const NOT_FOUND: Res = { code: 128, stderr: 'fatal: not found' }

interface InspectScenario {
  label: string
  /** `config --list --show-scope --includes -z` 의 결과 */
  show: Res
  /** show 가 --show-scope 를 모른다고 실패할 때의 범위별 읽기 결과(없으면 실패) */
  byScope?: Partial<Record<'system' | 'global' | 'local' | 'worktree', Res>>
  /** `git remote` 결과 */
  list: Res
  /** `git remote get-url --all -- <이름>` 결과(이름 → 결과) */
  urls?: Record<string, Res>
}

const SHOW_SCOPE_UNKNOWN: Res = { code: 129, stderr: "error: unknown option `show-scope'\nusage: git config [<options>]" }
const RUN_OK = (stdout: string): Res => ({ code: 0, stdout })

const INSPECT_SCENARIOS: InspectScenario[] = [
  {
    label: '평범한 저장소',
    show: RUN_OK('global\0user.name\nA\0local\0remote.origin.url\nhttps://h/r.git\0local\0core.bare\nfalse\0'),
    list: RUN_OK('origin\n'),
    urls: { origin: RUN_OK('https://h/r.git\n') }
  },
  {
    label: 'userinfo: 마지막 @ 까지 가리고 대소문자·경로의 @ 는 건드리지 않는다',
    show: RUN_OK('local\0remote.a.url\nhttps://h/r\0'),
    list: RUN_OK('a\nb\nc\nd\ne\nf\n'),
    urls: {
      a: RUN_OK('https://user:pw@h/r.git\n'),
      b: RUN_OK('HTTPS://u@@h/x\nhttp://a@b@c/p\n'),
      c: RUN_OK('https://h/a@b\nhttps://h?x=a@b\nhttps://h#a@b\n'),
      d: RUN_OK('ssh://u@h/r\nhttps://u:p@h:8080\n'),
      e: RUN_OK('hTtPs://名前:비번@호스트/x\n'),
      f: RUN_OK('https://\n@h/x\nhttps://@h\n')
    }
  },
  {
    label: '저장소 안의 위험한 설정과 시스템 설정은 구분한다',
    show: RUN_OK(
      'system\0core.sshcommand\nsys-ssh\0global\0core.sshcommand\nglobal-ssh\0global\0credential.helper\nstore\0local\0core.sshcommand\nevil\0local\0credential.helper\nx\0worktree\0http.proxy\np\0local\0core.bare\ntrue\0'
    ),
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '값 없는 키, 빈 sshcommand, 나중 값이 이긴다',
    show: RUN_OK('global\0core.sshcommand\0global\0core.sshcommand\nfirst\0system\0core.sshcommand\nsecond\0command\0core.sshcommand\nthird\0local\0user.name\0'),
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '지문: 제어 문자·따옴표·유니코드가 든 키와 값',
    show: RUN_OK(
      'local\0weird.key\n\b\f\u0001\u007f\u2028\u2029"\\/ 😀 한글\0local\0x.y\ta\0local\0ev\u0007il.\u200b\nv\0local\0lone.trail\n \ufeff\0'
    ),
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '설정의 remote·submodule URL 거부(UNC·NT 네임스페이스·도우미·옵션)',
    show: RUN_OK(
      [
        'local\0remote.a.url\n\\\\host\\share\\x.git',
        'local\0remote.b.url\n\\??\\UNC\\host\\share\\x.git',
        'local\0remote.c.pushurl\next::sh -c x',
        'local\0submodule.lib.url\n//h/s/lib',
        'local\0remote.d.url\n-oProxyCommand=x',
        'local\0remote.e.url\nssh://u@-host/x',
        'local\0remote.f.url\nfile:///C:/ok.git',
        'local\0remote.g.url',
        'local\0submodule.h.url',
        'local\0remote.i.pushurl\nhttps://ok/x',
        'local\0remote.\n.url\nx'
      ].join('\0') + '\0'
    ),
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '실제 URL: UNC·NT·NUL 없는 정상',
    show: RUN_OK('local\0user.name\nx\0'),
    list: RUN_OK('a\r\nb\nc\n\nd\n'),
    urls: {
      a: RUN_OK('\\\\h\\s\\x.git\r\n'),
      b: RUN_OK('\\??\\UNC\\h\\s\n  \ufeff\\??\\x\nhttps://ok\n'),
      c: RUN_OK('https://ok/c.git\nhttps://ok/c2.git\n'),
      d: RUN_OK('\n\n')
    }
  },
  {
    label: 'git remote 실패: 사유에 stderr 앞 120 UTF-16 단위(짝 쪼갬 포함)',
    show: RUN_OK('local\0user.name\nx\0'),
    list: { code: 1, stderr: `  \ufeff${'a'.repeat(119)}😀 뒤에 더 \n` },
    urls: {}
  },
  {
    label: 'git remote 실패: 짧은 stderr',
    show: RUN_OK('local\0user.name\nx\0'),
    list: { code: 128, stderr: 'fatal: not a git repository\r\n' },
    urls: {}
  },
  {
    label: 'get-url 실패',
    show: RUN_OK('local\0user.name\nx\0'),
    list: RUN_OK('ok\nbad\n'),
    urls: { ok: RUN_OK('https://h/r\n'), bad: { code: 2, stderr: 'fatal: No such remote' } }
  },
  {
    label: '설정을 읽지 못하면 던진다(stderr 앞뒤 공백·BOM 은 JS trim)',
    show: { code: 128, stderr: '\ufeff\t fatal: bad config line 1 in file .git/config \u3000\n' },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '--show-scope 를 모르는 옛 git: 범위별 읽기(worktree 꺼짐)',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: { system: NOT_FOUND, global: RUN_OK('core.sshcommand\nuser-ssh\0'), local: RUN_OK('remote.o.url\nhttps://h/r\0user.name\nx\0extensions.worktreeconfig\nfalse\0') },
    list: RUN_OK('o\n'),
    urls: { o: RUN_OK('https://h/r\n') }
  },
  {
    label: '옛 git: worktree 설정이 켜져 있고 config.worktree 가 없으면 비어 있는 것으로 본다',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: {
      local: RUN_OK('extensions.worktreeconfig\0'),
      worktree: { code: 1, stderr: "fatal: unable to read config file '/repo/.git/config.worktree': No such file or directory" }
    },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '옛 git: --worktree 를 모르면 비어 있는 것으로 본다',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: { local: RUN_OK('extensions.worktreeconfig\nTRUE\0'), worktree: { code: 129, stderr: "error: unknown option `worktree'" } },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '옛 git: worktree 읽기가 다른 이유로 실패하면 던진다',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: { local: RUN_OK('extensions.worktreeconfig\nyes\0'), worktree: { code: 128, stderr: 'fatal: bad config line 3 in file .git/config.worktree\n' } },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '옛 git: worktree 설정이 켜진 값(위험 항목이 worktree 범위에 있다)',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: { local: RUN_OK('extensions.worktreeconfig\non\0'), worktree: RUN_OK('core.editor\nvi\0') },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '옛 git: local 을 읽지 못하면 던진다',
    show: SHOW_SCOPE_UNKNOWN,
    byScope: { system: RUN_OK(''), global: RUN_OK(''), local: { code: 128, stderr: 'fatal: --local can only be used inside a git repository\n' } },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: '--show-scope 와 무관한 실패는 범위별 읽기로 넘어가지 않는다',
    show: { code: 129, stderr: "error: unknown option `foo'\nusage: show-scope" },
    byScope: { local: RUN_OK('') },
    list: RUN_OK(''),
    urls: {}
  },
  {
    label: 'unknown option 과 show-scope 가 다른 줄이면 폴백하지 않는다',
    show: { code: 129, stderr: 'unknown option\nshow-scope' },
    byScope: { local: RUN_OK('') },
    list: RUN_OK(''),
    urls: {}
  }
]

/** 정해진 응답으로 inspectConfig 를 돌려 결과(또는 던진 메시지)와 설정을 읽은 순서를 적는다 */
async function runInspect(sc: InspectScenario): Promise<Record<string, unknown>> {
  const calls: string[] = []
  const exec = fakeExec((args) => {
    if (args[0] === 'config') {
      if (args.includes('--show-scope')) {
        calls.push('show-scope')
        return sc.show
      }
      const scope = args[1]!.slice(2) as 'system' | 'global' | 'local' | 'worktree'
      calls.push(scope)
      return sc.byScope?.[scope] ?? NOT_FOUND
    }
    if (args[1] === 'get-url') return sc.urls?.[args[4]!] ?? NOT_FOUND
    return sc.list
  })
  try {
    return { calls, inspection: wellFormed(await inspectConfig(exec, '.')) }
  } catch (e) {
    return { calls, error: wellFormed((e as Error).message) }
  }
}

/** show-scope 한 번의 결과만으로 readConfigEntries 가 하는 선택(파싱 / 범위별 폴백 / 던짐) */
async function showScopeOutcome(show: Res): Promise<Record<string, unknown>> {
  const calls: string[] = []
  const exec = fakeExec((args) => {
    if (args.includes('--show-scope')) return show
    calls.push(args[1]!.slice(2))
    return RUN_OK('')
  })
  try {
    await readConfigEntries(exec, '.')
    return { outcome: calls.length > 0 ? 'fallback' : 'parsed' }
  } catch (e) {
    return { outcome: 'error', message: wellFormed((e as Error).message) }
  }
}

/** extensions.worktreeconfig 값 하나로 범위별 읽기가 worktree 까지 가는가 */
async function worktreeReadFor(value: string | null): Promise<boolean> {
  let asked = false
  const exec = fakeExec((args) => {
    if (args.includes('--show-scope')) return SHOW_SCOPE_UNKNOWN
    if (args[1] === '--local') return RUN_OK(value === null ? 'extensions.worktreeconfig\0' : `extensions.worktreeconfig\n${value}\0`)
    if (args[1] === '--worktree') {
      asked = true
      return RUN_OK('')
    }
    return RUN_OK('')
  })
  await readConfigEntries(exec, '.')
  return asked
}

/** 범위별 읽기에서 worktree 실패 stderr 가 "없음"으로 취급되는가(던지지 않는가) */
async function worktreeAbsentFor(stderr: string): Promise<boolean> {
  const exec = fakeExec((args) => {
    if (args.includes('--show-scope')) return SHOW_SCOPE_UNKNOWN
    if (args[1] === '--local') return RUN_OK('extensions.worktreeconfig\ntrue\0')
    if (args[1] === '--worktree') return { code: 1, stderr }
    return RUN_OK('')
  })
  try {
    await readConfigEntries(exec, '.')
    return true
  } catch {
    return false
  }
}

// ── 골든 만들기 ───────────────────────────────────────────────────────────

const PREFIXES = ['core.', 'remote.', 'branch.', 'user.', 'init.', 'pull.', 'push.', 'fetch.', 'lfs.', 'extensions.', 'submodule.', 'include.', 'includeif.']
const MIDDLES: ({ text: string } | { repeat: string; count: number })[] = [
  { text: '' },
  { text: 'a' },
  { text: 'a.b' },
  { text: '\r' },
  { text: '\n' },
  { text: '\u2028' },
  { text: '\u2029' },
  { text: '\ufeff' },
  { text: 'İ' },
  { text: '\u212a' },
  { text: 'ſ' },
  { text: '．' },
  { text: '😀' },
  { text: '\0' },
  { repeat: 'x', count: 4096 }
]
const middleText = (m: (typeof MIDDLES)[number]): string => ('text' in m ? m.text : m.repeat.repeat(m.count))

/** ALLOWED_KEYS 소스에서 잎 이름들(`(a|b)$` 의 대안 또는 마지막 마디)을 뽑는다 */
function leavesOf(sources: string[]): string[] {
  const out: string[] = []
  for (const src of sources) {
    const body = src.slice(0, -1)
    const g = /\(([a-z|-]+)\)$/.exec(body)
    out.push(...(g ? g[1]!.split('|') : [body.slice(body.lastIndexOf('\\.') + 2)]))
  }
  return out
}

const LEAF_VARIANTS: [string, (l: string) => string][] = [
  ['그대로', (l) => l],
  ['끝 공백', (l) => `${l} `],
  ['끝 줄바꿈', (l) => `${l}\n`],
  ['끝 CRLF', (l) => `${l}\r\n`],
  ['한 글자 추가', (l) => `${l}x`],
  ['앞 글자 추가', (l) => `x${l}`],
  ['대문자', (l) => l.toUpperCase()]
]

function allowedGolden(): Record<string, unknown> {
  expect(PREFIXES).toHaveLength(ALLOWED_KEYS.length)
  const leaves = [...new Set([...leavesOf(ALLOWED_KEYS.map((r) => r.source)), 'bare', 'fsmonitor', 'fetch', 'sshcommand', 'prunetags', 'uploadpack', 'helper'])]
  const suffixes = [...new Set(leaves.flatMap((l) => LEAF_VARIANTS.map(([, f]) => f(l))))]
  // verdicts[접두][가운데][형태(0: 접두+가운데+suffix, 1: 접두+가운데+'.'+suffix)] = suffix 마다 '1'/'0'
  const verdicts = PREFIXES.map((p) => MIDDLES.map((m) => [0, 1].map((form) => suffixes.map((s) => bit(isAllowedConfig(`${p}${middleText(m)}${form ? '.' : ''}${s}`, 'x'))).join(''))))

  const cases = [
    ...UNKNOWN_KEYS.map((k): [string, string | null, string] => [k, 'x', bit(isAllowedConfig(k, 'x'))]),
    ...ALLOWED_PAIRS.map(([k, v]): [string, string | null, string] => [k, v, bit(isAllowedConfig(k, v))]),
    ...VALUE_CASES.map(([k, v]): [string, string | null, string] => [k, v, bit(isAllowedConfig(k, v))]),
    ...REFSPEC_CASES.map(([k, v]): [string, string | null, string] => [k, v, bit(isAllowedConfig(k, v))]),
    // 값이 null 인 키: 값 없는 키는 허용 목록 키여도 통과(값을 보지 않는 키)하되 값 검사 키·refspec 은 거부
    ...['user.name', 'core.bare', 'core.fsmonitor', 'remote.origin.fetch', 'remote.origin.url', 'filter.lfs.required'].map((k): [string, string | null, string] => [k, null, bit(isAllowedConfig(k, null))])
  ]

  // 값까지 보는 키: 허용 값과 그 근접 오답
  const allValues = [...new Set(Object.values(ALLOWED_VALUES).flat())]
  const valueCases: [string, string | null, string][] = []
  for (const key of [...Object.keys(ALLOWED_VALUES), 'filter.lfs.other', 'core.hookspath']) {
    const vals = [...allValues, ...allValues.flatMap((v) => [`${v} `, ` ${v}`, `${v}\n`, v.toUpperCase(), v.slice(0, -1), `${v}x`, `${v}\r\n`]), '', ' ', 'false', 'True', '0', '1']
    for (const v of new Set(vals)) valueCases.push([key, v, bit(isAllowedConfig(key, v))])
    valueCases.push([key, null, bit(isAllowedConfig(key, null))])
  }

  // remote.<n>.fetch refspec: 이름·소문자화로 길이가 바뀌는 문자·\s 가 아닌 U+0085·BOM
  const names = ['origin', 'my.fork', 'İ', 'İ'.toLowerCase(), 'ſ', '\u212a', '😀']
  const srcs = ['*', 'main', 'a/b', 'x y', 'x*', '', 'a..b', 'a\u00a0b', 'a\ufeffb', 'a\u0085b', 'a~b', 'a^b', 'a?b', 'a[b', 'a\\b', '😀', 'İ']
  const refspec: [string, string | null, string][] = []
  for (const name of names) {
    const key = `remote.${name}.fetch`
    for (const plus of ['', '+']) {
      for (const src of srcs) {
        for (const rest of [src, '*']) {
          const forms = [`refs/remotes/${name}/${rest}`, `refs/remotes/${name}/${rest}`.toUpperCase(), `refs/remotes/${name.toLowerCase()}/${rest}`, `refs/heads/${rest}`]
          for (const dst of forms) refspec.push([key, `${plus}refs/heads/${src}:${dst}`, bit(fetchRefspecOk(key, `${plus}refs/heads/${src}:${dst}`))])
        }
      }
    }
  }
  for (const [key, v] of [
    ['remote.o.fetch', null],
    ['remote.o.fetch', ''],
    ['remote.o.fetch', ':'],
    ['remote.o.fetch', '+:'],
    ['remote.o.fetch', '+refs/heads/a'],
    ['remote.o.fetch', '++refs/heads/a:refs/remotes/o/a'],
    ['remote.o.fetch', '+refs/heads/a:refs/remotes/o/a:'],
    ['remote.o.fetch', '+refs/heads/a:refs/remotes/o/a\n'],
    ['remote.o.fetch', 'refs/heads/a:refs/remotes/o/a '],
    ['remote.o.fetch', 'refs/heads/\ud83d\ude00:refs/remotes/o/\ud83d\ude00'],
    ['remote.o.fetch', 'refs/heads/*:refs/remotes/o/*'],
    ['remote..fetch', 'refs/heads/*:refs/remotes//*'],
    ['remote.\n.fetch', 'refs/heads/*:refs/remotes/\n/*'],
    ['remote.o.fetch\n', 'refs/heads/*:refs/remotes/o/*'],
    ['remote.o.fetchx', 'refs/heads/*:refs/remotes/o/*'],
    ['REMOTE.o.fetch', 'refs/heads/*:refs/remotes/o/*'],
    ['remote.O.fetch', 'refs/heads/*:refs/remotes/o/*'],
    ['remote.O.fetch', 'refs/heads/*:refs/remotes/O/*']
  ] as [string, string | null][]) {
    refspec.push([key, v, bit(isAllowedConfig(key, v))])
  }
  return {
    axes: { prefixes: PREFIXES, middles: MIDDLES, suffixes, forms: ['접두+가운데+suffix', '접두+가운데+"."+suffix'], note: 'verdicts[접두][가운데][형태] = suffix 마다 1(허용)/0, 값은 "x"' },
    verdicts: Object.fromEntries(PREFIXES.map((p, i) => [p, verdicts[i]])),
    cases,
    valueCases,
    refspec
  }
}

function urlsGolden(): Record<string, unknown> {
  const cases: [string, string | null][] = []
  const add = (u: string): void => void cases.push([u, unsafeRemoteUrl(u)])
  for (const [u] of HELPER_URLS) add(u)
  for (const u of [...OPTION_LIKE_URLS, ...SAFE_URLS, ...UNC_REJECT, ...UNC_ALLOW]) add(u)
  // 앞뒤 공백: JS trim 은 U+FEFF·U+00A0·줄 구분자를 지우고 U+0085·U+180E 는 지우지 않는다
  const pads = ['', ' ', '\t', '\ufeff', '\u00a0', '\u2028', '\u3000', '\u0085', '\u180e', '\ufeff\ufeff ']
  const bodies = [
    '\\\\h\\s',
    '//h/s',
    '/\\h/s',
    '\\??\\UNC\\h\\s',
    '/??/UNC/h/s',
    '\\??/x',
    '/??\\x',
    'a\\??\\b',
    'a/??/b',
    '??',
    '\\?\\x',
    '\\\\?\\UNC\\h',
    'file://h/s',
    'file:///??/UNC/h',
    'file:////h/s',
    'file:///C:/x',
    'file:/x',
    'file:\n//h/s',
    'file://\nh',
    'file:\\\\h\\s',
    'file:/\\h',
    'file:\\/h',
    'file:///\\h',
    'FILE://h/s',
    'FiLe:////h',
    'ﬁle://h/s',
    'file:',
    'file:/',
    'file://',
    'file:///',
    'file:////',
    'file:\r\n',
    '-x',
    '-',
    'ext::x',
    'EXT::x',
    'a.b+c-d::x',
    'Ab1::x',
    '::x',
    'x::',
    'a:b::c',
    'a_b::c',
    'ſsh::x',
    'ssh://-x',
    'ssh://u@-x',
    'ssh://u@h@-x',
    'ssh://a/@-x',
    'ssh://u:p@-h/r',
    'ssh://[::1]/r',
    'ssh://-',
    'SSH://U@-h',
    'git+ssh://-h',
    '1ssh://-h',
    'ssh:/-h',
    'ssh://\n@-h',
    'ssh://u@\n-h',
    'ssh://@-h',
    'ssh://u@h/-x',
    'https://u:p@h/',
    'https://ok/x',
    'git@h:a/b.git',
    '../r.git',
    'C:\\r.git',
    '',
    ' '
  ]
  for (const pad of pads) for (const body of bodies) for (const tail of ['', ' ', '\ufeff']) add(`${pad}${body}${tail}`)
  return { cases }
}

let lowercaseMapCache: [number, string][] | null = null

/** toLowerCase 가 코드 포인트 하나를 바꾸는 전부(Node 의 Unicode 판 기준). 문맥이 필요한 최종 시그마는 따로 본다 */
function lowercaseMap(): [number, string][] {
  if (lowercaseMapCache) return lowercaseMapCache
  const map: [number, string][] = []
  for (let cp = 0; cp <= 0x10ffff; cp++) {
    if (cp >= 0xd800 && cp <= 0xdfff) continue
    const ch = String.fromCodePoint(cp)
    const lower = ch.toLowerCase()
    if (lower !== ch) map.push([cp, lower])
  }
  lowercaseMapCache = map
  return map
}

/** 정규식(속성 이스케이프)이 맞추는 코드 포인트의 닫힌 구간 목록(서로게이트는 제외, 오름차순) */
function rangesOf(re: RegExp): [number, number][] {
  const out: [number, number][] = []
  let lo = -1
  for (let cp = 0; cp <= 0x110000; cp++) {
    const hit = cp <= 0x10ffff && !(cp >= 0xd800 && cp <= 0xdfff) && re.test(String.fromCodePoint(cp))
    if (hit && lo < 0) lo = cp
    if (!hit && lo >= 0) {
      out.push([lo, cp - 1])
      lo = -1
    }
  }
  return out
}

function configGolden(): Record<string, unknown> {
  const outs = [
    CONFIG_LIST_OUT,
    '',
    '\0',
    'local',
    'local\0',
    'local\0a.b',
    'local\0a.b\nv\0',
    'local\0A.B.C\nv\0global\0İ.İ\nİ\0system\0ΑΣ.ΑΣ\nΑΣ\0',
    'local\0x.y\n\0local\0x.z\n\n\n\0',
    'local\0K.ſ\n\u212a\0local\0Ǆ.ǅ\n\0',
    '\0\0\0\0',
    'a\0b\nc\0d\0e\nf\nlast',
    'local\0\nvalue\0',
    'local\0.\n\0',
    'local\0remote.o.url\nu\r\n\0local\0k\nv\0'
  ]
  const scoped = [
    ['core.bare\0Core.SshCommand\nssh -i x\0remote.o.url\nu\nrl\0', 'local'],
    [SCOPED_LIST_OUT, 'worktree'],
    ['', 'global'],
    ['\0\0', 'system'],
    ['a\0\0b\nc\0', 'x\ny'],
    ['İ.İ\nİ\0ΑΣ\0', 'local'],
    ['nonul', 'local']
  ]
  return {
    // Rust 는 std 의 소문자화를 쓰지 않고 이 표(lowercase_table.rs 의 원본)만 쓴다. 최종 시그마(Σ)의 문맥은 cased·caseIgnorable 로 판정한다
    lowercaseMap: lowercaseMap(),
    cased: rangesOf(/^\p{Cased}$/u),
    caseIgnorable: rangesOf(/^\p{Case_Ignorable}$/u),
    list: outs.map((out) => ({ out, entries: parseConfigList(out) })),
    scoped: scoped.map(([out, scope]) => ({ out: out!, scope: scope!, entries: parseScopedConfigList(out!, scope!) })),
    // 키 소문자화(JS toLowerCase: 최종 시그마·İ·Kelvin 기호)
    lowercase: ['İ', 'ΑΣ', 'ΑΣ ΑΣ', 'Σ', 'ΑΣ.', 'AΣ\u0345', '\u212a', 'ſ', 'ǅ', 'ẞ', 'İİ', 'I', 'Straße', '😀A', 'ΣΑ', 'Α.Σ'].map((s) => ({ input: s, key: parseConfigList(`x\0${s}\n\0`)[0]!.key }))
  }
}

function inspectEntriesGolden(): Record<string, unknown> {
  const outs = [
    'global\0core.sshcommand\nssh -F x\0local\0remote.o.url\nhttps://h/r\0',
    'local\0core.sshcommand\nevil\0',
    'local\0remote.o.url\n\\\\h\\s\0local\0remote.p.url\nhttps://ok\0',
    'local\0remote.o.pushurl\n\\??\\UNC\\h\0',
    'local\0submodule.x.url\next::y\0local\0submodule.z.url\nhttps://ok\0',
    'worktree\0user.name\nx\0worktree\0evil.key\nv\0local\0evil.key\nv2\0',
    'global\0evil.key\nglobal-values-are-trusted\0command\0evil.key\ncli\0',
    'local\0remote.o.fetch\n+refs/heads/*:refs/remotes/o/*\0local\0remote.p.fetch\n+refs/heads/*:refs/heads/*\0local\0remote.q.fetch\0',
    'local\0core.bare\ntrue\0local\0core.bare\0local\0core.fsmonitor\nwatchman\0',
    'local\0remote.o.url\n\0local\0remote.o.pushurl\0',
    'local\0include.path\n../x\0local\0includeif.gitdir:~/w/.path\ny\0local\0include.evil\nz\0',
    'local\0Weird.KEY\nv\0local\0Ünï.CÖDE\nv\0',
    // Unicode 판 회귀: Rust std 는 아는데 Node(Unicode 16.0)는 모르는 대문자(U+A7CE→A7CF)를 이름에 쓴 refspec. TS 는 접두가 어긋나 "알 수 없는 설정"이어야 한다
    'local\0remote.꟎.fetch\n+refs/heads/*:refs/remotes/꟏/*\0',
    'local\0remote.꟎.fetch\n+refs/heads/*:refs/remotes/꟎/*\0',
    'local\0remote.꟏.fetch\n+refs/heads/*:refs/remotes/꟏/*\0',
    'local\0remote.\u{16ea0}.fetch\n+refs/heads/x:refs/remotes/\u{16ea0}/x\0'
  ]
  const items: { key: string; value: string; raw?: string }[][] = [
    [],
    [{ key: 'a.b', value: 'c' }],
    [{ key: 'a.b', value: 'c', raw: 'secret' }, { key: 'x.y', value: '\b\f\n\r\t\u0001\u001f\u007f\u2028\u2029"\\/ 😀' }],
    [{ key: 'ünï', value: '한글', raw: '' }]
  ]
  return {
    entries: outs.map((out) => ({ out, inspection: inspectEntries(parseConfigList(out)) })),
    fingerprints: items.map((dangerous) => ({ dangerous, sha: fingerprintOf(dangerous) }))
  }
}

async function inspectGolden(): Promise<Record<string, unknown>> {
  const out = []
  for (const sc of INSPECT_SCENARIOS) {
    out.push({ label: sc.label, show: sc.show, byScope: sc.byScope ?? null, list: sc.list, urls: sc.urls ?? {}, ...(await runInspect(sc)) })
  }
  return { scenarios: out, ...inspectEntriesGolden() }
}

/** 확인 창 입력(UTF-16 코드 단위) */
function confirmGolden(): Record<string, unknown> {
  const ch = (...c: number[]): string => String.fromCodePoint(...c)
  const quoteInputs: string[] = [
    '',
    'plain',
    'a\nb',
    `x${ch(0x202e)}y`,
    `x${ch(0x200b)}y${ch(0x200e)}z${ch(0x2066)}w${ch(0x2069)}`,
    `c1${ch(0x85, 0x9f)}`,
    `bom${ch(0xfeff)}`,
    'q"uote',
    `a${ch(0x00ad)}b${ch(0x061c)}c${ch(0x180e)}d${ch(0x7f)}`,
    String.fromCodePoint(0xe0041),
    '안녕 😀',
    "it's ok 안녕 😀",
    'say "hi"',
    'back\\slash',
    'tab\there',
    'a b',
    'a\u00a0b',
    `${ch(0x201d)} = ${ch(0x201c)}`,
    ...DISPLAY_CODEPOINTS.map(([, cp]) => `a${ch(cp)}b`)
  ]
  // 짝 없는 서로게이트는 코드 단위 배열로만 줄 수 있다
  const unitInputs: number[][] = [
    ...quoteInputs.map(u16),
    [0x78, 0xd800, 0x79],
    [0xdc00],
    [0xd83d],
    [0xd83d, 0x41],
    [0xde00, 0xd83d],
    [0xd83d, 0xde00, 0xd83d],
    [0xdbff, 0xdfff, 0xdc00, 0xd800]
  ]
  const quote = unitInputs.map((units) => ({ units, quote: quoteForDisplay(String.fromCharCode(...units)) }))

  // display: 길이 경계(80 이하는 그대로)와, 앞 48·뒤 32 단위 경계에 이모지가 걸리는 경우
  const emoji = '😀'
  const base = (len: number): string[] => Array.from({ length: len }, () => 'a')
  const withEmoji = (len: number, at: number): string => {
    const a = base(len)
    a.splice(at, 2, emoji)
    return a.join('')
  }
  const dispInputs: { units: number[]; max?: number }[] = []
  for (const len of [0, 1, 79, 80, 81, 82, 100, 200]) dispInputs.push({ units: u16('a'.repeat(len)) })
  for (const len of [80, 81, 82, 90, 120]) {
    for (const at of [46, 47, 48, 49]) dispInputs.push({ units: u16(withEmoji(len, at)) })
    for (const back of [34, 33, 32, 31]) dispInputs.push({ units: u16(withEmoji(len, len - back)) })
  }
  dispInputs.push({ units: u16(`${'가'.repeat(60)}\n숨김${'나'.repeat(60)}`) })
  dispInputs.push({ units: u16(`${'z'.repeat(60)}\u202e${'z'.repeat(60)}`) })
  dispInputs.push({ units: [...u16('a'.repeat(47)), 0xd83d, ...u16('b'.repeat(60))] }) // 짝 없는 서로게이트가 자르는 자리에
  for (const max of [0, 1, 2, 3, 5, 10]) {
    for (const s of ['', 'abcdefgh', '😀😀😀😀😀', 'a😀b😀c']) dispInputs.push({ units: u16(s), max })
  }
  const displays = dispInputs.map((d) => {
    const s = String.fromCharCode(...d.units)
    return { units: d.units, ...(d.max === undefined ? {} : { max: d.max }), display: d.max === undefined ? display(s) : display(s, d.max) }
  })

  const items = [
    ['core.sshcommand', 'ssh -i key'],
    ['a'.repeat(100), 'b'.repeat(100)],
    [`k${ch(0x201d)}`, `v${ch(0x201c)}`],
    ['한글.키', '값\n둘째 줄'],
    ['', '']
  ].map(([k, v]) => ({ key: u16(k!), value: u16(v!), formatted: formatItem(k!, v!) }))

  // SPECIAL 일치 범위: quoteForDisplay 가 \uXXXX 로 바꾸는 코드 포인트. 0..=0x10FFFF 전체를 훑는다
  const ranges: [number, number][] = []
  let start = -1
  for (let cp = 0; cp <= 0x110000; cp++) {
    const special = cp <= 0x10ffff && quoteForDisplay(String.fromCodePoint(cp)).startsWith('"\\u')
    if (special && start < 0) start = cp
    if (!special && start >= 0) {
      ranges.push([start, cp - 1])
      start = -1
    }
  }
  // 범위 경계 앞뒤: 표가 정확히 경계에서 바뀌는지
  const boundaries: [number, string][] = []
  for (const [a, b] of ranges) for (const cp of [a - 1, a, b, b + 1]) if (cp >= 0 && cp <= 0x10ffff) boundaries.push([cp, bit(quoteForDisplay(String.fromCodePoint(cp)).startsWith('"\\u'))])
  return { maxConfirmItems: MAX_CONFIRM_ITEMS, quote, display: displays, formatItem: items, special: ranges, boundaries }
}

async function miscGolden(): Promise<Record<string, unknown>> {
  const failureTexts = [
    ...CLASSIFY_CASES.map(([t]) => t),
    "fatal: transport 'ext' not allowed",
    "fatal: TRANSPORT 'file' NOT ALLOWED",
    "transport '\n' not allowed",
    "transport 'a'b' not allowed",
    "transport '' not allowed",
    'Repository not found.',
    'fatal: unable to access: The requested URL returned error: 403',
    'returned error: 404',
    'returned error: 4040',
    'returned error: 401 x',
    'remote: Invalid username or token. Password authentication is not supported',
    'remote: Bad credentials',
    'fatal: could not read Password for x',
    'could not read  Username',
    'COULD NOT READ USERNAME',
    'Host key verification failed.',
    'Permission denied',
    'access denied',
    'ACCESS DENIED',
    'Invalid credentials',
    'terminal prompts disabled',
    'publicKey',
    'permiſsion denied',
    'authentication\nfailed',
    'ſ authentication failed',
    '\u212authentication failed',
    'authentication failed \u212a',
    'accesſ denied',
    'tranſport \'file\' not allowed',
    '',
    '네트워크 오류'
  ]
  const classify = []
  for (const text of failureTexts) for (const github of [false, true]) classify.push({ text, github, kind: classifyFailure(new Error(text), { github }) })
  classify.push({ aborted: true, github: false, kind: classifyFailure(new AbortError()) })
  classify.push({ aborted: true, github: true, kind: classifyFailure(new AbortError(), { github: true }) })

  const delays = []
  for (const minutes of [0, 0.5, 1, 5, 15, 30, 60, 1440]) for (const failures of [0, 1, 2, 3, 4, 5, 9, 10, 11, 12, 20, 100]) delays.push([minutes, failures, nextDelayMs(minutes, failures)])
  for (const [m, f] of BACKOFF_CASES) delays.push([m, f, nextDelayMs(m, f)])

  const versionTexts = [
    ...VERSION_PARSE_CASES.map(([t]) => t),
    'git version 2.x git version 3.4.5',
    'xx git version 1.2',
    'git version 2.',
    'git version 2.3.',
    'git version 2.3.4.5',
    'git version 2.3.windows.1',
    'git version 02.003.0004',
    'git version 99999999999999999999.1.2',
    'git version 1.2.99999999999999999999999',
    'Git version 2.3.4',
    'git  version 2.3.4',
    'git version ٢.٣.٤',
    'git version 2.3\n.4',
    'git version 1.2 git version 3.4',
    '',
    'git version 2.45.1',
    'git version 2.45.0',
    'git version 2.46'
  ]
  const versions = versionTexts.map((text) => ({ text, version: parseGitVersion(text) }))
  const mins: [number, number, number][] = [FILE_TRANSPORT_MIN_GIT, [0, 0, 0], [2, 45, 1], [1e20, 0, 0]]
  const vs: ([number, number, number] | null)[] = [null, [0, 0, 0], [2, 45, 0], [2, 45, 1], [2, 45, 2], [2, 46, 0], [2, 44, 99], [3, 0, 0], [1, 99, 99], [1e20, 0, 0], [2, 1e20, 0]]
  const atLeast = []
  for (const min of mins) for (const v of vs) atLeast.push({ v, min, ok: versionAtLeast(v, min) })
  for (const [v, ok] of VERSION_AT_LEAST_CASES) atLeast.push({ v, min: FILE_TRANSPORT_MIN_GIT, ok })

  const cfgArgs = []
  for (const hooksPath of ['/dev/null', 'C:/Users/me/AppData/Roaming/antler/nohooks', 'D:/한글 경로/no hooks', '']) {
    for (const credentialHelpers of [undefined, false, true]) {
      for (const followRedirects of [undefined, 'initial', 'false'] as const) {
        const opts = { ...(credentialHelpers === undefined ? {} : { credentialHelpers }), ...(followRedirects === undefined ? {} : { followRedirects }) }
        cfgArgs.push({ hooksPath, opts, args: fetchConfigArgs(hooksPath, opts) })
      }
    }
  }

  const ssh = [...SSH_CASES, ...SSH_EDGE_CASES].map((c) => {
    const platform = c.platform ?? 'linux'
    return { userSshCommand: c.userSshCommand, kind: c.kind, env: c.env, platform, result: resolveSshCommand({ userSshCommand: c.userSshCommand }, c.kind, c.env, platform) }
  })

  const envs = []
  for (const sshCommand of ['ssh -o BatchMode=yes -o ConnectTimeout=15', "'/opt/my ssh/ssh'", '', 'ssh 한글']) {
    for (const allowFile of [false, true]) envs.push({ sshCommand, allowFile, env: Object.entries(fetchEnv({ sshCommand, allowFile })) })
  }

  // 사유 문구 자르기(UTF-16 120 단위)는 inspect 골든이 본다. 여기는 stderr 판정 정규식들
  const unknownShowScope = [
    "error: unknown option `show-scope'",
    'unknown option show-scope',
    'UNKNOWN OPTION SHOW-SCOPE',
    'unknown option\nshow-scope',
    'unknown option\rshow-scope',
    'unknown option\u2028show-scope',
    'unknown option\u2029show-scope',
    'unknown option\u0085show-scope',
    'show-scope unknown option',
    'unknown option--show-scope',
    'unknown optionshow-scope',
    'xx\nunknown option foo\nshow-scope\nunknown option bar show-scope',
    'unknown option unknown option show-scope',
    'unknown opti\u212aon show-scope',
    'unKnown option show-scope',
    '\u212a',
    'unknown option show-ſcope',
    ''
  ]
  const writeFetchHead = unknownShowScope.map((s) => s.replace(/show-scope/g, 'write-fetch-head'))
  const wtAbsent = [
    "unknown option `worktree'",
    "unknown option 'worktree'",
    'unknown option "worktree"',
    "unknown option  `worktree'",
    "UNKNOWN OPTION `worktree'",
    "unknown option `worktree",
    "fatal: unable to read config file '/r/.git/config.worktree': No such file or directory",
    "fatal: unable to read config file 'C:/r/.git/config.worktree': No such file or directory\n",
    "unable to read config file '': No such file or directory",
    "unable to read config file 'config.worktree': No such file or directory",
    "unable to read config file 'a\nb/config.worktree': No such file or directory",
    "unable to read config file 'a'b/config.worktree': No such file or directory",
    "unable to read config file '/r/.git/config.worktree': no such file or directory",
    "unable to read config file '/r/.git/config.worktree': Permission denied",
    "unable to read config file '/r/.git/xconfig.worktree': No such file or directory",
    "unable to read config file '/r/.git/config_worktree': No such file or directory",
    "unable to read config file 'x': unable to read config file '/r/config.worktree': No such file or directory",
    "unable to read config file 'a' config.worktree': No such file or directory",
    'fatal: bad config line'
  ]
  const worktreeValues = [null, '', ' ', 'true', 'TRUE', 'True', 'false', 'FALSE', ' false ', '\ufefffalse\ufeff', 'no', 'No', 'off', 'OFF', '0', '1', '00', 'yes', 'on', 'maybe', 'fals\u0065', 'ſalse', 'false\n', '\u2028off\u2029', '\u0085off', 'of f', '\u212a']
  return {
    classify,
    delays,
    versions,
    atLeast,
    fileTransportMinGit: FILE_TRANSPORT_MIN_GIT,
    fetchConfigArgs: cfgArgs,
    fetchArgs: FETCH_ARGS,
    ssh,
    fetchEnv: envs,
    unknownShowScope: await Promise.all(unknownShowScope.map(async (stderr) => ({ stderr, ...(await showScopeOutcome({ code: 129, stderr })) }))),
    unknownWriteFetchHead: await Promise.all(writeFetchHead.map(async (stderr) => ({ stderr, retries: await writeFetchHeadRetries(stderr) }))),
    worktreeConfigAbsent: await Promise.all(wtAbsent.map(async (stderr) => ({ stderr, regex: WORKTREE_CONFIG_ABSENT.test(stderr), absent: await worktreeAbsentFor(stderr) }))),
    worktreeOn: await Promise.all(worktreeValues.map(async (value) => ({ value, asked: await worktreeReadFor(value) })))
  }
}

/** --no-write-fetch-head 를 모르는 git 이면 한 번 더 한다(runFetch 의 판정). 호출 수로 본다 */
async function writeFetchHeadRetries(stderr: string): Promise<boolean> {
  let runs = 0
  const exec = fakeExec(() => {
    runs++
    return { code: 129, stderr }
  })
  try {
    await runFetch(exec, '.', { ...inspectEntries([]), remoteNames: ['o'] }, { hooksPath: '/dev/null', allowFile: false, credentialHelpers: false })
  } catch {
    // 실패는 정상(모든 호출이 실패하도록 꾸몄다)
  }
  return runs === 2
}

/**
 * 퍼즈 코퍼스. 결정적 LCG 로 만든 **입력 문자열 자체**를 골든에 적는다 — Rust 는 생성 규칙을 따라 하지 않고 적힌 문자열을 읽어 같은 판정을 하는지만 본다
 * (생성기까지 양쪽에 두면 구현 차이가 입력 차이로 가려진다). 알파벳은 JS 와 Rust 가 갈리기 쉬운 문자(줄 끝 문자·BOM·U+0085·İ·K·ſ·Σ·이모지)와
 * Node 의 소문자화 표 경계, **Node(Unicode 16.0)에는 대소문자 쌍이 없고 Rust std 에는 있는 대문자**를 섞는다.
 */
function fuzzGolden(): Record<string, unknown> {
  let seed = 0x2545f491
  const next = (): number => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
    return seed >>> 8
  }
  const pick = <T>(xs: readonly T[]): T => xs[next() % xs.length]!
  const cp = (...c: number[]): string => String.fromCodePoint(...c)
  const map = lowercaseMap()
  const boundary: string[] = []
  for (let i = 0; i < map.length; i += 37) {
    const c = map[i]![0]
    boundary.push(cp(c), cp(c + 1), map[i]![1])
  }
  const last = map[map.length - 1]![0]
  boundary.push(cp(map[0]![0] - 1), cp(map[0]![0]), cp(last), cp(last + 1))
  // Rust std 만 소문자 쌍을 아는 대문자(rustc 1.100 대 Node 22 의 Unicode 16.0)와 그 소문자 짝
  const rustOnly = [0xa7ce, 0xa7d2, 0xa7d4, 0xa7dd, 0xa7e2, 0xab6c, 0xab6d, 0x16ea0, 0x16eb8, 0x1df40, 0x1df48, 0x1df7e].flatMap((c) => [cp(c), cp(c + 1)])
  const alphabet: string[] = [
    ...['a', 'b', 'k', 's', 'x', 'A', 'K', 'S', 'I', 'i', '0', '.', '.', ':', '*', '+', '-', '/', '\\', '?', '@', '#', "'", '~', '^', '[', ' ', '\t'],
    ...['\n', '\r', '\u2028', '\u2029', '\ufeff', '\u0085', '\u00a0', '\u180e', '\u200b', '😀', '한', 'é', 'İ', 'ı', '\u212a', '\u212b', 'ſ', 'Σ', 'σ', 'ς', 'ΑΣ', '\u0345', 'ǅ', 'ẞ', 'ß', 'ﬁ'],
    ...['refs/heads/', 'refs/remotes/', 'remote.', '.url', '.fetch', '.pushurl', 'branch.', '.merge', 'submodule.', 'core.', 'user.name', 'file:', 'https://', 'http://', 'ssh://', 'ext::', '??', '\\??\\', '::', '..'],
    ...boundary,
    ...rustOnly
  ]
  const rstr = (maxLen: number): string => Array.from({ length: next() % (maxLen + 1) }, () => pick(alphabet)).join('')
  const nameLike = (): string => Array.from({ length: next() % 5 }, () => pick(next() % 3 === 0 ? alphabet : ['o', 'p', 'İ', 'Σ', ...rustOnly, ...boundary.slice(0, 40)])).join('')

  const config: [string, string | null, string, string, string][] = []
  const addConfig = (key: string, value: string | null): void => {
    const parsed = parseConfigList(value === null ? `local\0${key}\0` : `local\0${key}\n${value}\0`)[0]!
    config.push([key, value, bit(isAllowedConfig(key, value)), bit(fetchRefspecOk(key, value)), bit(isAllowedConfig(parsed.key, parsed.value))])
  }
  for (let n = 0; n < 2200; n++) {
    const shape = next() % 5
    if (shape === 0) {
      addConfig(rstr(14), next() % 6 === 0 ? null : rstr(14))
    } else if (shape <= 2) {
      // refspec: 이름·소문자화·접두가 맞거나 한 글자 어긋나는 것
      const name = nameLike()
      const dstName = pick([name, name, name.toLowerCase(), name.toUpperCase(), rstr(3)])
      const src = next() % 3 === 0 ? '*' : rstr(5)
      const dst = next() % 4 === 0 ? '*' : next() % 3 === 0 ? rstr(5) : src
      addConfig(`${pick(['remote.', 'remote.', 'Remote.'])}${name}.fetch`, `${pick(['', '+', '+'])}refs/heads/${src}:refs/remotes/${dstName}/${dst}`)
    } else if (shape === 3) {
      addConfig(`${pick(['remote', 'branch', 'submodule', 'REMOTE', 'includeIf', 'core', 'user', 'extensions'])}.${nameLike()}.${pick(['url', 'pushurl', 'merge', 'active', 'path', 'name', 'worktreeconfig', 'hookspath', 'URL'])}${pick(['', '', '', '\n', ' ', '\u2028'])}`, pick(['x', null, '', rstr(6)]))
    } else {
      addConfig(`${pick(['core', 'user', 'push', 'pull', 'filter.lfs', 'init', 'fetch', 'lfs'])}.${pick(['', pick(['bare', 'fsmonitor', 'name', 'default', 'clean', 'prune', 'ff', 'rebase', 'required']), rstr(5)])}`, pick(['true', 'false', 'git-lfs clean -- %f', rstr(8), null]))
    }
  }

  const urls: [string, string | null][] = []
  const heads = ['', '', ' ', '\ufeff', '\u2028', '\u00a0', '-', '--', '\\\\', '//', '/\\', 'file:', 'FILE:', 'fi\u212a:', 'https://', 'HTTPS://', 'ssh://', 'ext::', 'a+b::', 'A.B-c::', 'git@', 'C:\\', '../']
  for (let n = 0; n < 2200; n++) urls.push([`${pick(heads)}${next() % 3 === 0 ? pick(heads) : ''}${rstr(10)}${pick(['', '', ' ', '\ufeff', '@-', '/'])}`, null])
  for (const u of urls) u[1] = unsafeRemoteUrl(u[0])

  const lower: [string, string][] = []
  for (let n = 0; n < 2200; n++) {
    const text = rstr(9).replace(/\0/g, '')
    lower.push([text, parseConfigList(`x\0${text}\n\0`)[0]!.key])
  }
  return { generator: 'LCG(1664525, 1013904223) seed 0x2545f491 — Rust 는 이 생성 규칙을 쓰지 않고 아래 입력 문자열만 읽는다', config, urls, lower }
}

function listsGolden(): Record<string, unknown> {
  const rx = (r: RegExp): { source: string; flags: string } => ({ source: r.source, flags: r.flags })
  return {
    runtime: { unicode: process.versions.unicode },
    allowedKeys: ALLOWED_KEYS.map(rx),
    allowedValues: ALLOWED_VALUES,
    remoteUrlKey: rx(REMOTE_URL_KEY),
    submoduleUrlKey: rx(SUBMODULE_URL_KEY),
    // 아래 셋은 닫힌 문법 밖이라 Rust 가 손으로 구현했다 — 소스가 바뀌면 Rust 테스트가 실패한다(다시 옮겨야 한다)
    handWritten: { httpUserinfo: rx(HTTP_USERINFO), ntNamespace: rx(NT_NAMESPACE), worktreeConfigAbsent: rx(WORKTREE_CONFIG_ABSENT) }
  }
}

// ── 확인 창 인자 ──────────────────────────────────────────────────────────
// 사이드카 shim(src/sidecar/electronShim.ts)이 Rust 에 넘기는 MessageBoxArg 를 shim 을 실제로 불러 얻는다(변환을 여기서 다시 쓰지 않는다).
// Rust(confirm_text.rs 의 fetch_confirm_box)가 같은 인자를 만드는지 본다. options 는 shim 에 들어가기 전의 Electron 옵션(defaultId 포함)이다.

type BoxCase = { name: string; items: { key: string; value: string }[] }

function boxCases(): BoxCase[] {
  const item = (i: number): { key: string; value: string } => ({ key: `core.key${i}`, value: `v${i}` })
  const many = (n: number): { key: string; value: string }[] => Array.from({ length: n }, (_, i) => item(i))
  return [
    { name: 'repo', items: [{ key: 'core.sshcommand', value: 'ssh -F x' }] },
    { name: '내 저장소', items: [{ key: 'credential.helper', value: '!echo "hi"' }, { key: 'protocol.ext.allow', value: 'never' }] },
    { name: 'a\nb', items: [{ key: 'k‮e', value: 'v​w' }] },
    { name: 'x'.repeat(100), items: [{ key: 'trace2.eventtarget', value: `${'a'.repeat(47)}😀${'b'.repeat(80)}` }] },
    { name: 'e', items: [] },
    { name: 'twelve', items: many(MAX_CONFIRM_ITEMS) },
    { name: 'thirteen', items: many(MAX_CONFIRM_ITEMS + 1) },
    { name: 'fifty', items: many(50) },
    { name: 'q"uote', items: [{ key: 'a', value: `x${'”'} = ${'“'}safe` }, { key: '', value: '' }] }
  ]
}

/**
 * 분리 전 handlers.ts 의 confirmDangerousConfig 본문(`show` 에 넘기던 옵션과 반환 판정)을 그대로 옮긴 원본.
 * fetchConfirmOptions 로 뽑아낸 것이 이것과 같은 옵션을 만드는지 보는 데만 쓴다 — 고치지 않는다.
 */
async function legacyConfirmDangerousConfig(
  name: string,
  items: { key: string; value: string }[],
  show: (opts: unknown) => Promise<{ response: number }>
): Promise<boolean | 'always'> {
  const title = `${display(name)} 저장소의 설정에 fetch 때 외부 프로그램을 실행하거나 연결을 가로챌 수 있는 항목이 있습니다.`
  if (items.length > MAX_CONFIRM_ITEMS) {
    await show({
      type: 'warning',
      buttons: ['닫기'],
      defaultId: 0,
      cancelId: 0,
      title: 'fetch 차단',
      message: title,
      detail: `항목이 ${items.length}개로 너무 많아 모두 확인할 수 없어 fetch 하지 않습니다. 저장소의 .git/config 를 직접 확인하세요.\n\n${items
        .slice(0, MAX_CONFIRM_ITEMS)
        .map((i) => formatItem(i.key, i.value))
        .join('\n')}\n… 외 ${items.length - MAX_CONFIRM_ITEMS}개`
    })
    return false
  }
  const res = await show({
    type: 'warning',
    buttons: ['이번만 fetch', '이 저장소에서 항상 허용', '취소'],
    defaultId: 2,
    cancelId: 2,
    title: 'fetch 확인',
    message: title,
    detail: `${items.map((i) => formatItem(i.key, i.value)).join('\n')}\n\n직접 만든 설정이거나 신뢰하는 저장소일 때만 허용하세요. "이번만"은 이번 실행 동안, "항상"은 앱을 다시 켜도 적용되며, 어느 쪽이든 위 키·값이 바뀌면 다시 묻습니다. 값이 가리키는 스크립트·파일의 내용이 바뀌는 것은 확인하지 않습니다.`
  })
  return res.response === 0 ? true : res.response === 1 ? 'always' : false
}

async function messageBoxArg(options: unknown): Promise<MessageBoxArg> {
  let sent: FromSidecar | null = null
  bindHost((m) => {
    sent = m
  })
  const pending = dialog.showMessageBox(options)
  const msg = sent as FromSidecar | null
  if (!msg || msg.t !== 'host' || msg.op !== 'messageBox') throw new Error('shim 이 messageBox 를 보내지 않았다')
  resolveHost(msg.id, true, { response: 0 })
  await pending
  return msg.arg as MessageBoxArg
}

async function confirmBoxGolden(): Promise<Record<string, unknown>> {
  const cases = await Promise.all(
    boxCases().map(async (c) => {
      const { tooMany, options } = fetchConfirmOptions(c.name, c.items)
      return { name: c.name, items: c.items, tooMany, options, arg: await messageBoxArg(options) }
    })
  )
  return { cases }
}

async function buildAll(): Promise<Record<string, string>> {
  return {
    'confirmBox.json': render(await confirmBoxGolden()),
    'lists.json': render(listsGolden()),
    'allowed.json': render(allowedGolden()),
    'urls.json': render(urlsGolden()),
    'config.json': render(configGolden()),
    'inspect.json': render(await inspectGolden()),
    'confirm.json': render(confirmGolden()),
    'misc.json': render(await miscGolden()),
    'fuzz.json': render(fuzzGolden())
  }
}

describe('fetch 가드 골든', () => {
  it('허용 목록 정규식은 Rust 매처가 아는 닫힌 문법 안에 있다', () => {
    for (const re of [...ALLOWED_KEYS, REMOTE_URL_KEY, SUBMODULE_URL_KEY]) expect(grammarError(re.source, re.flags), re.source).toBeNull()
  })

  it('문법 검사는 문법 밖 토큰을 잡는다', () => {
    for (const bad of ['^a\\d$', '^a[bc]$', '^a*$', '^a.$', '^a.*$', '^a|b$', '^(a|b$', '^a)$', '^(?:a)$', '^((a))$', '^(a|b)?$', '^(|a)$', '^\\w$', '^a{2}$', '^a b$', 'a$', '^a', '^é$', '^a(b)(?=c)$']) {
      expect(grammarError(bad), bad).not.toBeNull()
    }
    expect(grammarError('^a$', 'i')).not.toBeNull()
    for (const good of ['^a\\.b$', '^a\\..+\\.(b|c-d)$', '^a\\.(.+\\.)?(b|c)$', '^(a)?$']) expect(grammarError(good), good).toBeNull()
  })

  it('fetchConfirmOptions 는 분리 전 handlers.ts 의 인라인 옵션과 같다(동작 불변)', async () => {
    for (const c of boxCases()) {
      const shown: unknown[] = []
      const answer = await legacyConfirmDangerousConfig(c.name, c.items, async (opts) => {
        shown.push(opts)
        return { response: 1 }
      })
      const { tooMany, options } = fetchConfirmOptions(c.name, c.items)
      expect(shown, c.name).toEqual([options])
      // 호출부(handlers.ts)의 판정: tooMany 면 어떤 응답이어도 false, 아니면 0 → true, 1 → 'always', 그 밖 → false
      expect(answer, c.name).toBe(tooMany ? false : 'always')
    }
  })

  it('MessageBoxArg 의 본문은 message·detail 을 빈 줄로 잇고 취소를 기본 선택으로 둔다', async () => {
    for (const c of boxCases()) {
      const { options } = fetchConfirmOptions(c.name, c.items)
      const arg = await messageBoxArg(options)
      expect(arg.message).toBe([options.message, options.detail].filter(Boolean).join('\n\n'))
      expect(options.defaultId).toBe(options.cancelId)
      expect(arg.cancelId).toBe(options.cancelId)
    }
  })

  it('fetch.test.ts 이식 대응표(test-map.json)가 원본의 모든 it 을 덮고 가리키는 Rust 테스트가 있다', () => {
    const map = JSON.parse(readFileSync(join(GOLDEN_DIR, TEST_MAP), 'utf8')) as Record<string, string>
    const keys = itKeys(readFileSync(join(__dirname, 'fetch.test.ts'), 'utf8'))
    expect(new Set(keys).size, 'describe > it 제목이 겹치면 대응표 키가 모호해진다').toBe(keys.length)
    expect(keys.filter((k) => !(k in map)), '대응표에 없는 it (fetch.test.ts 에 테스트를 더했으면 test-map.json 에도 적는다)').toEqual([])
    expect(Object.keys(map).filter((k) => !keys.includes(k)), '원본에 없는 대응표 항목').toEqual([])
    const rustSources = Object.fromEntries(
      ['fetch.rs', 'fetch_guard.rs', 'exec.rs'].map((f) => [f, readFileSync(join(__dirname, '..', 'src-tauri', 'tests', f), 'utf8')])
    )
    for (const [title, value] of Object.entries(map)) {
      if (value.startsWith('N/A: ')) {
        expect(value.length, `${title}: N/A 사유가 비어 있다`).toBeGreaterThan(8)
        continue
      }
      const m = /^(fetch\.rs|fetch_guard\.rs|exec\.rs)::(\S+)/.exec(value)
      expect(m, `${title}: 'N/A: 사유' 또는 '<파일>.rs::<테스트 이름>' 이어야 한다 → ${value}`).not.toBeNull()
      const [, file, name] = m!
      expect(new RegExp(String.raw`\bfn ${name}\(`).test(rustSources[file!]!), `${title}: ${file} 에 fn ${name} 이 없다`).toBe(true)
    }
  })

  it('골든 파일이 TS 의 현재 출력과 같다', async () => {
    const built = await buildAll()
    if (UPDATE) {
      mkdirSync(GOLDEN_DIR, { recursive: true })
      for (const [name, text] of Object.entries(built)) writeFileSync(join(GOLDEN_DIR, name), text)
      return
    }
    // test-map.json 은 손으로 쓰는 이식 대응표라 골든이 아니다(아래 'fetch.test.ts 이식 대응표')
    const onDisk = readdirSync(GOLDEN_DIR).filter((n) => n.endsWith('.json') && n !== TEST_MAP).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts 로 골든을 만든다').toEqual(Object.keys(built).sort())
    for (const [name, text] of Object.entries(built)) {
      expect(existsSync(join(GOLDEN_DIR, name))).toBe(true)
      expect(readFileSync(join(GOLDEN_DIR, name), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
