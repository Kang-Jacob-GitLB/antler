import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { describe, expect, it, vi } from 'vitest'
import type { GithubRepoLink } from '../src/shared/types'
import { SCRUB_PARENT_ENV_KEY, type GitExecutor } from '../src/main/executors/GitExecutor'
import { fetchConfigArgs, inspectEntries, parseConfigList, runFetch, type ConfigEntry } from '../src/main/git/fetch'
import {
  checkGithubConfig,
  EMPTY_GIT_CONFIG,
  ensureGithubHead,
  expectedGithubConfig,
  githubEnv,
  githubRemoteUrl,
  initGithubRepo,
  inspectGithubRepo,
  pickNetworkConfig,
  type NetworkConfigEntry
} from '../src/main/git/githubCache'

// GitHub 원격 전용 저장소 캐시 실행 기반의 골든(Phase 6b PR 2). TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenGithub.test.ts`.
// 검사하는 쪽은 src-tauri/tests/github_cache_exec.rs. git 도 네트워크도 쓰지 않는다 — 판정 함수와, 가짜 실행기에 남은 호출(argv) 순서만 비교한다.
//
// - `node:path` 는 win32 판이다(Antler 는 Windows 용이고 Rust 이식도 항상 win32 규칙 — `pnpm test` 는 ubuntu 에서도 돈다). 그래서 골든에는 가상 경로(C:\golden\…)만 든다.
// - `EMPTY_GIT_CONFIG` 는 import 시점의 `process.platform` 이라 값을 `<빈 설정>` 으로 바꿔 적는다(Rust 는 같은 자리에 자기 플랫폼 값을 놓고 비교한다).
// - 토큰은 가짜 값이다.

vi.mock('node:path', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:path')>()
  return { ...actual.win32, default: actual.win32, win32: actual.win32, posix: actual.posix }
})

const GOLDEN = new URL('./golden/github/', import.meta.url)
const UPDATE = process.env.UPDATE_GOLDEN === '1'

/** 최상위 값이 배열이면 항목 하나를 한 줄에 적는다(사례 하나의 diff 가 한 줄로 보이게). 나머지는 한 줄 JSON. 끝 줄바꿈 포함 */
function render(v: Record<string, unknown>): string {
  const rows = Object.entries(v).map(([k, x]) => (Array.isArray(x) ? `${JSON.stringify(k)}: [\n${x.map((e) => JSON.stringify(e)).join(',\n')}\n]` : `${JSON.stringify(k)}: ${JSON.stringify(x)}`))
  return `{\n${rows.join(',\n')}\n}\n`
}

const link = (owner = 'octocat', repo = 'Hello-World', defaultBranch?: string): GithubRepoLink => ({ owner, repo, ...(defaultBranch ? { defaultBranch } : {}) })
const entry = (key: string, value: string | null, scope = 'local'): ConfigEntry => ({ scope, key, value })
const ORIGINS: (string | null)[] = [null, 'file:///C:/golden/origin']

/** 정상적인 캐시 저장소의 로컬 설정(Windows 의 git init --bare 가 쓰는 core.* 포함) */
function healthy(l: GithubRepoLink, origin: string | null): ConfigEntry[] {
  return [
    entry('core.repositoryformatversion', '1'),
    entry('core.filemode', 'false'),
    entry('core.bare', 'true'),
    entry('core.symlinks', 'false'),
    entry('core.ignorecase', 'true'),
    entry('extensions.partialclone', 'origin'),
    entry('remote.origin.url', githubRemoteUrl(l, origin ?? undefined)),
    entry('remote.origin.promisor', 'true'),
    entry('remote.origin.partialclonefilter', 'blob:none')
  ]
}

const toRaw = (entries: ConfigEntry[]): string => entries.map((e) => `${e.scope}\0${e.key}${e.value === null ? '' : `\n${e.value}`}\0`).join('')

// ── 설정 정확 일치 검사 ──────────────────────────────────────────────────

interface ConfigCaseIn {
  name: string
  link: GithubRepoLink
  origin: string | null
  entries: ConfigEntry[]
}

function configCases(): ConfigCaseIn[] {
  const out: ConfigCaseIn[] = []
  const add = (name: string, entries: ConfigEntry[], l = link(), origin: string | null = null): void => {
    out.push({ name, link: l, origin, entries })
  }
  const h = (): ConfigEntry[] => healthy(link(), null)
  add('정상', h())
  add('정상(테스트 원본)', healthy(link(), ORIGINS[1]!), link(), ORIGINS[1])
  add('빈 목록', [])
  add('시스템·전역 scope 는 보지 않는다', [...h(), entry('credential.helper', 'manager', 'global'), entry('url.x.insteadof', 'y', 'system'), entry('http.proxy', 'p', 'command')])
  for (const [key, value] of [
    ['core.sshcommand', 'evil'],
    ['remote.origin.uploadpack', 'evil'],
    ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*'],
    ['remote.evil.url', 'https://github.com/o/r.git'],
    ['include.path', '../x'],
    ['includeif.gitdir:/.path', '../x'],
    ['credential.helper', 'evil'],
    ['http.proxy', 'http://evil'],
    ['core.fsmonitor', 'evil'],
    ['core.hookspath', '/x'],
    ['core.worktree', '/x'],
    ['extensions.worktreeconfig', 'true'],
    ['user.name', 'x'],
    ['core.filemode', 'true'],
    ['core.symlinks', 'maybe'],
    ['core.precomposeunicode', 'true']
  ] as const) {
    add(`키가 하나 더: ${key}`, [...h(), entry(key, value)])
    add(`키가 하나 더(worktree): ${key}`, [...h(), entry(key, value, 'worktree')])
  }
  add('값 없는 키', [...h(), entry('core.sshcommand', null)])
  add('url 변조', h().map((e) => (e.key === 'remote.origin.url' ? { ...e, value: 'https://evil.example/o/r.git' } : e)))
  add('다른 저장소의 url', healthy(link('octocat', 'Spoon-Knife'), null))
  add('다른 대소문자의 저장소 url', healthy(link('OCTOCAT', 'hello-world'), null))
  for (const e of h()) {
    if (e.key.startsWith('core.') && e.key !== 'core.repositoryformatversion') continue
    add(`필수 키 없음: ${e.key}`, h().filter((x) => x.key !== e.key))
    add(`필수 키 값이 바뀜: ${e.key}`, h().map((x) => (x.key === e.key ? { ...x, value: 'x' } : x)))
    add(`필수 키 값 없음: ${e.key}`, h().map((x) => (x.key === e.key ? { ...x, value: null } : x)))
    add(`필수 키 중복: ${e.key}`, [...h(), e])
  }
  add('같은 키가 두 번(값이 다름)', [...h(), entry('remote.origin.url', 'https://evil.example/x.git')])
  add('core.bare=false', h().map((e) => (e.key === 'core.bare' ? { ...e, value: 'false' } : e)))
  add('core.bare 값 없음', h().map((e) => (e.key === 'core.bare' ? { ...e, value: null } : e)))
  add('core.bare 없음(필수 아님)', h().filter((e) => e.key !== 'core.bare'))
  add('core.bare 두 번(둘 다 true)', [...h(), entry('core.bare', 'true')])
  for (const [key, value] of [
    ['extensions.refstorage', 'reftable'],
    ['extensions.refstorage', 'files'],
    ['extensions.refstorage', 'evil'],
    ['extensions.refstorage', null],
    ['extensions.objectformat', 'sha1'],
    ['extensions.objectformat', 'sha256'],
    ['extensions.noop', 'true']
  ] as const) {
    add(`형식 키: ${key}=${value}`, [...h(), entry(key, value)])
  }
  add('worktree scope 의 정상 항목도 센다(중복)', [...h(), entry('remote.origin.promisor', 'true', 'worktree')])
  add('여러 문제의 순서', [entry('user.name', 'x'), entry('remote.origin.url', 'z'), entry('core.bare', 'false'), ...h().filter((e) => e.key !== 'extensions.partialclone')])
  return out
}

// ── 네트워크 설정 고르기 ─────────────────────────────────────────────────

function networkCases(): { name: string; entries: ConfigEntry[] }[] {
  const long = 'x'.repeat(4097)
  return [
    {
      name: '고르는 키와 뺄 키',
      entries: [
        entry('http.proxy', 'http://old', 'system'),
        entry('http.proxy', 'http://proxy.corp:8080', 'global'),
        entry('http.sslcainfo', 'C:/ca.pem', 'global'),
        entry('http.sslcapath', '/certs', 'system'),
        entry('http.sslbackend', 'schannel', 'system'),
        entry('http.schannelcheckrevoke', 'false', 'global'),
        entry('http.schannelusesslcainfo', 'true', 'global'),
        entry('http.noproxy', 'localhost', 'global'),
        entry('http.sslverify', 'false', 'global'),
        entry('http.extraheader', 'X: y', 'global'),
        entry('url.ssh://x/.insteadof', 'https://github.com/', 'global'),
        entry('credential.helper', 'evil', 'global'),
        entry('http.proxy', 'http://repo-local', 'local'),
        entry('http.proxy', 'http://from-env', 'command'),
        entry('http.proxy', 'http://wt', 'worktree')
      ]
    },
    { name: '같은 키는 마지막 값이고 자리는 처음 자리', entries: [entry('http.noproxy', 'a', 'global'), entry('http.proxy', 'p1', 'system'), entry('http.noproxy', 'b', 'global'), entry('http.proxy', 'p2', 'global')] },
    { name: '값 없는 키는 건너뛴다', entries: [entry('http.proxy', null, 'global'), entry('http.sslcainfo', 'c', 'global')] },
    { name: '값이 4096 단위를 넘으면 건너뛴다', entries: [entry('http.proxy', 'x'.repeat(4096), 'global'), entry('http.sslcainfo', long, 'global')] },
    { name: 'astral 문자는 두 단위로 센다', entries: [entry('http.proxy', '😀'.repeat(2048), 'global'), entry('http.sslcapath', '😀'.repeat(2049), 'global')] },
    { name: '키는 소문자만 본다', entries: [entry('HTTP.PROXY', 'p', 'global')] },
    { name: '빈 값', entries: [entry('http.proxy', '', 'global')] },
    { name: '빈 목록', entries: [] }
  ]
}

// ── 실행 환경 ───────────────────────────────────────────────────────────

const HOOKS = 'C:/golden/hooks'
const SECRET = 'ghp_golden_fake_token'

interface EnvCaseIn {
  name: string
  /** 캐시 기준 폴더(Rust 는 이것으로 경로가 캐시 모양인지 판정한다) */
  cacheDir: string
  gitDir: string
  token: string | null
  netConfig: NetworkConfigEntry[]
  allowProtocol: string | null
}

function envCases(): EnvCaseIn[] {
  const dir = 'C:\\golden\\cache\\octocat\\hello.git'
  const base = { cacheDir: 'C:\\golden\\cache', gitDir: dir, token: null, netConfig: [], allowProtocol: null }
  return [
    { name: '토큰 없음', ...base },
    { name: '토큰', ...base, token: SECRET },
    { name: '토큰 개행', ...base, token: 'a\nb' },
    { name: '토큰 공백', ...base, token: 'a b' },
    { name: '토큰 NUL', ...base, token: 'a\0b' },
    { name: '토큰 빈 값', ...base, token: '' },
    { name: '토큰 4096', ...base, token: 'x'.repeat(4096) },
    { name: '토큰 4097', ...base, token: 'x'.repeat(4097) },
    { name: '네트워크 키', ...base, netConfig: [['http.proxy', 'http://proxy.corp:8080'], ['http.sslcainfo', 'C:/ca.pem']] },
    { name: '네트워크 키와 토큰', ...base, token: SECRET, netConfig: [['http.noproxy', 'localhost']] },
    { name: '프로토콜 file', ...base, allowProtocol: 'file' },
    { name: '프로토콜 http', ...base, token: SECRET, allowProtocol: 'http' },
    { name: '슬래시 경로·끝 구분자', ...base, gitDir: 'C:/golden/cache/octocat/hello.git/' },
    { name: '드라이브 대소문자·점 구성 요소', ...base, gitDir: 'c:\\golden\\cache\\.\\octocat\\hello.git' },
    { name: 'UNC 캐시', ...base, cacheDir: '\\\\server\\share\\cache', gitDir: '\\\\server\\share\\cache\\octocat\\hello.git' }
  ]
}

function captureEnv(c: EnvCaseIn): [string, string][] {
  const env = githubEnv(c.token, { gitDir: c.gitDir, hooksPath: HOOKS, netConfig: c.netConfig, ...(c.allowProtocol ? { allowProtocol: c.allowProtocol } : {}) })
  // SCRUB_PARENT_ENV_KEY 는 processRunner.childEnv 가 소비하는 TS 내부 표식이다 — Rust 는 같은 의미를 실행기 env_remove 로 구현하므로 골든에 넣지 않는다
  return Object.entries(env).filter(([k]) => k !== SCRUB_PARENT_ENV_KEY).map(([k, v]) => [k, v === EMPTY_GIT_CONFIG && k === 'GIT_CONFIG_GLOBAL' ? '<빈 설정>' : v])
}

// ── 가짜 실행기: 호출한 argv 를 남긴다 ────────────────────────────────────

type Reply = [code: number, stdout?: string, stderr?: string]

/** 명령(`args.join(' ')`)별로 응답을 준다. 없는 명령은 `defaultCode` 다 */
function scripted(script: Record<string, Reply>, calls: string[][], defaultCode: number): GitExecutor {
  return {
    kind: 'local',
    run: async (_dir: string, args: string[]) => {
      calls.push(args)
      const [code, stdout = '', stderr = ''] = script[args.join(' ')] ?? [defaultCode]
      return { code, stdout: Buffer.from(stdout), stderr }
    }
  } as unknown as GitExecutor
}

/** 호출 순서대로 응답을 준다(모자라면 성공) */
function sequenced(replies: Reply[], calls: string[][]): GitExecutor {
  let i = 0
  return {
    kind: 'local',
    run: async (_dir: string, args: string[]) => {
      calls.push(args)
      const [code, stdout = '', stderr = ''] = replies[i++] ?? [0]
      return { code, stdout: Buffer.from(stdout), stderr }
    }
  } as unknown as GitExecutor
}

const msg = (e: unknown): string => (e as Error).message

// ── HEAD 맞추기 ─────────────────────────────────────────────────────────

interface HeadCaseIn {
  name: string
  link: GithubRepoLink
  netArgs: string[]
  script: Record<string, Reply>
}

const REF = (b: string): string => `rev-parse --verify --quiet refs/remotes/origin/${b}^{commit}`
const LS = 'ls-remote --symref origin HEAD'

function headCases(): HeadCaseIn[] {
  const l = link('octocat', 'hello')
  const lb = (b: string): GithubRepoLink => link('octocat', 'hello', b)
  const setBoth = (b: string): Record<string, Reply> => ({
    [`symbolic-ref HEAD refs/remotes/origin/${b}`]: [0],
    [`symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/${b}`]: [0]
  })
  return [
    { name: '레코드의 브랜치가 받아져 있다', link: lb('main'), netArgs: [], script: { [REF('main')]: [0], ...setBoth('main') } },
    { name: '레코드의 브랜치가 이미 HEAD 다', link: lb('main'), netArgs: [], script: { [REF('main')]: [0], 'symbolic-ref --quiet HEAD': [0, 'refs/remotes/origin/main\n'], 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/main\n'] } },
    { name: 'HEAD 는 맞고 origin/HEAD 만 다르다', link: lb('main'), netArgs: [], script: { [REF('main')]: [0], 'symbolic-ref --quiet HEAD': [0, 'refs/remotes/origin/main\n'], 'symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main': [0] } },
    { name: '레코드가 없으면 origin/HEAD', link: l, netArgs: [], script: { 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/trunk\n'], [REF('trunk')]: [0], ...setBoth('trunk') } },
    { name: '레코드의 브랜치가 없어졌다 → origin/HEAD', link: lb('gone'), netArgs: [], script: { [REF('gone')]: [1], 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/trunk'], [REF('trunk')]: [0], ...setBoth('trunk') } },
    { name: 'origin/HEAD 가 가리키는 ref 가 없다 → ls-remote', link: l, netArgs: [], script: { 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/old\n'], [REF('old')]: [1], [LS]: [0, 'ref: refs/heads/main\tHEAD\nabc\tHEAD\n'], [REF('main')]: [0], ...setBoth('main') } },
    { name: 'ls-remote', link: l, netArgs: [], script: { [LS]: [0, 'ref: refs/heads/main\tHEAD\nabc\tHEAD\n'], [REF('main')]: [0], ...setBoth('main') } },
    {
      name: 'ls-remote 에 net 인자',
      link: l,
      netArgs: ['-c', 'http.followRedirects=false', '-c', 'core.hooksPath=C:/golden/hooks'],
      script: { '-c http.followRedirects=false -c core.hooksPath=C:/golden/hooks ls-remote --symref origin HEAD': [0, 'ref: refs/heads/dev\tHEAD\r\n'], [REF('dev')]: [0], ...setBoth('dev') }
    },
    { name: 'ls-remote 가 실패한다', link: l, netArgs: [], script: { [LS]: [128, '', 'fatal'] } },
    { name: 'ls-remote 출력에 기본 브랜치가 없다(빈 저장소)', link: l, netArgs: [], script: { [LS]: [0, 'abc\tHEAD\n'] } },
    { name: 'ls-remote 의 브랜치가 아직 받아지지 않았다', link: l, netArgs: [], script: { [LS]: [0, 'ref: refs/heads/main\tHEAD\n'], [REF('main')]: [1] } },
    { name: '안전하지 않은 이름은 쓰지 않는다(origin/HEAD)', link: l, netArgs: [], script: { 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/-evil\n'], [REF('-evil')]: [0] } },
    { name: '안전하지 않은 이름은 쓰지 않는다(ls-remote)', link: l, netArgs: [], script: { [LS]: [0, 'ref: refs/heads/a..b\tHEAD\n'], [REF('a..b')]: [0] } },
    { name: '레코드의 브랜치는 검사 없이 받아졌는지만 본다', link: lb('release/1.x'), netArgs: [], script: { [REF('release/1.x')]: [0], ...setBoth('release/1.x') } },
    { name: 'origin/HEAD 가 여러 줄이다', link: l, netArgs: [], script: { 'symbolic-ref --quiet refs/remotes/origin/HEAD': [0, 'refs/remotes/origin/a\nb\n'], [REF('a')]: [0] } },
    { name: 'symbolic-ref 쓰기가 실패한다', link: lb('main'), netArgs: [], script: { [REF('main')]: [0], 'symbolic-ref HEAD refs/remotes/origin/main': [128, '', 'fatal: cannot lock\n'] } },
    { name: 'symbolic-ref 쓰기가 실패한다(메시지 없음)', link: lb('main'), netArgs: [], script: { [REF('main')]: [0], 'symbolic-ref HEAD refs/remotes/origin/main': [1] } },
    { name: '아무것도 모른다', link: l, netArgs: [], script: {} }
  ]
}

async function captureHead(c: HeadCaseIn): Promise<Record<string, unknown>> {
  const calls: string[][] = []
  try {
    const result = await ensureGithubHead(scripted(c.script, calls, 1), 'C:\\golden\\cache\\octocat\\hello.git', c.link, {}, c.netArgs)
    return { ...c, result, calls }
  } catch (e) {
    return { ...c, error: msg(e), calls }
  }
}

// ── init·검사의 호출 ─────────────────────────────────────────────────────

async function captureInit(): Promise<Record<string, unknown>[]> {
  const out: Record<string, unknown>[] = []
  for (const l of [link('octocat', 'Hello-World'), link('OctoCat', 'Hello-World', 'main'), link('o', 'r', 'release/1.x')]) {
    for (const origin of ORIGINS) {
      const calls: string[][] = []
      await initGithubRepo(scripted({}, calls, 0), 'C:\\golden\\cache\\octocat\\hello.git', l, origin ? { url: githubRemoteUrl(l, origin) } : {})
      out.push({ link: l, origin, calls })
    }
  }
  // 중간에 실패하면 거기서 멈추고 stderr 문구(없으면 `git <명령> 실패(<코드>)`)를 던진다
  const failures: [string, Reply][] = [
    ['init 실패(메시지 있음)', [128, '', 'fatal: bad\n']],
    ['init 실패(메시지 없음)', [1]]
  ]
  for (const [name, reply] of failures) {
    const calls: string[][] = []
    const err = await initGithubRepo(scripted({ 'init -q --bare --template=': reply }, calls, 0), 'C:\\golden\\cache\\octocat\\hello.git', link('o', 'r')).catch(msg)
    out.push({ name, link: link('o', 'r'), origin: null, calls, script: { 'init -q --bare --template=': reply }, error: err })
  }
  const calls: string[][] = []
  const err = await initGithubRepo(scripted({ 'config --local remote.origin.promisor true': [5, '', 'locked'] }, calls, 0), 'C:\\golden\\cache\\octocat\\hello.git', link('o', 'r')).catch(msg)
  out.push({ name: '두 번째 설정 쓰기 실패', link: link('o', 'r'), origin: null, calls, script: { 'config --local remote.origin.promisor true': [5, '', 'locked'] }, error: err })
  return out
}

const CONFIG_LIST = 'config --list --show-scope --includes -z'

async function captureInspect(): Promise<Record<string, unknown>[]> {
  const out: Record<string, unknown>[] = []
  const l = link('octocat', 'Hello-World')
  const good = toRaw(healthy(l, null))
  const cases: { name: string; script: Record<string, Reply> }[] = [
    { name: '정상', script: { [CONFIG_LIST]: [0, good], remote: [0, 'origin\n'] } },
    { name: '정상(CRLF)', script: { [CONFIG_LIST]: [0, good], remote: [0, 'origin\r\n'] } },
    { name: 'remote 가 둘', script: { [CONFIG_LIST]: [0, good], remote: [0, 'origin\nextra\n'] } },
    { name: 'remote 가 origin 이 아님', script: { [CONFIG_LIST]: [0, good], remote: [0, 'upstream\n'] } },
    { name: 'remote 가 없음', script: { [CONFIG_LIST]: [0, good], remote: [0, ''] } },
    { name: 'remote 실패', script: { [CONFIG_LIST]: [0, good], remote: [128, 'origin\n', 'fatal'] } },
    { name: '설정 문제와 remote 문제', script: { [CONFIG_LIST]: [0, toRaw([entry('include.path', 'x')])], remote: [0, 'origin\nb\n'] } },
    { name: '설정을 읽지 못함', script: { [CONFIG_LIST]: [128, '', 'fatal: bad config'], remote: [0, 'origin\n'] } }
  ]
  for (const c of cases) {
    const calls: string[][] = []
    try {
      out.push({ name: c.name, link: l, calls, script: c.script, problems: await inspectGithubRepo(scripted(c.script, calls, 1), 'C:\\golden\\cache\\octocat\\hello.git', l) })
    } catch (e) {
      out.push({ name: c.name, link: l, calls, script: c.script, error: msg(e) })
    }
  }
  return out
}

// ── fetch 인자 ──────────────────────────────────────────────────────────

const NO_WRITE_FETCH_HEAD = ["error: unknown option `no-write-fetch-head'"]

interface FetchCaseIn {
  name: string
  replies: Reply[]
}

function fetchCases(): FetchCaseIn[] {
  return [
    { name: '성공', replies: [[0]] },
    { name: '옛 git 은 --no-write-fetch-head 를 모른다 → 그 옵션 없이 다시', replies: [[129, '', `${NO_WRITE_FETCH_HEAD[0]}\n`], [0]] },
    { name: '실패는 stderr 문구', replies: [[128, '', 'fatal: repository not found\n']] },
    { name: '실패는 메시지가 없으면 종료 코드', replies: [[3]] },
    { name: '다시 해도 실패', replies: [[129, '', `${NO_WRITE_FETCH_HEAD[0]}\n`], [128, '', 'fatal: x\n']] }
  ]
}

async function captureFetch(): Promise<Record<string, unknown>[]> {
  const out: Record<string, unknown>[] = []
  for (const c of fetchCases()) {
    const calls: string[][] = []
    const ins = { ...inspectEntries([]), remoteNames: ['origin'] }
    // 훅 경로·환경은 실행기가 정한다. 인자만 본다
    const exec = sequenced(c.replies, calls)
    let error: string | undefined
    try {
      await runFetch(exec, 'C:\\golden\\cache\\octocat\\hello.git', ins, { hooksPath: HOOKS, allowFile: false, credentialHelpers: false, filter: 'blob:none', followRedirects: 'false', env: {} })
    } catch (e) {
      error = msg(e)
    }
    out.push({ ...c, hooks: HOOKS, calls, ...(error === undefined ? {} : { error }) })
  }
  return out
}

async function build(): Promise<Record<string, string>> {
  const configs = configCases().map((c) => {
    const problems = checkGithubConfig(c.entries, expectedGithubConfig(c.link, githubRemoteUrl(c.link, c.origin ?? undefined)))
    // 같은 항목을 `git config --list --show-scope -z` 모양의 원문으로도 준다(Rust 는 원문을 파싱해서 본다)
    const raw = toRaw(c.entries)
    expect(checkGithubConfig(parseConfigList(raw), expectedGithubConfig(c.link, githubRemoteUrl(c.link, c.origin ?? undefined)))).toEqual(problems)
    return { name: c.name, link: c.link, origin: c.origin, raw, problems }
  })
  const networks = networkCases().map((c) => ({ name: c.name, raw: toRaw(c.entries), picked: pickNetworkConfig(parseConfigList(toRaw(c.entries))) }))
  const envs = envCases().map((c) => ({ ...c, hooks: HOOKS, env: captureEnv(c) }))
  const heads = await Promise.all(headCases().map(captureHead))
  return {
    'config.json': render({
      // 필수 키의 기대값(쓰는 순서) — 링크·원본마다
      expected: [link(), link('OctoCat', 'Hello-World')].flatMap((l) => ORIGINS.map((o) => ({ link: l, origin: o, config: [...expectedGithubConfig(l, githubRemoteUrl(l, o ?? undefined))] }))),
      cases: configs
    }),
    'network.json': render({ cases: networks }),
    'env.json': render({ cases: envs, fetchConfig: [fetchConfigArgs(HOOKS, { credentialHelpers: false, followRedirects: 'false' }), fetchConfigArgs(HOOKS, { credentialHelpers: false })] }),
    'head.json': render({ cases: heads }),
    'init.json': render({ init: await captureInit(), inspect: await captureInspect() }),
    'fetch.json': render({ cases: await captureFetch() })
  }
}

describe('GitHub 캐시 실행 기반 골든', () => {
  it('골든이 비어 있지 않고 보안 사례가 들어 있다', async () => {
    const f = await build()
    const env = JSON.parse(f['env.json']!) as { cases: { token: string | null; env: [string, string][] }[] }
    // 토큰이 든 값은 한 곳(GIT_CONFIG_VALUE_n)뿐이다
    const withToken = env.cases.find((c) => c.token === SECRET)!
    expect(withToken.env.filter(([, v]) => v.includes('Authorization: Basic'))).toHaveLength(1)
    expect(withToken.env.filter(([, v]) => v.includes(SECRET))).toHaveLength(0)
    const cfg = JSON.parse(f['config.json']!) as { cases: { problems: string[] }[] }
    expect(cfg.cases.length).toBeGreaterThan(60)
    expect(cfg.cases.some((c) => c.problems.length === 0)).toBe(true)
    expect(cfg.cases.some((c) => c.problems.some((p) => p.endsWith('(알 수 없는 설정)')))).toBe(true)
    const heads = JSON.parse(f['head.json']!) as { cases: { result?: string | null; error?: string }[] }
    expect(heads.cases.some((c) => c.result === 'trunk') && heads.cases.some((c) => c.result === null) && heads.cases.some((c) => c.error)).toBe(true)
  })

  it('골든 파일이 TS 의 현재 출력과 같다', async () => {
    const fixtures = await build()
    if (UPDATE) {
      mkdirSync(GOLDEN, { recursive: true })
      for (const [name, text] of Object.entries(fixtures)) writeFileSync(new URL(name, GOLDEN), text)
      return
    }
    const onDisk = readdirSync(GOLDEN).filter((n) => n.endsWith('.json')).sort()
    expect(onDisk, 'UPDATE_GOLDEN=1 npx vitest run tests/goldenGithub.test.ts 로 골든을 만든다').toEqual(Object.keys(fixtures).sort())
    for (const [name, text] of Object.entries(fixtures)) {
      expect(readFileSync(new URL(name, GOLDEN), 'utf8').replace(/\r\n/g, '\n'), name).toBe(text)
    }
  })
})
