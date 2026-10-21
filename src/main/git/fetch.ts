// 원격 fetch. 이 앱이 저장소에 하는 유일한 쓰기(원격 추적 ref·객체 갱신)이므로 입구에서 저장소 설정을 검사한다.
import { createHash } from 'node:crypto'
import { readdirSync, statSync } from 'node:fs'
import { AbortError, GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import type { SourceKind } from '@shared/types'

export interface ConfigEntry {
  scope: string
  /** 소문자로 정규화한 키(섹션·변수는 git 이 이미 소문자, 부분 섹션은 대소문자를 구분하지만 패턴 검사에는 무관하다) */
  key: string
  value: string | null
}

/**
 * 저장소(local/worktree scope, include 로 끌어온 파일 포함) 설정은 **허용 목록**으로 검사한다. 목록에 없는 키는 전부
 * "알 수 없는 설정"으로 보고 자동 fetch 를 하지 않으며, 수동 새로고침에서 사용자가 키·값을 보고 허용해야 한다.
 *
 * 왜 차단 목록이 아니라 허용 목록인가: git 과 그 주변 도구(Git Credential Manager 등)는 외부 명령을 실행하거나 연결을 열거나
 * 파일을 쓰는 설정을 계속 추가한다(core.sshCommand · credential.* · trace2.* · fetch.bundleURI · http.* · url.*.insteadOf …).
 * 차단 목록은 새로 나온 키를 번번이 놓쳤다. 반대로 "읽기만 하는 데이터"인 키는 한정돼 있다.
 * 시스템·전역 설정은 사용자가 정한 것이라 신뢰하고 검사하지 않는다.
 *
 * 허용한 키(모두 소문자; 값이 명령·경로로 실행·접속되지 않는 데이터뿐이다):
 *  - core.*  저장소 형식·파일 시스템 동작 플래그. editor·sshCommand·gitProxy·askPass·pager·alternateRefsCommand 등 명령은 뺀다.
 *    core.bare 는 false 일 때만 허용한다(true 로 두면 체크아웃된 브랜치 보호가 꺼진다).
 *    core.hooksPath 는 husky 등이 흔히 쓰는데, fetch 에는 -c 로 빈 디렉터리를 덮어쓰므로(tests 로 확인) 허용한다.
 *    core.fsmonitor 는 true/false 값일 때만(그 밖의 값은 실행할 훅 경로) 허용한다.
 *    core.excludesFile 은 무시 패턴 파일 경로다(읽기만 하고 실행하지 않는다).
 *  - remote.<n>.{url,pushurl,tagopt,prune,mirror,skipdefaultupdate,skipfetchall,promisor,partialclonefilter}
 *    URL 은 따로 unsafeRemoteUrl·userinfo 검사를 거친다. uploadpack·receivepack·vcs·proxy 는 명령/접속이라 뺀다.
 *    prunetags 는 뺀다(로컬 태그 삭제). remote.<n>.fetch 는 값이 refs/remotes/<n>/ 아래로만 쓰는 형태일 때만 허용한다(fetchRefspecOk).
 *    (fetch 자체는 설정된 refspec 을 아예 쓰지 않는다 — runFetch 참고. 이 검사는 그 값을 믿어도 되는지 사용자에게 묻지 않기 위한 것이다)
 *  - branch.<n>.{remote,merge,rebase,pushremote,description,vscode-merge-base,github-pr-owner-number}  브랜치 이름·메모 같은 데이터
 *  - user.{name,email}, init.defaultbranch, pull.{rebase,ff}, push.{default,autosetupremote}, fetch.prune
 *  - lfs.repositoryformatversion, filter.lfs.{clean,smudge,process,required} — git-lfs 기본값과 정확히 같을 때만
 *    (filter 는 fetch 에서 실행되지 않지만 임의 명령을 담을 수 있는 키라 값을 고정한다)
 *  - extensions.{objectformat,worktreeconfig,refstorage,partialclone}
 *  - submodule.<n>.{url,active}, submodule.active  (url 은 unsafeRemoteUrl 검사)
 *  - include.path, includeIf.<조건>.path  가리키는 파일의 항목은 따로 이 검사를 받는다
 * 그 밖의 http.* · credential.* · protocol.* · url.* · trace2.* · transfer.* · fetch.bundleURI · ssh.* · alias.* · diff/merge/filter.* ·
 * gui/i18n/color.* 는 모두 "알 수 없는 설정"이다.
 */
const ALLOWED_KEYS: RegExp[] = [
  /^core\.(repositoryformatversion|filemode|logallrefupdates|ignorecase|precomposeunicode|symlinks|autocrlf|eol|safecrlf|longpaths|quotepath|checkstat|trustctime|untrackedcache|fscache|preloadindex|commentchar|hidedotfiles|abbrev|whitespace|worktree|sparsecheckout|sparsecheckoutcone|hookspath|excludesfile)$/,
  /^remote\..+\.(url|pushurl|push|tagopt|prune|mirror|skipdefaultupdate|skipfetchall|promisor|partialclonefilter)$/,
  /^branch\..+\.(remote|merge|rebase|pushremote|description|vscode-merge-base|github-pr-owner-number)$/,
  /^user\.(name|email)$/,
  /^init\.defaultbranch$/,
  /^pull\.(rebase|ff)$/,
  /^push\.(default|autosetupremote)$/,
  /^fetch\.prune$/,
  /^lfs\.repositoryformatversion$/,
  /^extensions\.(objectformat|worktreeconfig|refstorage|partialclone)$/,
  /^submodule\.(.+\.)?(url|active)$/,
  /^include\.path$/,
  /^includeif\..+\.path$/
]

/** 값까지 맞아야 허용하는 키. git-lfs 가 `git lfs install` 로 쓰는 기본값(--skip-smudge 변형 포함) */
const ALLOWED_VALUES: Record<string, string[]> = {
  'filter.lfs.clean': ['git-lfs clean -- %f'],
  'filter.lfs.smudge': ['git-lfs smudge -- %f', 'git-lfs smudge --skip -- %f'],
  'filter.lfs.process': ['git-lfs filter-process', 'git-lfs filter-process --skip'],
  'filter.lfs.required': ['true'],
  'core.fsmonitor': ['true', 'false'],
  // init·clone 이 항상 쓰는 값. true 로 두면 체크아웃된 브랜치 보호가 꺼지므로 false 만 허용한다
  'core.bare': ['false']
}

const REMOTE_URL_KEY = /^remote\..+\.(url|pushurl)$/
const SUBMODULE_URL_KEY = /^submodule\..+\.url$/

/** 저장소 설정 항목이 허용 목록에 있는가(키, 필요하면 값까지) */
/**
 * remote.<n>.fetch 가 refs/remotes/<n>/ 아래로만 쓰는 형태인가: `[+]refs/heads/*:refs/remotes/<n>/*` 또는 한 브랜치씩
 * `[+]refs/heads/<x>:refs/remotes/<n>/<x>`. 그 밖(refs/heads·refs/stash·refs/replace·HEAD 로 쓰기, 삭제 refspec …)은 알 수 없는 설정이다.
 * key 는 소문자로 정규화돼 있으므로 이름은 대소문자 구분 없이 비교한다.
 */
export function fetchRefspecOk(key: string, value: string | null): boolean {
  const m = /^remote\.(.+)\.fetch$/.exec(key)
  if (!m || value === null) return false
  const parts = value.split(':')
  if (parts.length !== 2) return false
  const src = parts[0]!.replace(/^\+/, '')
  const dst = parts[1]!
  const prefix = `refs/remotes/${m[1]}/`
  if (!src.startsWith('refs/heads/') || dst.toLowerCase().slice(0, prefix.length) !== prefix) return false
  const srcRest = src.slice('refs/heads/'.length)
  const dstRest = dst.slice(prefix.length)
  if (srcRest === '*') return dstRest === '*'
  return srcRest !== '' && !/[*\s^~?[\\]/.test(srcRest) && !srcRest.includes('..') && srcRest === dstRest
}

export function isAllowedConfig(key: string, value: string | null): boolean {
  if (/^remote\..+\.fetch$/.test(key)) return fetchRefspecOk(key, value)
  const fixed = ALLOWED_VALUES[key]
  if (fixed) return value !== null && fixed.includes(value)
  return ALLOWED_KEYS.some((re) => re.test(key))
}

/**
 * 거부할 remote URL 인지 판정한다(사유를 돌려주고, 괜찮으면 null).
 * GIT_ALLOW_PROTOCOL 이 ext/fd/원격 도우미를 이미 막지만 의도를 명시하고 한 겹 더 둔다.
 */
export function unsafeRemoteUrl(url: string): string | null {
  const u = url.trim()
  if (u.startsWith('-')) return '옵션처럼 보이는 URL'
  // UNC·네트워크 경로(\\host\share, //host@port/DavWWWRoot, \\?\UNC\…, \\.\…)와 호스트가 있는 file URL: Windows 는 접속하며 NTLM 을 보낸다.
  // 모든 플랫폼에서 거부한다(로컬 경로와 file:///C:/… 는 그대로 둔다)
  if (/^[\\/]{2}/.test(u)) return 'UNC·네트워크 경로'
  const file = /^file:(.*)$/is.exec(u)
  if (file && (/^[\\/]{2}[^\\/]/.test(file[1]!) || /^[\\/]{4}/.test(file[1]!))) return '호스트가 있는 file URL(네트워크 경로)'
  const helper = /^([A-Za-z0-9+.-]+)::/.exec(u)
  if (helper) return `${helper[1]!.toLowerCase()}:: 전송 도우미`
  if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\/([^/@]*@)?-/.test(u)) return '옵션처럼 보이는 호스트'
  return null
}

/** `git config --list --show-scope -z` 출력 파싱. 항목은 `scope\0key\nvalue\0`(값 없는 키는 `scope\0key\0`) 두 토큰씩 */
export function parseConfigList(out: string): ConfigEntry[] {
  const entries: ConfigEntry[] = []
  const tokens = out.split('\0')
  for (let i = 0; i + 1 < tokens.length; i += 2) entries.push(parseRecord(tokens[i]!, tokens[i + 1]!))
  return entries
}

/** --show-scope 없는 `git config --<scope> --list -z` 출력 파싱. 항목은 `key\nvalue\0` 한 토큰씩이고 scope 는 호출자가 정한다 */
export function parseScopedConfigList(out: string, scope: string): ConfigEntry[] {
  return out
    .split('\0')
    .filter(Boolean)
    .map((rec) => parseRecord(scope, rec))
}

function parseRecord(scope: string, rec: string): ConfigEntry {
  const nl = rec.indexOf('\n')
  return { scope, key: (nl < 0 ? rec : rec.slice(0, nl)).toLowerCase(), value: nl < 0 ? null : rec.slice(nl + 1) }
}

export interface ConfigInspection {
  hasRemote: boolean
  /** 허용 목록에 없는 저장소 설정(알 수 없는 설정). 사용자가 허용하면 fetch 할 수 있다. raw 는 표시하지 않는 원본(지문 계산용, 예: 비밀번호가 든 URL) */
  dangerous: { key: string; value: string; raw?: string }[]
  /** `git remote` 가 돌려준 remote 이름들(fetch 대상) */
  remoteNames: string[]
  /** 모든 remote 의 실제(insteadOf 적용) URL. 수동 fetch 승인 지문의 재료 */
  remotes: { name: string; url: string }[]
  /** remote 이름+URL 지문 — URL 이 바뀌면 달라진다 */
  remotesFingerprint: string
  /** 허용 여부와 무관하게 거부하는 항목(remote URL) */
  forbidden: { key: string; reason: string }[]
  /** 사용자(시스템·전역·명령줄 scope)가 정한 core.sshCommand 값. 저장소 안의 값은 절대 쓰지 않는다 */
  userSshCommand: string | null
  /** 위험 항목의 지문(같은 항목을 같은 값으로 다시 허용할 필요가 없게) */
  fingerprint: string
}

export function inspectEntries(entries: ConfigEntry[]): ConfigInspection {
  const dangerous: ConfigInspection['dangerous'] = []
  const forbidden: ConfigInspection['forbidden'] = []
  let hasRemote = false
  let userSshCommand: string | null = null
  for (const e of entries) {
    const repoScoped = e.scope === 'local' || e.scope === 'worktree'
    // 나중에 나온 값이 이긴다
    if (e.key === 'core.sshcommand' && !repoScoped && e.value) userSshCommand = e.value
    if (REMOTE_URL_KEY.test(e.key) || SUBMODULE_URL_KEY.test(e.key)) {
      if (REMOTE_URL_KEY.test(e.key) && e.key.endsWith('.url')) hasRemote = true
      const reason = unsafeRemoteUrl(e.value ?? '')
      if (reason) forbidden.push({ key: e.key, reason })
    }
    if (repoScoped && !isAllowedConfig(e.key, e.value)) dangerous.push({ key: e.key, value: e.value ?? '' })
  }
  const fingerprint = fingerprintOf(dangerous)
  return { hasRemote, dangerous, remoteNames: [], remotes: [], remotesFingerprint: '', forbidden, userSshCommand, fingerprint }
}

function fingerprintOf(dangerous: ConfigInspection['dangerous']): string {
  return createHash('sha256').update(JSON.stringify(dangerous.map((d) => [d.key, d.raw ?? d.value]))).digest('hex')
}

/** http(s) URL 의 userinfo(user[:pass]@). 있으면 NTLM/Negotiate 인증이 allowNTLMAuth=false 로도 나갈 수 있다 */
const HTTP_USERINFO = /^(https?:\/\/)([^/?#]*)@/i

/**
 * 설정 파일 목록에 없는 옛 형식 remote(.git/remotes/<n>, .git/branches/<n>)까지 포함해, 모든 remote 의 실제 URL 을 검사한다.
 * `git remote` 와 `git remote get-url --all` 은 설정·옛 파일을 읽기만 한다. 하나라도 읽지 못하면 막는다.
 */
async function inspectRemoteUrls(exec: GitExecutor, repoPath: string, ins: ConfigInspection, opts?: RunOpts): Promise<void> {
  const list = await exec.run(repoPath, ['remote'], opts)
  if (list.code !== 0) {
    ins.forbidden.push({ key: 'remote', reason: `remote 목록을 읽지 못함: ${list.stderr.trim().slice(0, 120)}` })
    return
  }
  const names = list.stdout
    .toString('utf8')
    .split('\n')
    .map((n) => n.replace(/\r$/, ''))
    .filter(Boolean)
  if (names.length > 0) ins.hasRemote = true
  ins.remoteNames = names
  for (const name of names) {
    const r = await exec.run(repoPath, ['remote', 'get-url', '--all', '--', name], opts)
    if (r.code !== 0) {
      ins.forbidden.push({ key: `remote.${name}`, reason: 'URL 을 읽지 못함' })
      continue
    }
    for (const url of r.stdout.toString('utf8').split('\n').map((u) => u.replace(/\r$/, ''))) {
      if (!url) continue
      ins.remotes.push({ name, url })
      const reason = unsafeRemoteUrl(url)
      if (reason) ins.forbidden.push({ key: `remote.${name}.url`, reason })
      // userinfo 가 있는 http(s) URL 은 자동으로 쓰지 않는다(비밀번호가 화면·로그에 나오지 않게 가린다)
      else if (HTTP_USERINFO.test(url)) {
        ins.dangerous.push({ key: `remote.${name}.url`, value: url.replace(HTTP_USERINFO, '$1<userinfo>@'), raw: url })
      }
    }
  }
  ins.fingerprint = fingerprintOf(ins.dangerous)
  ins.remotesFingerprint = createHash('sha256').update(JSON.stringify(ins.remotes.map((r) => [r.name, r.url]))).digest('hex')
}

export async function inspectConfig(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<ConfigInspection> {
  const r = await exec.run(repoPath, ['config', '--list', '--show-scope', '--includes', '-z'], opts)
  let entries: ConfigEntry[]
  if (r.code === 0) entries = parseConfigList(r.stdout.toString('utf8'))
  else if (/unknown option.*show-scope/i.test(r.stderr)) entries = await readConfigByScope(exec, repoPath, opts)
  // 설정을 읽지 못하면 검사할 수 없으므로 fetch 하지 않는다
  else throw configReadError(r.code, r.stderr)
  const ins = inspectEntries(entries)
  await inspectRemoteUrls(exec, repoPath, ins, opts)
  return ins
}

/** `--worktree` 를 모르는 git, 또는 config.worktree 가 없음(GIT_ENV 가 LC_ALL=C.UTF-8 이라 메시지는 영어다) */
const WORKTREE_CONFIG_ABSENT = /unknown option [`']worktree'|unable to read config file '[^']*config\.worktree': No such file or directory/

function configReadError(code: number, stderr: string): GitError {
  return new GitError(`저장소 설정을 검사하지 못해 fetch 하지 않습니다: ${stderr.trim()}`, code, stderr, ['config'])
}

/**
 * --show-scope 가 없는 git(2.26 미만, 예: Ubuntu 18.04 의 2.17)용: 범위마다 따로 읽는다. 순서는 git 이 적용하는 순서와 같다.
 *  - system·global 은 파일이 없으면 실패하므로 비어 있는 것으로 본다(사용자 설정이라 검사 대상도 아니고 core.sshCommand 만 읽는다)
 *  - local 은 반드시 읽혀야 한다
 *  - worktree(2.20+)는 extensions.worktreeConfig 가 켜져 있을 때만 읽는다(꺼져 있으면 --worktree 가 .git/config 를 다시
 *    돌려줘 local 과 겹친다). 옵션을 모르는 git(2.20 미만)은 config.worktree 를 아예 적용하지 않고, 파일이 없으면 적용할 것이
 *    없으므로 둘 다 비어 있는 것으로 본다. 그 밖의 이유로 읽지 못하면 검사할 수 없으므로 실패한다
 */
async function readConfigByScope(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<ConfigEntry[]> {
  const entries: ConfigEntry[] = []
  for (const scope of ['system', 'global', 'local', 'worktree']) {
    // git 의 참 값(값 없는 키·true·yes·on·0 이 아닌 수)이면 켜진 것으로 본다 — 애매하면 읽는 쪽으로
    const on = entries.some((e) => e.key === 'extensions.worktreeconfig' && (e.value === null || !/^(false|no|off|0|)$/i.test(e.value.trim())))
    if (scope === 'worktree' && !on) continue
    const r = await exec.run(repoPath, ['config', `--${scope}`, '--list', '--includes', '-z'], opts)
    if (r.code === 0) entries.push(...parseScopedConfigList(r.stdout.toString('utf8'), scope))
    else if (scope === 'local') throw configReadError(r.code, r.stderr)
    else if (scope === 'worktree' && !WORKTREE_CONFIG_ABSENT.test(r.stderr)) throw configReadError(r.code, r.stderr)
  }
  return entries
}

// ── git 버전 ────────────────────────────────────────────────────────────

/** `git version 2.49.0.windows.1` → [2, 49, 0]. 읽지 못하면 null */
export function parseGitVersion(text: string): [number, number, number] | null {
  const m = /git version (\d+)\.(\d+)(?:\.(\d+))?/.exec(text)
  return m ? [Number(m[1]), Number(m[2]), Number(m[3] ?? 0)] : null
}

export function versionAtLeast(v: [number, number, number] | null, min: [number, number, number]): boolean {
  if (!v) return false
  for (let i = 0; i < 3; i++) {
    if (v[i]! !== min[i]!) return v[i]! > min[i]!
  }
  return true
}

/** file 전송을 허용해도 되는 최소 버전. 그 아래는 CVE-2024-32004(로컬 클론·fetch 를 통한 코드 실행)에 취약하다 */
export const FILE_TRANSPORT_MIN_GIT: [number, number, number] = [2, 45, 1]

export async function gitVersion(exec: GitExecutor): Promise<[number, number, number] | null> {
  try {
    const r = await exec.run('.', ['--version'], { signal: AbortSignal.timeout(15_000) })
    return r.code === 0 ? parseGitVersion(r.stdout.toString('utf8')) : null
  } catch {
    return null
  }
}

// ── fetch 실행 ───────────────────────────────────────────────────────────

/**
 * fetch 에만 쓰는 -c 설정. 저장소 설정·훅이 fetch 중에 코드를 실행하거나 부수 작업을 하지 못하게 덮어쓴다.
 * http.emptyAuth·allowNTLMAuth·delegation, credential.allowWindowsAuth 는 Windows 통합 인증(NTLM/Negotiate)을
 * 무인 fetch 중에 공격자의 https remote 로 흘리지 않게 한다(모르는 키는 옛 git 도 무시한다).
 */
export function fetchConfigArgs(hooksPath: string, opts: { credentialHelpers?: boolean } = {}): string[] {
  return [
    // 값이 빈 credential.helper 는 도우미 목록을 비운다 — 무인(자동) fetch 가 사용자가 쓴 적 없는 호스트에 도우미를 부르지 않게 한다
    ...(opts.credentialHelpers ? [] : ['-c', 'credential.helper=']),
    '-c',
    'maintenance.auto=false',
    '-c',
    'gc.auto=0',
    '-c',
    'fetch.writeCommitGraph=false',
    // .git/hooks 의 reference-transaction 훅이 ref 갱신 때 실행되는 것을 막는다(비어 있는 훅 디렉터리를 지정)
    '-c',
    `core.hooksPath=${hooksPath}`,
    // 번들 URI: 저장소가 정한 경로·URL(로컬·UNC·http)에서 번들을 받아 refs/bundles/* 를 쓰는 기능을 끈다(허용 목록의 이중 방어)
    '-c',
    'fetch.bundleURI=',
    '-c',
    'transfer.bundleURI=false',
    '-c',
    'http.followRedirects=initial',
    '-c',
    'http.emptyAuth=false',
    '-c',
    'http.allowNTLMAuth=false',
    '-c',
    'http.delegation=none',
    '-c',
    'credential.allowWindowsAuth=false'
  ]
}

const FETCH_ARGS = ['fetch', '--prune', '--no-prune-tags', '--no-recurse-submodules']

/**
 * 훅 차단용 디렉터리. Windows 의 로컬 git.exe 는 `/dev/null` 을 그대로 받아 `<저장소 드라이브>:\dev\null\` 로 풀어 쓰므로
 * (공유·USB·다중 사용자 드라이브에서 공격자가 그 경로에 훅을 심을 수 있다) 앱이 소유한 드라이브 문자 경로의 빈 디렉터리를 쓴다.
 * POSIX 환경(WSL·SSH·mac·Linux)은 /dev/null 이면 충분하다.
 */
export function hooksPathFor(kind: SourceKind, noHooksDir: string | null, platform: NodeJS.Platform = process.platform): string {
  if (kind !== 'local') return '/dev/null'
  if (noHooksDir) {
    assertEmptyDir(noHooksDir)
    return noHooksDir.replace(/\\/g, '/')
  }
  if (platform === 'win32') throw new Error('훅 차단용 빈 디렉터리가 없어 fetch 하지 않습니다')
  return '/dev/null'
}

function assertEmptyDir(dir: string): void {
  let ok = false
  try {
    ok = statSync(dir).isDirectory() && readdirSync(dir).length === 0
  } catch {
    ok = false
  }
  if (!ok) throw new Error(`훅 차단용 디렉터리가 없거나 비어 있지 않아 fetch 하지 않습니다: ${dir}`)
}

/** 이 fetch 가 쓸 ssh 명령. 저장소 안의 core.sshCommand 가 절대 적용되지 않도록 항상 GIT_SSH_COMMAND 로 명시한다(환경변수가 설정보다 우선) */
export function resolveSshCommand(
  ins: Pick<ConfigInspection, 'userSshCommand'>,
  kind: SourceKind,
  env: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform
): string {
  if (ins.userSshCommand) return ins.userSshCommand
  if (kind === 'local') {
    if (env['GIT_SSH_COMMAND']) return env['GIT_SSH_COMMAND']
    const ssh = env['GIT_SSH']
    if (ssh) {
      const p = platform === 'win32' ? ssh.replace(/\\/g, '/') : ssh
      return `'${p.replace(/'/g, `'\\''`)}'`
    }
  }
  return 'ssh -o BatchMode=yes -o ConnectTimeout=15'
}

/** fetch 전용 환경변수. GIT_ENV(전부 막음) 위에 덮어쓴다 */
export function fetchEnv(opts: { sshCommand: string; allowFile: boolean }): Record<string, string> {
  return {
    // ext:: · fd:: 와 원격 도우미는 제외한다. file 은 자동 fetch 에서 빼고, 수동에서도 CVE-2024-32004 가 고쳐진 git 에서만 둔다
    GIT_ALLOW_PROTOCOL: opts.allowFile ? 'https:http:ssh:git:file' : 'https:http:ssh:git',
    GIT_TERMINAL_PROMPT: '0',
    GIT_ASKPASS: '',
    SSH_ASKPASS_REQUIRE: 'never',
    GCM_INTERACTIVE: 'never',
    GIT_SSH_COMMAND: opts.sshCommand
  }
}

export interface FetchRunOpts {
  signal?: AbortSignal
  /** fetchConfigArgs 에 넣을 훅 디렉터리(hooksPathFor 의 결과) */
  hooksPath: string
  allowFile: boolean
  /** 자격 증명 도우미를 쓴다(사용자가 직접 누른 fetch, 또는 같은 remote 가 수동 fetch 로 승인된 경우) */
  credentialHelpers: boolean
}

/**
 * fetch 를 실행한다. 실패하면 GitError(stderr 포함).
 *
 * `fetch --all` 을 쓰지 않는다. 설정(remote.<n>.fetch)의 refspec 이 목적지를 정하면 refs/heads·refs/stash·refs/replace·HEAD 를
 * 덮어쓰고, --prune 이 로컬 브랜치를 지우고, prunetags 가 태그를 지울 수 있다(실측: tests/fetch.test.ts 의 시나리오). 그래서 remote 마다
 * 목적지가 refs/remotes/<이름>/ 로 고정된 refspec 을 **명령줄에** 직접 준다.
 *  - `--refmap=`(빈 값)이 필수다. 이게 없으면 git 이 설정된 refspec 으로 "기회적 원격 추적 갱신"을 해서 명령줄 refspec 을 줘도
 *    설정이 고른 목적지에 쓴다(git 2.49 실측: refs/heads/HEAD·refs/replace/<sha>·refs/stash 가 만들어짐).
 *  - `--prune` 은 명령줄 refspec 의 목적지(refs/remotes/<이름>/*)만 정리하고, `--no-prune-tags` 로 설정의 prunetags 를 이긴다.
 *  - 태그는 git 기본 자동 따라가기(새 태그만 추가, 기존 태그는 강제로 덮지 않는다)를 그대로 둔다.
 * 옛 git 이 --no-write-fetch-head 를 모르면 그 옵션 없이 한 번 더 한다. 한 remote 가 실패해도 나머지는 계속하고 첫 오류를 던진다.
 */
export async function runFetch(exec: GitExecutor, repoPath: string, ins: ConfigInspection, o: FetchRunOpts): Promise<void> {
  const runOpts: RunOpts = {
    env: fetchEnv({ sshCommand: resolveSshCommand(ins, exec.kind), allowFile: o.allowFile }),
    ...(o.signal ? { signal: o.signal } : {})
  }
  const cfg = fetchConfigArgs(o.hooksPath, { credentialHelpers: o.credentialHelpers })
  let firstError: GitError | null = null
  for (const name of ins.remoteNames) {
    const run = (extra: string[]) =>
      exec.run(
        repoPath,
        [...cfg, ...FETCH_ARGS, ...extra, '--refmap=', '--', name, `+refs/heads/*:refs/remotes/${name}/*`],
        runOpts
      )
    let r = await run(['--no-write-fetch-head'])
    if (r.code !== 0 && /unknown option.*write-fetch-head/i.test(r.stderr)) r = await run([])
    if (r.code !== 0 && !firstError) {
      firstError = new GitError(r.stderr.trim() || `git fetch 실패(${r.code})`, r.code, r.stderr, FETCH_ARGS)
    }
  }
  if (firstError) throw firstError
}

export type FetchFailure = 'auth' | 'network' | 'protocol'

/**
 * 오류 종류: 인증 문제·허용되지 않은 전송(file)은 재시도해도 소용없어 자동 fetch 를 멈추고,
 * 그 밖은 지수 백오프로 다시 시도한다.
 */
export function classifyFailure(err: unknown): FetchFailure {
  if (err instanceof AbortError) return 'network'
  const text = err instanceof Error ? err.message : String(err)
  if (/transport '[^']*' not allowed/i.test(text)) return 'protocol'
  return /authentication failed|permission denied|could not read (username|password)|terminal prompts disabled|host key verification failed|publickey|access denied|invalid credentials/i.test(
    text
  )
    ? 'auth'
    : 'network'
}

/** 자동 fetch 다음 실행까지의 대기(ms). 실패가 쌓이면 2배씩 늘리되 30분을 넘기지 않는다 */
export function nextDelayMs(minutes: number, failures: number): number {
  const base = minutes * 60_000
  return Math.min(base * 2 ** Math.min(failures, 10), Math.max(base, 30 * 60_000))
}
