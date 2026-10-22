// GitHub 원격 전용 저장소: 작업 폴더 없이 앱 캐시(<userData>/remote-repos/<owner>/<repo>.git)에 bare + blob:none partial 저장소를
// 만들어 두고 fetch 만 한다. 이 파일은 그 저장소의 검증·경로·설정·실행 환경·초기화를 모은다.
// 프로덕션 경로는 https + github.com 으로 고정이고, 테스트만 url·프로토콜을 인자로 바꿀 수 있다.
import { execFile } from 'node:child_process'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import type { GithubRepoLink } from '@shared/types'
import { GIT_ENV, GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { isAllowedConfig, parseConfigList, readConfigEntries, type ConfigEntry } from './fetch'

export const GITHUB_SOURCE_ID = 'github'
/** 앱 캐시 아래 저장소 폴더 이름의 접미사 */
const CACHE_SUFFIX = '.git'
/** GitHub 의 한도: 사용자·조직 이름 39자, 저장소 이름 100자 */
const OWNER_MAX = 39
const REPO_MAX = 100
const BRANCH_MAX = 200
/** 한 번에 추가할 수 있는 원격 저장소 수 */
export const MAX_ADD_GITHUB = 200

export const GITHUB_ORIGIN = 'https://github.com'

const NAME_RE = /^[A-Za-z0-9_.-]+$/
/** Windows 예약 장치 이름(확장자가 붙어도 예약이다: con.git). 폴더 이름이 되므로 모든 OS 에서 거부해 결과를 같게 한다 */
const WIN_RESERVED = /^(con|prn|aux|nul|com[0-9]|lpt[0-9])$/i

function checkName(v: unknown, what: string, max: number): string {
  if (typeof v !== 'string' || v.length === 0 || v.length > max || !NAME_RE.test(v)) throw new Error(`잘못된 ${what}입니다`)
  // . / .. 는 경로가 되고, - 로 시작하면 옵션처럼 보이며, 끝의 . 은 Windows 가 지워서 다른 폴더와 겹친다
  if (v === '.' || v === '..' || v.startsWith('-') || v.endsWith('.') || WIN_RESERVED.test(v.split('.')[0]!)) throw new Error(`잘못된 ${what}입니다`)
  return v
}

/** git 에 ref 이름으로 넘겨도 되는 기본 브랜치 이름인가(보수적으로: 영문·숫자·._/+- 만) */
export function isSafeBranch(b: unknown): b is string {
  return (
    typeof b === 'string' &&
    b.length > 0 &&
    b.length <= BRANCH_MAX &&
    /^[A-Za-z0-9._/+-]+$/.test(b) &&
    !b.startsWith('-') &&
    !b.startsWith('/') &&
    !b.endsWith('/') &&
    !b.endsWith('.') &&
    !b.endsWith('.lock') &&
    !b.includes('//') &&
    !b.includes('..') &&
    !/(^|\/)\./.test(b)
  )
}

/**
 * owner/repo(와 기본 브랜치) 검증. 잘못되면 던진다.
 * 기본 브랜치는 부가 정보라 쓸 수 없는 값이면 버린다(첫 fetch 때 원격에 물어 알아낸다).
 */
export function validateGithubLink(input: unknown): GithubRepoLink {
  if (!input || typeof input !== 'object') throw new Error('잘못된 저장소입니다')
  const i = input as Record<string, unknown>
  const owner = checkName(i['owner'], '소유자 이름', OWNER_MAX)
  const repo = checkName(i['repo'], '저장소 이름', REPO_MAX)
  if (repo.toLowerCase().endsWith(CACHE_SUFFIX)) throw new Error('잘못된 저장소 이름입니다')
  return { owner, repo, ...(isSafeBranch(i['defaultBranch']) ? { defaultBranch: i['defaultBranch'] } : {}) }
}

/** 저장 파일을 읽을 때 쓰는 느슨한 형태: 검증에 실패하면 null */
export function sanitizeGithubLink(input: unknown): GithubRepoLink | null {
  try {
    return validateGithubLink(input)
  } catch {
    return null
  }
}

export function githubFullName(link: Pick<GithubRepoLink, 'owner' | 'repo'>): string {
  return `${link.owner}/${link.repo}`
}

/** 캐시 폴더. 대소문자만 다른 같은 저장소가 둘로 갈라지지 않게 소문자로 정한다 */
export function githubCachePath(base: string, link: Pick<GithubRepoLink, 'owner' | 'repo'>): string {
  return join(base, link.owner.toLowerCase(), `${link.repo.toLowerCase()}${CACHE_SUFFIX}`)
}

/**
 * path 가 base 바로 아래 정확히 `<owner>/<repo>.git`(소문자) 모양이면 그 owner/repo(소문자)를, 아니면 null.
 * 실행 환경·삭제처럼 "앱 캐시 안의 우리 저장소"임을 확인해야 하는 곳이 모두 이 함수로 판정한다.
 */
export function githubCacheSlug(base: string, path: string): { owner: string; repo: string } | null {
  if (typeof path !== 'string' || path.includes('\0')) return null
  const rel = relative(resolve(base), resolve(path))
  if (!rel || isAbsolute(rel)) return null
  const parts = rel.split(sep)
  const [o, r] = parts
  if (parts.length !== 2 || !o || !r || !r.endsWith(CACHE_SUFFIX)) return null
  const repo = r.slice(0, -CACHE_SUFFIX.length)
  try {
    checkName(o, '소유자 이름', OWNER_MAX)
    checkName(repo, '저장소 이름', REPO_MAX)
  } catch {
    return null
  }
  return o === o.toLowerCase() && repo === repo.toLowerCase() ? { owner: o, repo } : null
}

export function isGithubCachePath(base: string, path: string): boolean {
  return githubCacheSlug(base, path) !== null
}

/** 캐시 경로가 이 레코드(owner/repo)의 것인가 */
export function cachePathMatchesLink(base: string, path: string, link: Pick<GithubRepoLink, 'owner' | 'repo'>): boolean {
  const s = githubCacheSlug(base, path)
  return !!s && s.owner === link.owner.toLowerCase() && s.repo === link.repo.toLowerCase()
}

/** origin 의 URL. 초기화와 fetch 앞의 정확 일치 검사가 같은 함수를 쓰므로 둘이 어긋날 수 없다 */
export function githubRemoteUrl(link: Pick<GithubRepoLink, 'owner' | 'repo'>, origin = GITHUB_ORIGIN): string {
  return `${origin}/${link.owner}/${link.repo}${CACHE_SUFFIX}`
}

// ── 설정 ────────────────────────────────────────────────────────────────

/** init 직후 로컬 설정에 우리가 써 두는 키와 값(키는 git config --list 가 돌려주는 소문자) */
export function expectedGithubConfig(link: Pick<GithubRepoLink, 'owner' | 'repo'>, url = githubRemoteUrl(link)): Map<string, string> {
  return new Map([
    ['core.repositoryformatversion', '1'],
    ['extensions.partialclone', 'origin'],
    ['remote.origin.url', url],
    ['remote.origin.promisor', 'true'],
    ['remote.origin.partialclonefilter', 'blob:none']
  ])
}

/**
 * git init 이 플랫폼에 따라 덧붙이는 키(값은 기존 허용 목록 규칙을 따른다). bare 라서 core.bare=true 도 항상 있는데,
 * 이 값은 일반 저장소 검사(isAllowedConfig)에서는 허용하지 않으므로 여기서만 따로 받는다.
 */
const INIT_EXTRA_KEYS = new Set(['core.filemode', 'core.symlinks', 'core.ignorecase', 'core.logallrefupdates', 'core.precomposeunicode'])

/**
 * init 이 저장소 형식에 따라 덧붙일 수 있는 키와 허용 값. git 3.0 은 기본 ref 저장 형식이 reftable 이라 init 이
 * extensions.refstorage=reftable 을 쓴다. 이 키들은 없어도 되고(필수 아님), 있으면 여기 적힌 값이어야 한다.
 */
const INIT_FORMAT_VALUES: Record<string, string[]> = {
  'extensions.refstorage': ['files', 'reftable'],
  'extensions.objectformat': ['sha1']
}

/**
 * 원격 전용 저장소의 로컬(local/worktree scope) 설정이 우리가 만든 것과 정확히 같은지 본다. 문제가 없으면 빈 목록,
 * 있으면 사유가 붙은 키 목록(값은 담지 않는다).
 *  - 고정 키(expectedGithubConfig)는 값이 정확히 같고 정확히 한 번 있어야 한다. 허용 목록으로 되돌아가지 않는다
 *    (isAllowedConfig 는 remote.*.url·promisor 를 아무 값이나 통과시킨다)
 *  - 고정 키가 아닌 것은 INIT_EXTRA_KEYS 에 isAllowedConfig 를 통과하거나 core.bare=true, 또는 INIT_FORMAT_VALUES 의 값일 때만 허용한다.
 *    include.path 는 고정 키를 가릴 수 있어 어느 쪽에도 없으므로 막힌다.
 */
export function checkGithubConfig(entries: ConfigEntry[], expected: Map<string, string>): string[] {
  const problems: string[] = []
  const counts = new Map<string, number>()
  for (const e of entries) {
    if (e.scope !== 'local' && e.scope !== 'worktree') continue
    const want = expected.get(e.key)
    if (want !== undefined) {
      counts.set(e.key, (counts.get(e.key) ?? 0) + 1)
      if (e.value !== want) problems.push(`${e.key} (예상과 다름)`)
    } else if (e.key === 'core.bare') {
      if (e.value !== 'true') problems.push(`${e.key} (예상과 다름)`)
    } else if (INIT_FORMAT_VALUES[e.key]) {
      if (e.value === null || !INIT_FORMAT_VALUES[e.key]!.includes(e.value)) problems.push(`${e.key} (예상과 다름)`)
    } else if (!(INIT_EXTRA_KEYS.has(e.key) && isAllowedConfig(e.key, e.value))) {
      problems.push(`${e.key} (알 수 없는 설정)`)
    }
  }
  for (const key of expected.keys()) {
    const n = counts.get(key) ?? 0
    if (n === 0) problems.push(`${key} (없음)`)
    else if (n > 1) problems.push(`${key} (중복)`)
  }
  return problems
}

/**
 * fetch 직전에 하는 검사: 설정이 정확히 일치하고 remote 가 origin 하나뿐이어야 한다(옛 형식 .git/remotes 파일까지 `git remote` 가 읽는다).
 * 통과하면 빈 목록, 아니면 사유 목록.
 */
export async function inspectGithubRepo(exec: GitExecutor, dir: string, link: GithubRepoLink, opts?: RunOpts, url?: string): Promise<string[]> {
  const problems = checkGithubConfig(await readConfigEntries(exec, dir, opts), expectedGithubConfig(link, url))
  const r = await exec.run(dir, ['remote'], opts)
  const names = r.stdout.toString('utf8').split('\n').map((n) => n.replace(/\r$/, '')).filter(Boolean)
  if (r.code !== 0 || names.length !== 1 || names[0] !== 'origin') problems.push('remote (origin 하나여야 함)')
  return problems
}

// ── 실행 환경 ───────────────────────────────────────────────────────────

/** 사용자 설정에서 가져온 네트워크 설정 하나([키, 값]. 키는 소문자) */
export type NetworkConfigEntry = readonly [key: string, value: string]

/**
 * 격리된 github 환경으로 넘겨 주는 사용자 설정 키(전부 소문자). 회사 프록시·사설 인증서 뒤에서도 fetch 되게 하려는 것이다.
 * http.sslVerify 처럼 보안을 낮추는 키와 url.*.insteadOf · credential.* 는 일부러 넣지 않는다.
 */
const NETWORK_CONFIG_KEYS = new Set([
  'http.proxy',
  'http.sslcainfo',
  'http.sslcapath',
  'http.sslbackend',
  'http.schannelcheckrevoke',
  'http.schannelusesslcainfo',
  'http.noproxy'
])

/** `git config --list` 항목 중 시스템·전역 scope 의 네트워크 키만 고른다. 같은 키가 여럿이면 마지막 값(git 이 쓰는 값)만 남긴다 */
export function pickNetworkConfig(entries: ConfigEntry[]): NetworkConfigEntry[] {
  const last = new Map<string, string>()
  for (const e of entries) {
    if ((e.scope !== 'system' && e.scope !== 'global') || !NETWORK_CONFIG_KEYS.has(e.key) || e.value === null || e.value.length > 4096) continue
    last.set(e.key, e.value)
  }
  return [...last]
}

/**
 * 사용자 전역·시스템 git 설정에서 네트워크 키만 읽는다(앱 시작 때 한 번). github 환경은 사용자 설정을 통째로 끊으므로(githubEnv)
 * 프록시·인증서만 이렇게 따로 옮긴다. 읽을 때는 GIT_CONFIG_COUNT/KEY/VALUE/PARAMETERS 와 GIT_DIR 류를 비워
 * 이미 격리된 값이나 어떤 저장소의 설정이 섞이지 않게 한다. 읽지 못하면(git 없음·옛 git) 빈 목록이다.
 */
export async function readNetworkConfig(gitBinary = 'git', baseEnv: NodeJS.ProcessEnv = process.env): Promise<NetworkConfigEntry[]> {
  const env: NodeJS.ProcessEnv = {}
  for (const [k, v] of Object.entries(baseEnv)) {
    if (/^GIT_CONFIG_(COUNT|PARAMETERS|KEY_\d+|VALUE_\d+)$/.test(k) || /^GIT_(DIR|WORK_TREE|COMMON_DIR|INDEX_FILE|OBJECT_DIRECTORY)$/.test(k)) continue
    env[k] = v
  }
  return new Promise((resolveEntries) => {
    execFile(
      gitBinary,
      ['config', '--list', '--show-scope', '-z'],
      { env, cwd: tmpdir(), encoding: 'utf8', timeout: 10_000, windowsHide: true },
      (err, stdout) => resolveEntries(err ? [] : pickNetworkConfig(parseConfigList(stdout)))
    )
  })
}

/** GIT_CONFIG_GLOBAL 에 줘서 사용자 전역 설정을 읽지 않게 하는 빈 파일(git 이 NUL·/dev/null 을 직접 처리한다) */
export const EMPTY_GIT_CONFIG = process.platform === 'win32' ? 'NUL' : '/dev/null'

export interface GithubEnvOpts {
  /** 이 환경이 쓸 캐시 저장소 폴더. GIT_DIR 로 고정해, 폴더가 비었거나 깨져도 git 이 상위 폴더의 저장소로 올라가지 못하게 한다 */
  gitDir: string
  /** core.hooksPath 로 줄 앱 소유의 빈 디렉터리(hooksPathFor 의 결과) */
  hooksPath: string
  /** 사용자 설정에서 가져온 네트워크 키(readNetworkConfig). 기본: 없음 */
  netConfig?: readonly NetworkConfigEntry[]
  /** 테스트 전용: 프로덕션은 https 로 고정이다 */
  allowProtocol?: string
}

/** 환경 변수·헤더에 넣어도 되는 토큰인가(개행 등이 든 값은 헤더를 깨뜨린다) */
function usableToken(token: string | null): token is string {
  return typeof token === 'string' && token.length > 0 && token.length <= 4096 && !/[\s\0]/.test(token)
}

/** 토큰(과 base64 형태)을 가린다. 오류 메시지·로그에 토큰이 남지 않게 한다 */
export function redactToken(text: string, token: string | null): string {
  if (!usableToken(token)) return text
  const b64 = Buffer.from(`x-access-token:${token}`).toString('base64')
  return text.split(token).join('***').split(b64).join('***')
}

/**
 * GitHub 원격 전용 저장소에서 git 을 실행할 때의 환경. 조회 명령·lazy fetch·fetch 가 모두 이 함수를 거친다
 * (LocalExecutor 가 캐시 경로 안의 저장소에서만 이 환경을 쓴다).
 *  - GIT_NO_LAZY_FETCH 는 끈다(blob:none 저장소가 필요한 blob 을 그때그때 받는다). 사용자 환경에 1 이 있어도 이기도록 '0' 으로 둔다
 *  - 전송은 https 만. LFS 객체는 받지 않는다. 프롬프트·askpass·GCM 대화는 막는다
 *  - credential.helper 를 비워 사용자 자격 증명 도우미(GCM 등)를 부르지 않고, 토큰이 있으면 github.com 에만 Authorization 헤더를 붙인다.
 *    토큰은 인자·디스크가 아니라 이 환경 변수(GIT_CONFIG_VALUE_n)로만 git 에 전달된다
 *  - 격리: 시스템·전역 설정을 읽지 않고(GIT_CONFIG_NOSYSTEM · GIT_CONFIG_GLOBAL=빈 파일), 상속된 GIT_CONFIG_COUNT/PARAMETERS 도 버린다.
 *    사용자의 insteadOf(ssh 로 바꿔 전송 금지에 걸림) · init.defaultRefFormat · safe.bareRepository 가 이 기능을 깨지 못하게 하려는 것이다.
 *    프록시·인증서만 netConfig 로 command scope 에 옮긴다(HTTPS_PROXY 같은 환경 변수는 그대로 상속된다)
 *  - GIT_DIR 로 저장소를 고정하고 GIT_CEILING_DIRECTORIES 로 상위 탐색을 막는다. 캐시 폴더가 비었거나 깨져 있으면 `-C <폴더>` 가
 *    상위 폴더의 다른 저장소로 올라가 그 저장소에 config 를 쓰거나 lazy fetch 환경을 적용하는 일이 없도록 그냥 실패시킨다
 *  - 유지보수·훅·번들 URI·리다이렉트 추종은 끈다(리다이렉트를 따라가면 Authorization 헤더가 다른 호스트로 갈 수 있다)
 */
export function githubEnv(token: string | null, o: GithubEnvOpts): Record<string, string> {
  const gitDir = resolve(o.gitDir)
  const env: Record<string, string> = {
    ...GIT_ENV,
    GIT_NO_LAZY_FETCH: '0',
    GIT_ALLOW_PROTOCOL: o.allowProtocol ?? 'https',
    GIT_LFS_SKIP_SMUDGE: '1',
    GIT_TERMINAL_PROMPT: '0',
    GIT_ASKPASS: '',
    SSH_ASKPASS_REQUIRE: 'never',
    GCM_INTERACTIVE: 'never',
    GIT_DIR: gitDir,
    GIT_CEILING_DIRECTORIES: dirname(gitDir).replace(/\\/g, '/'),
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: EMPTY_GIT_CONFIG,
    GIT_CONFIG_PARAMETERS: ''
  }
  const extra: [string, string][] = [
    ...(o.netConfig ?? []).map(([k, v]): [string, string] => [k, v]),
    ['credential.helper', ''],
    ['maintenance.auto', 'false'],
    ['gc.auto', '0'],
    ['core.hooksPath', o.hooksPath],
    ['transfer.bundleURI', 'false'],
    ['http.followRedirects', 'false']
  ]
  if (usableToken(token)) {
    extra.push(['http.https://github.com/.extraheader', `Authorization: Basic ${Buffer.from(`x-access-token:${token}`).toString('base64')}`])
  }
  extra.forEach(([k, v], i) => {
    env[`GIT_CONFIG_KEY_${i}`] = k
    env[`GIT_CONFIG_VALUE_${i}`] = v
  })
  env['GIT_CONFIG_COUNT'] = String(extra.length)
  return env
}

// ── 초기화·HEAD ─────────────────────────────────────────────────────────

async function mustRun(exec: GitExecutor, dir: string, args: string[], opts?: RunOpts): Promise<string> {
  const r = await exec.run(dir, args, opts)
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git ${args[0]} 실패(${r.code})`, r.code, r.stderr, args)
  return r.stdout.toString('utf8')
}

/**
 * 빈 폴더 dir 에 원격 전용 저장소를 만든다(네트워크 없음). --template= 로 사용자의 init 템플릿·훅이 복사되지 않게 하고,
 * 기본 브랜치를 알면 HEAD 를 refs/remotes/origin/<브랜치> 로 미리 가리켜 둔다(첫 fetch 가 끝나면 그대로 그 브랜치가 열린다).
 * 호출자는 dir 을 이미 만들어 두어야 하고, 실패하면 dir 을 정리해야 한다.
 */
export async function initGithubRepo(exec: GitExecutor, dir: string, link: GithubRepoLink, o: { url?: string } = {}): Promise<void> {
  await mustRun(exec, dir, ['init', '-q', '--bare', '--template='])
  for (const [key, value] of expectedGithubConfig(link, o.url)) await mustRun(exec, dir, ['config', '--local', key, value])
  if (link.defaultBranch) await mustRun(exec, dir, ['symbolic-ref', 'HEAD', `refs/remotes/origin/${link.defaultBranch}`])
}

/**
 * fetch 뒤에 HEAD 가 받아 온 기본 브랜치를 가리키게 맞춘다(그래야 로그의 HEAD·사이드바 기본 브랜치가 열린다).
 * 기본 브랜치는 ① 레코드에 있고 실제로 받아졌으면 그것, ② refs/remotes/origin/HEAD(git 2.48+ 가 fetch 때 만든다),
 * ③ `ls-remote --symref origin HEAD` 순서로 알아낸다. 알아낸 브랜치 이름을 돌려주고, 모르면(빈 저장소 등) null.
 * ls-remote 는 네트워크 호출이라 fetch 와 같은 환경(opts.env)·인자(netArgs)로 돌린다. HEAD 갱신은 캐시 저장소 안의 ref 쓰기다.
 */
export async function ensureGithubHead(exec: GitExecutor, dir: string, link: GithubRepoLink, opts: RunOpts, netArgs: string[] = []): Promise<string | null> {
  const has = async (b: string): Promise<boolean> => (await exec.run(dir, ['rev-parse', '--verify', '--quiet', `refs/remotes/origin/${b}^{commit}`], opts)).code === 0
  const symref = async (name: string): Promise<string | null> => {
    const r = await exec.run(dir, ['symbolic-ref', '--quiet', name], opts)
    return r.code === 0 ? r.stdout.toString('utf8').trim() : null
  }
  let branch: string | null = null
  if (link.defaultBranch && (await has(link.defaultBranch))) branch = link.defaultBranch
  if (!branch) {
    const m = /^refs\/remotes\/origin\/(.+)$/.exec((await symref('refs/remotes/origin/HEAD')) ?? '')
    if (m && isSafeBranch(m[1]) && (await has(m[1]))) branch = m[1]
  }
  if (!branch) {
    const r = await exec.run(dir, [...netArgs, 'ls-remote', '--symref', 'origin', 'HEAD'], opts)
    const m = r.code === 0 ? /^ref: refs\/heads\/(\S+)\tHEAD$/m.exec(r.stdout.toString('utf8')) : null
    if (m && isSafeBranch(m[1]) && (await has(m[1]))) branch = m[1]
  }
  if (!branch) return null
  const target = `refs/remotes/origin/${branch}`
  for (const name of ['HEAD', 'refs/remotes/origin/HEAD']) {
    if ((await symref(name)) !== target) await mustRun(exec, dir, ['symbolic-ref', name, target], opts)
  }
  return branch
}
