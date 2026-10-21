// fetch: 저장소 설정 검사(악성 설정 차단) · 안전한 저장소의 실제 fetch · 스케줄 계산. 임시 저장소와 file:// bare remote 만 쓴다.
import { execFileSync } from 'node:child_process'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import type { FetchStatus } from '../src/shared/types'
import { wslArgv } from '../src/main/executors/WslExecutor'
import { buildRemoteCommand } from '../src/main/executors/ssh/quote'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { GitError, type GitExecutor, type RunOpts } from '../src/main/executors/GitExecutor'
import { MAX_STDERR, runProcess, stderrCollector, streamProcess } from '../src/main/executors/processRunner'
import { display, formatItem, MAX_CONFIRM_ITEMS, quoteForDisplay } from '../src/main/ipc/confirmText'
import {
  classifyFailure,
  FILE_TRANSPORT_MIN_GIT,
  fetchConfigArgs,
  fetchEnv,
  hooksPathFor,
  runFetch,
  inspectConfig,
  inspectEntries,
  isAllowedConfig,
  nextDelayMs,
  parseConfigList,
  parseGitVersion,
  parseScopedConfigList,
  resolveSshCommand,
  unsafeRemoteUrl,
  versionAtLeast
} from '../src/main/git/fetch'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { FetchService } from '../src/main/services/FetchService'
import { RepoService } from '../src/main/services/RepoService'

const SSH_DEFAULT = 'ssh -o BatchMode=yes -o ConnectTimeout=15'

describe('설정 검사(순수 함수)', () => {
  it('git config --list --show-scope -z 출력을 파싱한다', () => {
    const out = 'global\0user.name\nA B\0local\0core.sshcommand\nssh -i x\0local\0core.bare\0worktree\0remote.o.url\nu\nrl\0'
    expect(parseConfigList(out)).toEqual([
      { scope: 'global', key: 'user.name', value: 'A B' },
      { scope: 'local', key: 'core.sshcommand', value: 'ssh -i x' },
      { scope: 'local', key: 'core.bare', value: null },
      { scope: 'worktree', key: 'remote.o.url', value: 'u\nrl' }
    ])
  })

  it('--show-scope 없는 git config --<scope> --list -z 출력을 호출자가 정한 scope 로 파싱한다', () => {
    expect(parseScopedConfigList('core.bare\0Core.SshCommand\nssh -i x\0remote.o.url\nu\nrl\0', 'local')).toEqual([
      { scope: 'local', key: 'core.bare', value: null },
      { scope: 'local', key: 'core.sshcommand', value: 'ssh -i x' },
      { scope: 'local', key: 'remote.o.url', value: 'u\nrl' }
    ])
    expect(parseScopedConfigList('', 'global')).toEqual([])
  })

  // 허용 목록 모델: 목록에 없는 키는 전부 "알 수 없는 설정"이다
  it.each([
    'core.sshcommand',
    'core.gitproxy',
    'core.askpass',
    'core.alternaterefscommand',
    'core.editor',
    'core.pager',
    'fetch.prunetags',
    'remote.origin.prunetags',
    'credential.helper',
    'credential.https://example.com.helper',
    'credential.trace',
    'credential.tracesecrets',
    'credential.https://example.com.provider',
    'credential.interactive',
    'credential.username',
    'remote.origin.uploadpack',
    'remote.origin.receivepack',
    'remote.origin.vcs',
    'remote.my.fork.proxy',
    'protocol.ext.allow',
    'protocol.allow',
    'protocol.version',
    'url.ext::sh -c x%s.insteadof',
    'url.https://x/.pushinsteadof',
    'uploadpack.packobjectshook',
    'http.proxy',
    'http.version',
    'http.https://example.com/.sslcainfo',
    'http.extraheader',
    'http.emptyauth',
    'http.proactiveauth',
    'http.delegation',
    'http.allowntlmauth',
    'http.schannelcheckrevoke',
    'http.schannelusesslcainfo',
    'http.followredirects',
    'ssh.variant',
    'trace2.eventtarget',
    'trace2.normaltarget',
    'trace2.perftarget',
    'fetch.bundleuri',
    'fetch.bundlecreationtoken',
    'transfer.bundleuri',
    'alias.st',
    'diff.external',
    'merge.tool',
    'gui.wmstate',
    'color.ui',
    'some.future.key'
  ])('알 수 없는 설정: %s', (key) => expect(isAllowedConfig(key, 'x')).toBe(false))

  it.each([
    ['user.name', 'a'],
    ['user.email', 'a@b'],
    ['core.bare', 'false'],
    ['core.repositoryformatversion', '0'],
    ['core.filemode', 'false'],
    ['core.autocrlf', 'true'],
    ['core.hookspath', '.husky'],
    ['core.fsmonitor', 'true'],
    ['core.fsmonitor', 'false'],
    ['remote.origin.url', 'https://h/r.git'],
    ['remote.my.fork.fetch', '+refs/heads/*:refs/remotes/my.fork/*'],
    ['remote.origin.promisor', 'true'],
    ['remote.origin.partialclonefilter', 'blob:none'],
    ['branch.main.remote', 'origin'],
    ['branch.feature/x.merge', 'refs/heads/main'],
    ['branch.main.vscode-merge-base', 'origin/main'],
    ['branch.main.github-pr-owner-number', 'o#r#1'],
    ['pull.rebase', 'true'],
    ['fetch.prune', 'true'],
    ['extensions.partialclone', 'origin'],
    ['extensions.worktreeconfig', 'true'],
    ['lfs.repositoryformatversion', '0'],
    ['filter.lfs.clean', 'git-lfs clean -- %f'],
    ['filter.lfs.smudge', 'git-lfs smudge -- %f'],
    ['filter.lfs.process', 'git-lfs filter-process'],
    ['filter.lfs.required', 'true'],
    ['submodule.lib.url', 'https://h/lib.git'],
    ['submodule.active', '.'],
    ['include.path', '../x'],
    ['includeif.gitdir:~/w/.path', '~/w.cfg']
  ])('허용 키: %s = %s', (key, value) => expect(isAllowedConfig(key, value)).toBe(true))

  it.each([
    ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*', true],
    ['remote.origin.fetch', 'refs/heads/*:refs/remotes/origin/*', true],
    ['remote.origin.fetch', '+refs/heads/main:refs/remotes/origin/main', true],
    ['remote.my.fork.fetch', '+refs/heads/*:refs/remotes/my.fork/*', true],
    ['remote.origin.fetch', '+refs/heads/feature:refs/heads/feature', false],
    ['remote.origin.fetch', '+refs/heads/main:refs/stash', false],
    ['remote.origin.fetch', '+refs/heads/main:refs/replace/abc', false],
    ['remote.origin.fetch', '+refs/heads/main:HEAD', false],
    ['remote.origin.fetch', '+refs/heads/none/*:refs/heads/*', false],
    ['remote.origin.fetch', '+refs/heads/*:refs/remotes/other/*', false],
    ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/x/*', false],
    ['remote.origin.fetch', '+refs/heads/a:refs/remotes/origin/b', false],
    ['remote.origin.fetch', '+refs/tags/*:refs/tags/*', false],
    ['remote.origin.fetch', '+refs/*:refs/remotes/origin/*', false],
    ['remote.origin.fetch', '^refs/heads/x', false],
    ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*:refs/heads/x', false]
  ])('remote 설정의 fetch refspec: %s = %s → 허용 %s', (key, value, ok) => expect(isAllowedConfig(key, value)).toBe(ok))

  it('값까지 맞아야 허용하는 키: git-lfs 기본값이 아니거나 fsmonitor 가 명령이면 알 수 없는 설정', () => {
    expect(isAllowedConfig('filter.lfs.clean', 'evil --x')).toBe(false)
    expect(isAllowedConfig('filter.lfs.process', 'sh -c "touch x"')).toBe(false)
    expect(isAllowedConfig('filter.lfs.required', 'false')).toBe(false)
    expect(isAllowedConfig('core.fsmonitor', '.git/hooks/fsmonitor-watchman')).toBe(false)
    expect(isAllowedConfig('core.bare', 'true')).toBe(false)
    expect(isAllowedConfig('filter.lfs.clean', null)).toBe(false)
    expect(isAllowedConfig('filter.other.clean', 'git-lfs clean -- %f')).toBe(false)
  })

  it('거부할 remote URL', () => {
    expect(unsafeRemoteUrl('ext::sh -c "touch /tmp/x"')).toMatch(/ext::/)
    expect(unsafeRemoteUrl('fd::17')).toMatch(/fd::/)
    expect(unsafeRemoteUrl('foo::bar')).toMatch(/foo::/)
    expect(unsafeRemoteUrl('-oProxyCommand=x')).not.toBeNull()
    expect(unsafeRemoteUrl('ssh://-oProxyCommand=x/repo')).not.toBeNull()
    expect(unsafeRemoteUrl('ssh://git@-host/repo')).not.toBeNull()
    for (const ok of ['https://github.com/a/b.git', 'git@github.com:a/b.git', 'file:///tmp/r.git', '../r.git', 'C:/r.git', 'ssh://git@host/r']) {
      expect(unsafeRemoteUrl(ok)).toBeNull()
    }
  })

  it('사용자 전역 core.sshCommand 는 존중하고, 저장소 안의 것은 위험 항목으로 본다', () => {
    const g = inspectEntries(parseConfigList('global\0core.sshcommand\nssh -F x\0local\0remote.o.url\nhttps://h/r\0'))
    expect(g.userSshCommand).toBe('ssh -F x')
    expect(g.dangerous).toEqual([])
    expect(g.hasRemote).toBe(true)
    const l = inspectEntries(parseConfigList('local\0core.sshcommand\nevil\0'))
    expect(l.userSshCommand).toBeNull()
    expect(l.dangerous.map((d) => d.key)).toEqual(['core.sshcommand'])
  })

  it('fetch 환경: ext/fd 제외, file 은 허용할 때만, 프롬프트 금지, GIT_SSH_COMMAND 는 항상 명시', () => {
    const env = fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: false })
    expect(env['GIT_ALLOW_PROTOCOL']).toBe('https:http:ssh:git')
    expect(fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: true })['GIT_ALLOW_PROTOCOL']).toBe('https:http:ssh:git:file')
    expect(env['GIT_TERMINAL_PROMPT']).toBe('0')
    expect(env['GIT_SSH_COMMAND']).toContain('BatchMode=yes')
  })

  it('ssh 명령 선택: 사용자 설정 > 환경변수 GIT_SSH_COMMAND > GIT_SSH > BatchMode 기본(저장소 설정은 어디에도 없다)', () => {
    const none = { userSshCommand: null }
    expect(resolveSshCommand({ userSshCommand: 'ssh -F my' }, 'local', { GIT_SSH_COMMAND: 'env-ssh' })).toBe('ssh -F my')
    expect(resolveSshCommand(none, 'local', { GIT_SSH_COMMAND: 'env-ssh', GIT_SSH: '/x/ssh' })).toBe('env-ssh')
    expect(resolveSshCommand(none, 'local', { GIT_SSH: '/opt/my ssh/ssh' }, 'linux')).toBe("'/opt/my ssh/ssh'")
    expect(resolveSshCommand(none, 'local', { GIT_SSH: 'C:\\Program Files\\PuTTY\\plink.exe' }, 'win32')).toBe("'C:/Program Files/PuTTY/plink.exe'")
    expect(resolveSshCommand(none, 'local', {})).toBe(SSH_DEFAULT)
    // 원격 환경(WSL·SSH)에서는 이 프로세스의 환경변수가 의미 없다
    expect(resolveSshCommand(none, 'wsl', { GIT_SSH_COMMAND: 'env-ssh' })).toBe(SSH_DEFAULT)
  })

  it('git 버전 파싱·비교(file 전송은 2.45.1 이상에서만)', () => {
    expect(parseGitVersion('git version 2.49.0.windows.1\n')).toEqual([2, 49, 0])
    expect(parseGitVersion('git version 2.43.0')).toEqual([2, 43, 0])
    expect(parseGitVersion('git version 2.39')).toEqual([2, 39, 0])
    expect(parseGitVersion('weird')).toBeNull()
    expect(versionAtLeast([2, 45, 1], FILE_TRANSPORT_MIN_GIT)).toBe(true)
    expect(versionAtLeast([2, 49, 0], FILE_TRANSPORT_MIN_GIT)).toBe(true)
    expect(versionAtLeast([3, 0, 0], FILE_TRANSPORT_MIN_GIT)).toBe(true)
    expect(versionAtLeast([2, 45, 0], FILE_TRANSPORT_MIN_GIT)).toBe(false)
    expect(versionAtLeast([2, 43, 0], FILE_TRANSPORT_MIN_GIT)).toBe(false)
    expect(versionAtLeast(null, FILE_TRANSPORT_MIN_GIT)).toBe(false)
  })

  it('훅 디렉터리: 로컬은 앱 소유 디렉터리(슬래시), 원격은 /dev/null, Windows 로컬에 디렉터리가 없으면 거부', () => {
    const dir = mkdtempSync(join(tmpdir(), 'antler-nohooks-'))
    try {
      expect(hooksPathFor('local', dir)).toBe(dir.replace(/\\/g, '/'))
      expect(hooksPathFor('wsl', dir)).toBe('/dev/null')
      expect(hooksPathFor('ssh', null)).toBe('/dev/null')
      expect(hooksPathFor('local', null, 'linux')).toBe('/dev/null')
      expect(() => hooksPathFor('local', null, 'win32')).toThrow(/훅 차단/)
      // 비어 있지 않거나 없으면 거부
      writeFileSync(join(dir, 'reference-transaction'), '')
      expect(() => hooksPathFor('local', dir)).toThrow(/비어 있지 않아/)
      expect(() => hooksPathFor('local', join(dir, 'missing'))).toThrow(/없거나/)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it('백오프: 주기 → 2배씩 → 30분 상한', () => {
    expect(nextDelayMs(5, 0)).toBe(5 * 60_000)
    expect(nextDelayMs(5, 1)).toBe(10 * 60_000)
    expect(nextDelayMs(5, 2)).toBe(20 * 60_000)
    expect(nextDelayMs(5, 3)).toBe(30 * 60_000)
    expect(nextDelayMs(1, 20)).toBe(30 * 60_000)
    expect(nextDelayMs(60, 5)).toBe(60 * 60_000)
  })

  it('오류 분류: 인증 실패는 auth, 허용 안 된 전송은 protocol, 나머지는 network', () => {
    expect(classifyFailure(new Error("fatal: transport 'file' not allowed"))).toBe('protocol')
    expect(classifyFailure(new Error('fatal: Authentication failed for https://x'))).toBe('auth')
    expect(classifyFailure(new Error('git@h: Permission denied (publickey).'))).toBe('auth')
    expect(classifyFailure(new Error('fatal: could not read Username for https://x: terminal prompts disabled'))).toBe('auth')
    expect(classifyFailure(new Error('fatal: unable to access: Could not resolve host'))).toBe('network')
  })

  it('fetch 환경변수가 WSL·SSH 실행 명령에 실린다', () => {
    const env = fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: true })
    expect(wslArgv('Ubuntu', 'r', ['fetch'], env)).toContain('GIT_ALLOW_PROTOCOL=https:http:ssh:git:file')
    expect(wslArgv('Ubuntu', 'r', ['fetch'])).toContain('GIT_ALLOW_PROTOCOL=none')
    const cmd = buildRemoteCommand('r', ['fetch'], { GIT_ALLOW_PROTOCOL: 'none', ...env })
    expect(cmd).toContain(`GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=15'`)
    expect(cmd).toContain("GIT_ASKPASS=''")
  })
})

// ── 실제 git ──────────────────────────────────────────────────────────────

// fetch 한 번에 git 을 열 번 안팎 띄우고(검사 2회 포함) 각 테스트가 fetch 를 여러 번 하므로 기본 20초로는 부족하다
vi.setConfig({ testTimeout: 90_000 })

let root: string
const exec = new LocalExecutor()
const ID = ['-c', 'user.name=t', '-c', 'user.email=t@t']
const git = (cwd: string, ...a: string[]): string => execFileSync('git', ['-C', cwd, ...ID, ...a], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const posix = (p: string): string => p.replace(/\\/g, '/')

let seq = 0
/** bare remote(file://) + 시드 작업 복제본 + 테스트 대상 복제본 */
function makeRepos(): { bare: string; work: string; repo: string; marker: string; pushCommit: (msg: string) => string } {
  const dir = join(root, `case-${seq++}`)
  mkdirSync(dir)
  const bare = join(dir, 'remote.git')
  const work = join(dir, 'work')
  const repo = join(dir, 'repo')
  execFileSync('git', ['init', '-q', '--bare', '-b', 'main', bare])
  execFileSync('git', ['clone', '-q', pathToFileURL(bare).href, work], { stdio: 'ignore' })
  git(work, 'checkout', '-q', '-b', 'main')
  git(work, 'commit', '-q', '--allow-empty', '-m', 'first')
  git(work, 'push', '-q', 'origin', 'main')
  execFileSync('git', ['clone', '-q', pathToFileURL(bare).href, repo], { stdio: 'ignore' })
  const pushCommit = (msg: string): string => {
    git(work, 'commit', '-q', '--allow-empty', '-m', msg)
    git(work, 'push', '-q', 'origin', 'main')
    return git(work, 'rev-parse', 'HEAD')
  }
  return { bare, work, repo, marker: join(dir, 'PWNED'), pushCommit }
}

/** 실행되면 마커 파일을 만드는 무해한 페이로드(git 이 셸로 실행하는 설정 값) */
const payload = (marker: string): string => `node -e "require('fs').writeFileSync(process.argv[1],'x')" "${posix(marker)}"`

let svc: RepoService | null = null
let fetcher: FetchService | null = null
const statuses: FetchStatus[] = []
const changed: string[] = []

function setup(repo: string): { fetcher: FetchService; repoId: string; config: ConfigStore } {
  const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
  const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
  svc = new RepoService(config)
  fetcher = new FetchService(config, svc, (s) => statuses.push(s), (id) => changed.push(id), noHooks)
  return { fetcher, repoId: r.id, config }
}

let noHooks: string
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-fetch-'))
  noHooks = join(root, 'no-hooks')
})
afterEach(async () => {
  fetcher?.dispose()
  fetcher = null
  await svc?.dispose()
  svc = null
  statuses.length = 0
  changed.length = 0
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('안전한 저장소의 fetch', () => {
  it('bare remote 에 새 커밋이 올라오면 origin/main 이 갱신되고 변경을 알린다', async () => {
    const { bare, repo, pushCommit } = makeRepos()
    void bare
    const { fetcher, repoId } = setup(repo)
    const sha = pushCommit('second')
    expect(git(repo, 'rev-parse', 'origin/main')).not.toBe(sha)
    const st = await fetcher.fetch(repoId, { manual: true })
    expect(st.error).toBeNull()
    expect(st.blocked).toBeNull()
    expect(st.fetching).toBe(false)
    expect(st.lastOkAt).not.toBeNull()
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(sha)
    expect(changed).toEqual([repoId])
    // --no-write-fetch-head
    expect(existsSync(join(repo, '.git', 'FETCH_HEAD'))).toBe(false)
  })

  it('--prune: remote 에서 지운 브랜치의 원격 추적 ref 를 정리한다', async () => {
    const { work, repo } = makeRepos()
    git(work, 'push', '-q', 'origin', 'main:topic')
    const { fetcher, repoId } = setup(repo)
    await fetcher.fetch(repoId, { manual: true })
    expect(git(repo, 'branch', '-r')).toContain('origin/topic')
    git(work, 'push', '-q', 'origin', '--delete', 'topic')
    await fetcher.fetch(repoId, { manual: true })
    expect(git(repo, 'branch', '-r')).not.toContain('origin/topic')
  })

  it('같은 저장소의 동시 fetch 는 하나로 합쳐진다', async () => {
    const { repo } = makeRepos()
    const { fetcher, repoId } = setup(repo)
    const a = fetcher.fetch(repoId, { manual: false })
    const b = fetcher.fetch(repoId, { manual: true })
    expect(b).toBe(a)
    await a
    expect(statuses.filter((s) => s.fetching)).toHaveLength(1)
  })

  it('remote 가 없으면 대상이 아니다', async () => {
    const dir = join(root, `norem-${seq++}`)
    execFileSync('git', ['init', '-q', dir])
    const { fetcher, repoId } = setup(dir)
    const st = await fetcher.fetch(repoId, { manual: false })
    expect(st.noRemote).toBe(true)
    expect(st.error).toBeNull()
  })

  it('저장소 .git/hooks 의 reference-transaction 훅은 실행되지 않는다', async () => {
    const { repo, marker, pushCommit } = makeRepos()
    const hook = join(repo, '.git', 'hooks', 'reference-transaction')
    writeFileSync(hook, `#!/bin/sh\necho x > "${posix(marker)}"\n`, { mode: 0o755 })
    const { fetcher, repoId } = setup(repo)
    const sha = pushCommit('second')
    const st = await fetcher.fetch(repoId, { manual: true })
    expect(st.error).toBeNull()
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(sha)
    expect(existsSync(marker)).toBe(false)
    // 대조: -c core.hooksPath 없이 직접 fetch 하면 훅이 실제로 실행된다(테스트가 의미 있는지 확인)
    git(repo, 'update-ref', '-d', 'refs/remotes/origin/main')
    git(repo, 'fetch', '-q', 'origin')
    expect(existsSync(marker)).toBe(true)
  })
})

describe('악성 저장소 설정', () => {
  const cases: [string, (marker: string) => [string, string]][] = [
    ['core.sshCommand', (m) => ['core.sshCommand', payload(m)]],
    ['credential.helper', (m) => ['credential.helper', `!${payload(m)}`]],
    ['remote.origin.uploadpack', (m) => ['remote.origin.uploadpack', payload(m)]]
  ]

  it.each(cases)('%s 가 있으면 자동 fetch 를 건너뛰고 아무것도 실행하지 않는다', async (name, make) => {
    const { repo, marker, pushCommit } = makeRepos()
    const [key, value] = make(marker)
    git(repo, 'config', key, value)
    const { fetcher, repoId } = setup(repo)
    const before = git(repo, 'rev-parse', 'origin/main')
    pushCommit('second')
    const st = await fetcher.fetch(repoId, { manual: false })
    expect(st.blocked?.confirmable).toBe(true)
    expect(st.blocked?.keys.map((k) => k.toLowerCase())).toContain(name.toLowerCase())
    expect(existsSync(marker)).toBe(false)
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(before)
    // 자동 fetch 는 막힌 저장소를 다시 예약하지 않는다
    fetcher.setActive(repoId)
    expect(fetcher.status(repoId).blocked).not.toBeNull()
  })

  it('수동 새로고침: 거절하면 막힌 채로 두고, 허용하면 같은 설정에 한해 한 번만 묻는다', async () => {
    const { repo, marker, pushCommit } = makeRepos()
    // 무해한 위험 키(프로토콜 정책)로 허용 경로를 검증한다
    git(repo, 'config', 'protocol.ext.allow', 'never')
    const { fetcher, repoId } = setup(repo)
    const sha = pushCommit('second')
    const asked: string[][] = []
    let answer = false
    const confirm = async (_id: string, items: { key: string; value: string }[]): Promise<boolean> => {
      asked.push(items.map((i) => i.key))
      return answer
    }
    let st = await fetcher.fetch(repoId, { manual: true, confirm })
    expect(st.blocked?.confirmable).toBe(true)
    expect(asked).toEqual([['protocol.ext.allow']])
    expect(git(repo, 'rev-parse', 'origin/main')).not.toBe(sha)

    answer = true
    st = await fetcher.fetch(repoId, { manual: true, confirm })
    expect(st.blocked).toBeNull()
    expect(st.error).toBeNull()
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(sha)

    // 허용한 설정은 다시 묻지 않는다. 설정이 바뀌면 다시 묻는다
    await fetcher.fetch(repoId, { manual: true, confirm })
    expect(asked).toHaveLength(2)
    git(repo, 'config', 'protocol.file.allow', 'always')
    await fetcher.fetch(repoId, { manual: true, confirm })
    expect(asked).toHaveLength(3)
    expect(existsSync(marker)).toBe(false)
  })

  it('"항상 허용"은 앱을 다시 켜도(새 FetchService) 묻지 않고, 설정이 바뀌면 다시 묻고, 저장소를 지우면 사라진다', async () => {
    const { repo, pushCommit } = makeRepos()
    git(repo, 'config', 'protocol.ext.allow', 'never')
    const { fetcher: first, repoId, config } = setup(repo)
    let asked = 0
    const always = async (): Promise<'always'> => {
      asked++
      return 'always'
    }
    expect((await first.fetch(repoId, { manual: true, confirm: always })).blocked).toBeNull()
    expect(asked).toBe(1)
    first.dispose()

    // 다시 켠 앱: 확인 없이 자동 fetch 까지 통과한다(file remote 라 자동 fetch 는 전송 정책에서만 실패한다)
    const second = new FetchService(config, svc!, () => {}, () => {}, noHooks)
    fetcher = second
    const sha = pushCommit('second')
    expect((await second.fetch(repoId, { manual: false })).blocked).toBeNull()
    const never = async (): Promise<boolean> => {
      asked++
      return false
    }
    expect((await second.fetch(repoId, { manual: true, confirm: never })).blocked).toBeNull()
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(sha)
    expect(asked).toBe(1)

    // 설정이 바뀌면 지문이 달라져 다시 묻는다
    git(repo, 'config', 'protocol.file.allow', 'always')
    expect((await second.fetch(repoId, { manual: true, confirm: never })).blocked?.confirmable).toBe(true)
    expect(asked).toBe(2)

    // renderer 가 보낼 수 있는 패치로는 바꿀 수 없고, 저장소를 지우면 지운다
    const before = config.trustedConfig(repoId)
    config.setUi({ trustedConfigs: { [repoId]: 'forged' } } as never)
    expect(config.trustedConfig(repoId)).toBe(before)
    config.removeRepo(repoId)
    expect(config.trustedConfig(repoId)).toBeUndefined()
  })

  it('include 로 끌어온 파일의 위험 설정도 잡는다', async () => {
    const { repo, marker } = makeRepos()
    const inc = join(root, `inc-${seq++}.gitconfig`)
    writeFileSync(inc, `[core]\n\tsshCommand = ${payload(marker).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}\n`)
    git(repo, 'config', 'include.path', posix(inc))
    const ins = await inspectConfig(exec, repo)
    expect(ins.dangerous.map((d) => d.key)).toContain('core.sshcommand')
    expect(existsSync(marker)).toBe(false)
  })

  it.each([
    ['ext::', (m: string) => `ext::sh -c "${posix(payload(m)).replace(/"/g, '\\"')}"`],
    ['옵션처럼 보이는 URL', () => '-oProxyCommand=echo']
  ])('remote URL %s 는 허용 확인 없이 거부한다', async (_n, url) => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'remote.origin.url', url(marker))
    const { fetcher, repoId } = setup(repo)
    const st = await fetcher.fetch(repoId, { manual: true, confirm: async () => true })
    expect(st.blocked?.confirmable).toBe(false)
    expect(st.blocked?.keys[0]).toContain('remote.origin.url')
    expect(existsSync(marker)).toBe(false)
  })

  it('검사를 우회해 직접 호출해도 GIT_ALLOW_PROTOCOL 이 ext:: 를 막는다', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'remote.origin.url', `ext::sh -c "touch ${posix(marker)}"`)
    const r = await exec.run(repo, ['fetch', 'origin'], { env: fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: true }) })
    expect(r.code).not.toBe(0)
    expect(existsSync(marker)).toBe(false)
  })
})

describe('훅 디렉터리(Windows 의 /dev/null 문제)', () => {
  const hookPath = (repo: string, hooksPath: string): string =>
    posix(execFileSync('git', ['-C', repo, ...fetchConfigArgs(hooksPath), 'rev-parse', '--git-path', 'hooks/reference-transaction'], { encoding: 'utf8' }).trim())

  it('앱 소유 디렉터리를 쓰면 훅 경로가 그 안으로 풀린다(저장소 드라이브의 \\dev\\null 이 아니다)', () => {
    const { repo } = makeRepos()
    mkdirSync(noHooks, { recursive: true })
    const hp = hooksPathFor('local', noHooks)
    const resolved = hookPath(repo, hp)
    expect(resolved.toLowerCase().startsWith(posix(noHooks).toLowerCase())).toBe(true)
    expect(resolved).not.toContain('dev/null')
    if (process.platform === 'win32') {
      // 대조: 옛 방식. git.exe 는 /dev/null 을 그대로 받아 저장소 드라이브 기준 경로로 푼다 — 여기에 훅을 심을 수 있다
      expect(hookPath(repo, '/dev/null')).toContain('dev/null')
    }
  })

  it('디렉터리가 비어 있지 않으면 fetch 를 거부한다', async () => {
    const { repo } = makeRepos()
    const { fetcher, repoId } = setup(repo)
    writeFileSync(join(noHooks, 'reference-transaction'), '')
    try {
      const st = await fetcher.fetch(repoId, { manual: true })
      expect(st.error).toMatch(/훅 차단/)
      expect(st.lastOkAt).toBeNull()
    } finally {
      rmSync(join(noHooks, 'reference-transaction'), { force: true })
    }
  })
})

describe('테스트 격리', () => {
  it('git config --global 은 사용자의 ~/.gitconfig 가 아니라 임시 파일을 읽는다', () => {
    const tmpGlobal = process.env['GIT_CONFIG_GLOBAL']
    expect(tmpGlobal).toBeTruthy()
    expect(posix(tmpGlobal!).toLowerCase().startsWith(posix(tmpdir()).toLowerCase())).toBe(true)
    expect(posix(tmpGlobal!).toLowerCase().startsWith(posix(homedir()).toLowerCase() + '/.gitconfig')).toBe(false)
    expect(process.env['GIT_CONFIG_NOSYSTEM']).toBe('1')
    expect(process.env['GCM_INTERACTIVE']).toBe('never')
    // git 이 실제로 그 파일만 본다: setup 이 넣은 테스트 식별자 말고는 항목이 없다
    // (사용자·시스템 설정이 섞이면 credential.helper=manager 등이 보인다)
    const keys = execFileSync('git', ['config', '--global', '--list', '--name-only'], { encoding: 'utf8' }).trim().split('\n').sort()
    expect(keys).toEqual(['user.email', 'user.name'])
    execFileSync('git', ['config', '--global', 'antler.isolation-probe', '1'])
    const probe = execFileSync('git', ['config', '--global', '--list', '--show-origin'], { encoding: 'utf8' })
    // --show-origin 은 경로의 슬래시를 겹쳐 쓴다(file:"c://users//…")
    expect(posix(probe).replace(/\/+/g, '/').toLowerCase()).toContain(posix(tmpGlobal!).replace(/\/+/g, '/').toLowerCase())
    execFileSync('git', ['config', '--global', '--unset', 'antler.isolation-probe'])
  })
})

/** 모든 요청에 401 과 통합 인증 챌린지를 돌려주는 루프백 서버. 받은 Authorization 헤더를 기록한다 */
async function authServer(challenge: string[]): Promise<{ port: number; seen: (string | undefined)[]; close(): Promise<void> }> {
  const seen: (string | undefined)[] = []
  const server = createServer((req, res) => {
    seen.push(req.headers['authorization'])
    res.writeHead(401, { 'WWW-Authenticate': challenge })
    res.end()
  })
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
  return { port: (server.address() as AddressInfo).port, seen, close: () => new Promise<void>((r) => server.close(() => r())) }
}
const integrated = (h: (string | undefined)[]): (string | undefined)[] => h.filter((a) => a && /^(NTLM|Negotiate)/i.test(a))

describe('Windows 통합 인증(NTLM/Negotiate) 유출', () => {
  it('공격자 http remote 가 401 NTLM·Negotiate 를 요구해도 Authorization 헤더를 보내지 않는다', async () => {
    const srv = await authServer(['NTLM', 'Negotiate'])
    try {
      const { repo } = makeRepos()
      git(repo, 'config', 'remote.origin.url', `http://127.0.0.1:${srv.port}/x.git`)
      mkdirSync(noHooks, { recursive: true })
      const args = fetchConfigArgs(hooksPathFor('local', noHooks))
      const env = fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: false })

      const r = await exec.run(repo, [...args, 'fetch', 'origin'], { env })
      expect(r.code).not.toBe(0)
      expect(srv.seen.length).toBeGreaterThan(0)
      expect(integrated(srv.seen)).toEqual([])

      // 대조: http/credential.allowWindowsAuth 덮어쓰기를 뺀 인자. 이 환경에서 인증 헤더를 보내는지 기록만 한다
      srv.seen.length = 0
      await exec.run(repo, [...args.slice(0, args.indexOf('http.emptyAuth=false') - 1), 'fetch', 'origin'], { env })
      console.log(`[대조] 덮어쓰기 없이 보낸 통합 인증 헤더: ${integrated(srv.seen).length}개 (요청 ${srv.seen.length}개)`)
    } finally {
      await srv.close()
    }
  })

  it.each(['http://user@127.0.0.1', 'http://user:@127.0.0.1'])('userinfo 가 있는 URL(%s)은 자동 fetch 를 막고, 값은 가려서 보여 준다', async (base) => {
    const srv = await authServer(['NTLM', 'Negotiate'])
    try {
      const { repo } = makeRepos()
      git(repo, 'config', 'remote.origin.url', `${base}:${srv.port}/x.git`)
      const ins = await inspectConfig(exec, repo)
      expect(ins.dangerous.map((d) => d.key)).toEqual(['remote.origin.url'])
      expect(ins.dangerous[0]!.value).toContain('<userinfo>@')
      expect(ins.dangerous[0]!.value).not.toMatch(/\/\/user/)
      const { fetcher, repoId } = setup(repo)
      const st = await fetcher.fetch(repoId, { manual: false })
      expect(st.blocked?.confirmable).toBe(true)
      expect(st.blocked?.keys).toEqual(['remote.origin.url'])
      expect(srv.seen).toEqual([]) // 요청 자체가 나가지 않았다

      // 대조: 막지 않고 우리 인자로 직접 실행하면 userinfo URL 은 통합 인증 헤더가 나갈 수 있다(기록만)
      mkdirSync(noHooks, { recursive: true })
      await exec.run(repo, [...fetchConfigArgs(hooksPathFor('local', noHooks)), 'fetch', 'origin'], {
        env: fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: false })
      })
      console.log(`[대조] ${base} 직접 실행: 통합 인증 헤더 ${integrated(srv.seen).length}개 (요청 ${srv.seen.length}개)`)
    } finally {
      await srv.close()
    }
  })
})

describe('자동 fetch 의 자격 증명 도우미(수동 fetch 승인)', () => {
  const globalConfig = (...a: string[]): string => execFileSync('git', ['config', '--global', ...a], { encoding: 'utf8' })
  const helperValue = (marker: string): string => `!node -e "require('fs').writeFileSync(process.argv[1],'x')" "${posix(marker)}"`

  it('승인 전 자동 fetch 는 도우미를 부르지 않고, 수동 fetch 로 승인된 remote 만 부른다. URL 이 바뀌면 승인이 사라진다', async () => {
    const srv = await authServer(['Basic realm="x"'])
    try {
      const { repo, marker } = makeRepos()
      const helperMarker = marker + '.helper'
      globalConfig('--add', 'credential.helper', helperValue(helperMarker))
      git(repo, 'config', 'remote.origin.url', `http://127.0.0.1:${srv.port}/x.git`)
      const { fetcher, repoId, config } = setup(repo)

      // ① 승인 전: 도우미 없음(그리고 인증 실패로 자동 fetch 가 멈추고, 수동으로 한 번 누르라는 안내 플래그가 선다)
      let st = await fetcher.fetch(repoId, { manual: false })
      expect(existsSync(helperMarker)).toBe(false)
      expect(st.authPaused).toBe(true)
      expect(st.needsManual).toBe(true)

      // ② 승인된 remote(지문 일치): 자동 fetch 도 도우미를 쓴다
      const ins = await inspectConfig(exec, repo)
      config.setFetchApproval(repoId, ins.remotesFingerprint)
      st = await fetcher.fetch(repoId, { manual: false })
      expect(existsSync(helperMarker)).toBe(true)
      expect(st.needsManual).toBe(false)

      // ③ URL 이 바뀌면 승인이 무효 — 도우미가 다시 꺼진다
      rmSync(helperMarker, { force: true })
      git(repo, 'config', 'remote.origin.url', `http://127.0.0.1:${srv.port}/other.git`)
      st = await fetcher.fetch(repoId, { manual: false })
      expect(existsSync(helperMarker)).toBe(false)
      expect(st.needsManual).toBe(true)
    } finally {
      globalConfig('--unset-all', 'credential.helper')
      await srv.close()
    }
  })

  it('수동 fetch 가 성공하면 그 remote 가 승인되고, renderer 가 보낼 수 있는 설정 패치로는 승인을 바꿀 수 없다', async () => {
    const { repo } = makeRepos()
    const { fetcher, repoId, config } = setup(repo)
    expect(config.fetchApproval(repoId)).toBeUndefined()
    await fetcher.fetch(repoId, { manual: false }) // file remote 라 자동은 실패 — 승인 없음
    expect(config.fetchApproval(repoId)).toBeUndefined()
    const st = await fetcher.fetch(repoId, { manual: true })
    expect(st.error).toBeNull()
    const ins = await inspectConfig(exec, repo)
    expect(config.fetchApproval(repoId)).toBe(ins.remotesFingerprint)
    // config:setUi 는 ui 만 병합한다 — 최상위 fetchApprovals 는 건드릴 수 없다
    const before = config.fetchApproval(repoId)
    config.setUi({ fetchApprovals: { [repoId]: 'forged' } } as never)
    expect(config.fetchApproval(repoId)).toBe(before)
    // 저장소를 지우면 승인도 지운다
    config.removeRepo(repoId)
    expect(config.fetchApproval(repoId)).toBeUndefined()
  })
})

describe('확인 창 표시 안전', () => {
  it('제어·방향·폭 없는 문자를 \\uXXXX 로 드러내고 따옴표로 감싼다', () => {
    const ch = (...c: number[]): string => String.fromCharCode(...c)
    expect(quoteForDisplay('a\nb')).toBe('"a\\u000ab"')
    expect(quoteForDisplay(`x${ch(0x202e)}y`)).toBe('"x\\u202ey"')
    expect(quoteForDisplay(`x${ch(0x200b)}y${ch(0x200e)}z${ch(0x2066)}w${ch(0x2069)}`)).toBe('"x\\u200by\\u200ez\\u2066w\\u2069"')
    expect(quoteForDisplay(`c1${ch(0x85, 0x9f)}`)).toBe('"c1\\u0085\\u009f"')
    expect(quoteForDisplay(`bom${ch(0xfeff)}`)).toBe('"bom\\ufeff"')
    expect(quoteForDisplay('q"uote')).toBe('"q\\"uote"')
    // Cf(소프트 하이픈·ALM)·U+180E·DEL·태그 문자(BMP 밖)·짝 없는 서로게이트
    expect(quoteForDisplay(`a${ch(0x00ad)}b${ch(0x061c)}c${ch(0x180e)}d${ch(0x7f)}`)).toBe('"a\\u00adb\\u061cc\\u180ed\\u007f"')
    expect(quoteForDisplay(String.fromCodePoint(0xe0041))).toBe('"\\u{e0041}"')
    expect(quoteForDisplay(`x${ch(0xd800)}y`)).toBe('"x\\ud800y"')
    expect(quoteForDisplay('안녕 😀')).toBe('"안녕 😀"') // 보이는 문자는 그대로
  })

  it('긴 값은 앞…뒤와 전체 길이를 보여 준다', () => {
    const v = `${'a'.repeat(60)}HIDDEN${'z'.repeat(60)}`
    const shown = display(v)
    expect(shown).not.toContain('HIDDEN')
    expect(shown).toContain('…')
    expect(shown).toContain(`(길이 ${v.length})`)
    expect(display('short')).toBe('"short"')
  })

  it('한 번에 보여 줄 항목 수 한도', () => expect(MAX_CONFIRM_ITEMS).toBe(12))
})

describe('stderr 상한', () => {
  const spec = (code: string) => ({ command: process.execPath, argv: ['-e', code], gitArgs: ['x'] })

  it('수집기는 앞부분만 4MB 까지 담는다', () => {
    const c = stderrCollector()
    c.push(Buffer.from('HEAD'))
    c.push(Buffer.alloc(MAX_STDERR, 'x'))
    c.push(Buffer.from('TAIL'))
    const t = c.text()
    expect(t.length).toBe(MAX_STDERR)
    expect(t.startsWith('HEAD')).toBe(true)
    expect(t.includes('TAIL')).toBe(false)
  })

  it('runProcess: 10MB 를 써도 stderr 는 4MB 로 잘리고 프로세스는 정상 종료한다', async () => {
    const code = "process.stderr.write('HEAD');for(let i=0;i<10;i++)process.stderr.write(Buffer.alloc(1<<20,'x'));"
    const r = await runProcess(spec(code))
    expect(r.code).toBe(0)
    expect(r.stderr.length).toBe(MAX_STDERR)
    expect(r.stderr.startsWith('HEAD')).toBe(true)
  })

  it('streamProcess: 실패 시 GitError 의 stderr 도 4MB 로 잘린다', async () => {
    // POSIX 에서 파이프로 연결된 stderr 는 비동기라 process.exit 이 남은 출력을 버린다 — 동기 쓰기로 6MB 를 다 보낸다
    const code = "const fs=require('fs');for(let i=0;i<6;i++)fs.writeSync(2,Buffer.alloc(1<<20,'x'));process.exit(3)"
    let err: GitError | null = null
    try {
      for await (const _ of streamProcess(spec(code))) void _
    } catch (e) {
      err = e as GitError
    }
    expect(err).toBeInstanceOf(GitError)
    expect(err!.stderr.length).toBe(MAX_STDERR)
  })
})

describe('검사 후 실행 사이의 변경(TOCTOU)과 옛 형식 remote', () => {
  it('확인 중에 위험 설정이 늘어나면 실행 직전 재검사에서 멈춘다', async () => {
    const { repo, marker, pushCommit } = makeRepos()
    git(repo, 'config', 'protocol.ext.allow', 'never')
    const { fetcher, repoId } = setup(repo)
    const before = git(repo, 'rev-parse', 'origin/main')
    pushCommit('second')
    const st = await fetcher.fetch(repoId, {
      manual: true,
      confirm: async () => {
        // 사용자가 허용을 누르는 사이(또는 첫 검사 직후) 저장소 설정이 바뀐다
        git(repo, 'config', 'core.sshCommand', payload(marker))
        return true
      }
    })
    expect(st.blocked?.confirmable).toBe(true)
    expect(st.blocked?.keys).toContain('core.sshcommand')
    expect(existsSync(marker)).toBe(false)
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(before)
  })

  it('확인 중에 remote URL 이 ext:: 로 바뀌면 허용과 무관하게 거부한다', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'protocol.ext.allow', 'never')
    const { fetcher, repoId } = setup(repo)
    const st = await fetcher.fetch(repoId, {
      manual: true,
      confirm: async () => {
        git(repo, 'config', 'remote.origin.url', `ext::sh -c "touch ${posix(marker)}"`)
        return true
      }
    })
    expect(st.blocked?.confirmable).toBe(false)
    expect(existsSync(marker)).toBe(false)
  })

  // git 은 설정에 이름이 있는(remote.<n>.* 키가 하나라도 있는) remote 의 URL 을 옛 파일에서도 읽는다 — 설정 키 목록만으로는 못 본다.
  // 설정에 이름이 전혀 없는 옛 파일 remote 는 `git remote`·`fetch --all` 이 다루지 않는다(아래 마지막 테스트)
  it('.git/remotes/<name> 의 옛 형식 URL 도 검사한다', async () => {
    const dir = join(root, `legacy-${seq++}`)
    execFileSync('git', ['init', '-q', dir])
    git(dir, 'config', 'remote.legacy.fetch', '+refs/heads/*:refs/remotes/legacy/*')
    const marker = join(dir, 'PWNED')
    mkdirSync(join(dir, '.git', 'remotes'))
    writeFileSync(join(dir, '.git', 'remotes', 'legacy'), `URL: ext::sh -c "touch ${posix(marker)}"\nPull: refs/heads/*:refs/remotes/legacy/*\n`)
    const ins = await inspectConfig(exec, dir)
    expect(ins.hasRemote).toBe(true)
    expect(ins.forbidden.map((f) => f.key)).toContain('remote.legacy.url')
    const { fetcher, repoId } = setup(dir)
    const st = await fetcher.fetch(repoId, { manual: true, confirm: async () => true })
    expect(st.blocked?.confirmable).toBe(false)
    expect(existsSync(marker)).toBe(false)
  })

  it('.git/branches/<name> 의 옛 형식 URL 도 검사한다', async () => {
    const dir = join(root, `legacy-${seq++}`)
    execFileSync('git', ['init', '-q', dir])
    git(dir, 'config', 'remote.old.fetch', '+refs/heads/*:refs/remotes/old/*')
    mkdirSync(join(dir, '.git', 'branches'))
    writeFileSync(join(dir, '.git', 'branches', 'old'), 'fd::17\n')
    const ins = await inspectConfig(exec, dir)
    expect(ins.forbidden.map((f) => f.key)).toContain('remote.old.url')
  })

  it('설정에 이름이 없는 옛 파일 remote 는 fetch --all 의 대상이 아니다(fetch 대상 없음으로 처리)', async () => {
    const dir = join(root, `legacy-${seq++}`)
    execFileSync('git', ['init', '-q', dir])
    mkdirSync(join(dir, '.git', 'remotes'))
    writeFileSync(join(dir, '.git', 'remotes', 'only'), 'URL: https://example.invalid/x.git\n')
    const ins = await inspectConfig(exec, dir)
    expect(ins.hasRemote).toBe(false)
  })
})

describe('file 전송과 ssh 명령', () => {
  it('자동 fetch 는 file remote 를 쓰지 않고(자동 재시도도 멈춘다), 수동 fetch 는 git 2.45.1 이상에서 가능하다', async () => {
    const { repo, pushCommit } = makeRepos()
    const { fetcher, repoId } = setup(repo)
    const sha = pushCommit('second')
    const auto = await fetcher.fetch(repoId, { manual: false })
    expect(auto.error).toMatch(/not allowed|file/)
    expect(auto.authPaused).toBe(true)
    expect(git(repo, 'rev-parse', 'origin/main')).not.toBe(sha)
    const manual = await fetcher.fetch(repoId, { manual: true })
    expect(manual.error).toBeNull()
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(sha)
  })

  it('GIT_SSH_COMMAND 를 항상 명시하므로 저장소의 core.sshCommand 는 적용되지 않는다', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'core.sshCommand', payload(marker))
    git(repo, 'config', 'remote.origin.url', 'ssh://127.0.0.1:1/x.git')
    const r = await exec.run(repo, ['fetch', 'origin'], { env: fetchEnv({ sshCommand: SSH_DEFAULT, allowFile: false }) })
    expect(r.code).not.toBe(0)
    expect(existsSync(marker)).toBe(false)
    // 대조: 환경변수 없이는 저장소 설정이 실행된다
    await exec.run(repo, ['fetch', 'origin'], { env: { GIT_ALLOW_PROTOCOL: 'ssh' } })
    expect(existsSync(marker)).toBe(true)
  })
})

describe('허용 목록: 현실적인 저장소 설정은 그대로 통과한다', () => {
  const unknownKeys = async (repo: string): Promise<string[]> => (await inspectConfig(exec, repo)).dangerous.map((d) => d.key)

  it('git clone 직후의 기본 설정', async () => {
    const { repo } = makeRepos()
    expect(await unknownKeys(repo)).toEqual([])
  })

  it('git-lfs(git lfs install --local 이 쓰는 값)·VS Code·GitHub PR 확장·husky·partial clone·서브모듈이 남기는 설정', async () => {
    const { repo } = makeRepos()
    const c = (k: string, v: string): string => git(repo, 'config', k, v)
    c('lfs.repositoryformatversion', '0')
    c('filter.lfs.clean', 'git-lfs clean -- %f')
    c('filter.lfs.smudge', 'git-lfs smudge -- %f')
    c('filter.lfs.process', 'git-lfs filter-process')
    c('filter.lfs.required', 'true')
    c('branch.main.vscode-merge-base', 'origin/main')
    c('branch.main.github-pr-owner-number', 'owner#repo#12')
    c('core.hooksPath', '.husky')
    c('core.fsmonitor', 'true')
    c('core.untrackedcache', 'true')
    c('core.excludesFile', '.gitignore-extra')
    c('user.name', 'Me')
    c('user.email', 'me@example.com')
    c('pull.rebase', 'true')
    c('fetch.prune', 'true')
    c('remote.origin.promisor', 'true')
    c('remote.origin.partialclonefilter', 'blob:none')
    c('extensions.partialclone', 'origin')
    c('submodule.lib.url', 'https://example.com/lib.git')
    c('submodule.lib.active', 'true')
    expect(await unknownKeys(repo)).toEqual([])
    // 자동 fetch 가 막히지 않는다(검사 통과) — file remote 라 전송 정책 때문에 실패할 뿐이다
    const { fetcher, repoId } = setup(repo)
    const st = await fetcher.fetch(repoId, { manual: true })
    expect(st.blocked).toBeNull()
    expect(st.error).toBeNull()
  })

  it('git-lfs 기본값이 아닌 filter.lfs.* 와 명령형 core.fsmonitor 는 알 수 없는 설정으로 걸린다', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'filter.lfs.process', payload(marker))
    git(repo, 'config', 'core.fsmonitor', payload(marker))
    expect((await unknownKeys(repo)).sort()).toEqual(['core.fsmonitor', 'filter.lfs.process'])
    expect(existsSync(marker)).toBe(false)
  })
})

describe('번들 URI·trace2 (허용 목록 밖의 설정)', () => {
  const refsBundles = (repo: string): string => git(repo, 'for-each-ref', 'refs/bundles')

  async function bundleHost(): Promise<{ url: string; hits: string[]; close(): Promise<void> }> {
    const hits: string[] = []
    const server = createServer((req, res) => {
      hits.push(req.url ?? '')
      res.writeHead(404)
      res.end()
    })
    await new Promise<void>((r) => server.listen(0, '127.0.0.1', r))
    const port = (server.address() as AddressInfo).port
    return { url: `http://127.0.0.1:${port}/b.bundle`, hits, close: () => new Promise<void>((r) => server.close(() => r())) }
  }

  it('fetch.bundleURI(로컬 경로): 자동 fetch 는 막히고 수동은 키를 보여 주며, 거절하면 번들을 받지 않는다', async () => {
    const { work, repo } = makeRepos()
    const bundle = join(root, `b-${seq++}.bundle`)
    git(work, 'commit', '-q', '--allow-empty', '-m', 'in-bundle')
    git(work, 'bundle', 'create', bundle, 'main')
    git(repo, 'config', 'fetch.bundleURI', bundle)
    const { fetcher, repoId } = setup(repo)

    const auto = await fetcher.fetch(repoId, { manual: false })
    expect(auto.blocked?.confirmable).toBe(true)
    expect(auto.blocked?.keys).toContain('fetch.bundleuri')
    expect(refsBundles(repo)).toBe('')

    const asked: string[][] = []
    const manual = await fetcher.fetch(repoId, {
      manual: true,
      confirm: async (_id, items) => {
        asked.push(items.map((i) => i.key))
        return false
      }
    })
    expect(asked).toEqual([['fetch.bundleuri']])
    expect(manual.blocked?.confirmable).toBe(true)
    expect(refsBundles(repo)).toBe('')
  })

  it('가드를 우회해 직접 실행해도 -c fetch.bundleURI= 로 번들 호스트·로컬 번들에 접근하지 않는다', async () => {
    const host = await bundleHost()
    try {
      const { work, repo, pushCommit } = makeRepos()
      pushCommit('second')
      mkdirSync(noHooks, { recursive: true })
      const hooksPath = hooksPathFor('local', noHooks)
      const bundle = join(root, `b-${seq++}.bundle`)
      git(work, 'bundle', 'create', bundle, 'main')

      for (const target of [host.url, bundle]) {
        git(repo, 'config', 'fetch.bundleURI', target)
        const ins = await inspectConfig(exec, repo)
        expect(ins.dangerous.map((d) => d.key)).toContain('fetch.bundleuri') // 가드는 이것을 걸러야 한다 — 일부러 무시하고 실행
        await runFetch(exec, repo, ins, { hooksPath, allowFile: true, credentialHelpers: false })
        expect(host.hits).toEqual([])
        expect(refsBundles(repo)).toBe('')
      }

      // 대조: 덮어쓰기 없이 git 을 직접 부르면 번들 호스트에 접근하는지 기록한다
      git(repo, 'config', 'fetch.bundleURI', host.url)
      await exec.run(repo, ['fetch', 'origin'], { env: { GIT_ALLOW_PROTOCOL: 'file:http' } })
      console.log(`[대조] fetch.bundleURI 덮어쓰기 없이: 번들 호스트 요청 ${host.hits.length}개`)
    } finally {
      await host.close()
    }
  })

  it('trace2.* 는 알 수 없는 설정이라 자동 fetch 를 막는다(파일을 쓰지 않는다)', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'trace2.eventTarget', marker)
    git(repo, 'config', 'trace2.normalTarget', marker)
    const { fetcher, repoId } = setup(repo)
    const st = await fetcher.fetch(repoId, { manual: false })
    expect(st.blocked?.confirmable).toBe(true)
    expect(st.blocked?.keys).toEqual(expect.arrayContaining(['trace2.eventtarget', 'trace2.normaltarget']))
    expect(existsSync(marker)).toBe(false)
  })

  it.each([
    ['http.schannelCheckRevoke', 'false'],
    ['http.followRedirects', 'true'],
    ['ssh.variant', 'simple']
  ])('%s 도 알 수 없는 설정으로 걸린다', async (k, v) => {
    const { repo } = makeRepos()
    git(repo, 'config', k, v)
    expect((await inspectConfig(exec, repo)).dangerous.map((d) => d.key)).toEqual([k.toLowerCase()])
  })
})

describe('확인 창: 닮은꼴 문자', () => {
  const ch = (...c: number[]): string => String.fromCodePoint(...c)
  const unesc = (cp: number): string => (cp > 0xffff ? `\\u{${cp.toString(16)}}` : `\\u${cp.toString(16).padStart(4, '0')}`)

  it.each([
    ['한글 채움 U+115F', 0x115f],
    ['한글 채움 U+1160', 0x1160],
    ['한글 채움 U+3164', 0x3164],
    ['반각 한글 채움 U+FFA0', 0xffa0],
    ['점자 빈칸 U+2800', 0x2800],
    ['결합 그래핌 조이너 U+034F', 0x034f],
    ['변이 선택자 U+FE0F', 0xfe0f],
    ['변이 선택자 보충 U+E0100', 0xe0100],
    ['NBSP U+00A0', 0x00a0],
    ['전각 공백 U+3000', 0x3000],
    ['EM 공백 U+2003', 0x2003],
    ['결합 겹침 줄 U+0336', 0x0336],
    ['오른쪽 큰따옴표 U+201D', 0x201d],
    ['왼쪽 큰따옴표 U+201C', 0x201c],
    ['전각 따옴표 U+FF02', 0xff02],
    ['꺾쇠 따옴표 U+00BB', 0x00bb],
    ['ALM U+061C', 0x061c],
    ['소프트 하이픈 U+00AD', 0x00ad],
    ['몽골어 모음 구분자 U+180E', 0x180e]
  ])('%s 는 \\uXXXX 로 드러난다', (_n, cp) => {
    expect(quoteForDisplay(`a${ch(cp)}b`)).toBe(`"a${unesc(cp)}b"`)
  })

  it('값 안의 ” = “ 로 키=값 경계를 흉내 낼 수 없다', () => {
    const shown = formatItem('core.sshcommand', `x${ch(0x201d)} = ${ch(0x201c)}safe`)
    // 진짜 구분자(" = ")는 정확히 한 번뿐이고, 값 안의 따옴표는 이스케이프되어 있다
    expect(shown.match(/" = "/g)).toHaveLength(1)
    expect(shown).toContain('\\u201d')
    expect(shown).toContain('\\u201c')
    expect(shown).not.toContain(ch(0x201d))
    expect(shown).not.toContain(ch(0x201c))
  })

  it('보이는 글자·ASCII 공백·작은따옴표는 그대로 둔다', () => {
    expect(quoteForDisplay("it's ok 안녕 😀")).toBe(`"it's ok 안녕 😀"`)
    expect(quoteForDisplay('say "hi"')).toBe('"say \\"hi\\""')
  })
})

describe('테스트 격리: 임시 디렉터리', () => {
  it('전역 git 설정 파일은 globalSetup 이 만든 임시 홈 안에 있다(실행이 끝나면 지워진다)', () => {
    expect(process.env['ANTLER_TEST_HOME']).toBeTruthy()
    expect(posix(process.env['GIT_CONFIG_GLOBAL']!).startsWith(posix(process.env['ANTLER_TEST_HOME']!))).toBe(true)
  })
})

describe('fetch 는 로컬 ref 를 쓰지 않는다(설정이 목적지를 정해도)', () => {
  type Cfg = (...a: string[]) => string
  const scenarios: [string, (cfg: Cfg, sha: string) => void][] = [
    ['로컬 브랜치 덮어쓰기', (c) => c('--add', 'remote.origin.fetch', '+refs/heads/feature:refs/heads/feature')],
    ['refs/stash 덮어쓰기', (c) => c('--add', 'remote.origin.fetch', '+refs/heads/main:refs/stash')],
    ['모든 로컬 브랜치 prune', (c) => c('--add', 'remote.origin.fetch', '+refs/heads/none/*:refs/heads/*')],
    [
      '로컬 태그 prune(prunetags)',
      (c) => {
        c('fetch.prunetags', 'true')
        c('remote.origin.prunetags', 'true')
        c('remote.origin.prune', 'true')
        c('--add', 'remote.origin.fetch', '+refs/tags/*:refs/tags/*')
      }
    ],
    [
      'core.bare=true 로 체크아웃 브랜치 이동',
      (c) => {
        c('core.bare', 'true')
        c('--add', 'remote.origin.fetch', '+refs/heads/main:refs/heads/main')
      }
    ],
    ['refs/replace 생성', (c, sha) => c('--add', 'remote.origin.fetch', `+refs/heads/main:refs/replace/${sha}`)],
    ['refs/heads/HEAD 생성', (c) => c('--add', 'remote.origin.fetch', '+refs/heads/main:refs/heads/HEAD')],
    ['HEAD 덮어쓰기', (c) => c('--add', 'remote.origin.fetch', '+refs/heads/main:HEAD')]
  ]

  /** 로컬 ref 스냅샷(refs/heads·tags·stash·replace 와 HEAD) */
  const snapshot = (repo: string): Map<string, string> => {
    const m = new Map<string, string>()
    const out = git(repo, 'for-each-ref', '--format=%(refname) %(objectname)', 'refs/heads', 'refs/tags', 'refs/stash', 'refs/replace')
    for (const line of out.split('\n').filter(Boolean)) {
      const [name, sha] = line.split(' ')
      m.set(name!, sha!)
    }
    m.set('HEAD', `${git(repo, 'rev-parse', 'HEAD')} ${git(repo, 'symbolic-ref', '-q', 'HEAD')}`)
    return m
  }
  /** 이전에 있던 로컬 ref 는 하나도 바뀌거나 사라지지 않았고, 새로 생긴 것은 (원격에서 따라온 새) 태그뿐이다 */
  const expectLocalRefsKept = (before: Map<string, string>, after: Map<string, string>): void => {
    for (const [name, v] of before) expect([name, after.get(name)]).toEqual([name, v])
    for (const name of after.keys()) if (!before.has(name)) expect(name.startsWith('refs/tags/')).toBe(true)
  }

  function prepare(): { repo: string; sha: string; work: string; pushCommit: (m: string) => string } {
    const { repo, work, pushCommit } = makeRepos()
    const sha = git(repo, 'rev-parse', 'HEAD')
    git(repo, 'branch', 'feature')
    git(repo, 'tag', 'localtag')
    git(repo, 'update-ref', 'refs/stash', sha)
    pushCommit('second')
    git(work, 'push', '-q', 'origin', 'main:feature')
    git(work, 'tag', 'v2')
    git(work, 'push', '-q', 'origin', 'v2')
    return { repo, sha, work, pushCommit }
  }

  it.each(scenarios)('%s: 자동 fetch 는 막히고, 허용 후 수동 fetch·가드 우회 직접 실행에서도 로컬 ref 가 그대로다', async (_name, setupCfg) => {
    const { repo, sha, pushCommit } = prepare()
    const cfg: Cfg = (...a) => git(repo, 'config', ...a)
    setupCfg(cfg, sha)
    const before = snapshot(repo)
    const { fetcher, repoId } = setup(repo)

    const auto = await fetcher.fetch(repoId, { manual: false })
    expect(auto.blocked?.confirmable).toBe(true)
    expectLocalRefsKept(before, snapshot(repo))

    // 사용자가 허용한 수동 fetch: 실제로 fetch 가 실행된다 — 원격 추적 ref 만 갱신된다
    const manual = await fetcher.fetch(repoId, { manual: true, confirm: async () => true })
    expect(manual.error).toBeNull()
    expect(manual.blocked).toBeNull()
    expect(git(repo, 'rev-parse', 'refs/remotes/origin/main')).toBe(git(repo, 'rev-parse', 'origin/main'))
    expect(git(repo, 'rev-parse', 'origin/main')).not.toBe(sha)
    expect(git(repo, 'rev-parse', 'origin/feature')).not.toBe(sha)
    expectLocalRefsKept(before, snapshot(repo))

    // 가드를 일부러 건너뛰고 직접 실행해도 같다(설정이 아니라 명령 자체가 목적지를 고정한다)
    const third = pushCommit('third')
    mkdirSync(noHooks, { recursive: true })
    const ins = await inspectConfig(exec, repo)
    await runFetch(exec, repo, ins, { hooksPath: hooksPathFor('local', noHooks), allowFile: true, credentialHelpers: false })
    expect(git(repo, 'rev-parse', 'origin/main')).toBe(third)
    expectLocalRefsKept(before, snapshot(repo))
  })

  it('정상 설정: 원격 추적 ref 가 갱신되고, 원격에서 지운 브랜치는 prune 되며, 새 태그만 따라온다(기존·로컬 태그는 그대로)', async () => {
    const { repo, work } = prepare()
    git(work, 'push', '-q', 'origin', 'main:gone')
    const { fetcher, repoId } = setup(repo)
    await fetcher.fetch(repoId, { manual: true })
    expect(git(repo, 'branch', '-r')).toContain('origin/gone')
    git(work, 'push', '-q', 'origin', '--delete', 'gone')
    const before = snapshot(repo)
    const st = await fetcher.fetch(repoId, { manual: true })
    expect(st.error).toBeNull()
    expect(git(repo, 'branch', '-r')).not.toContain('origin/gone')
    expect(git(repo, 'branch', '-r')).toContain('origin/feature')
    expect(git(repo, 'tag')).toContain('v2')
    expect(git(repo, 'tag')).toContain('localtag')
    expectLocalRefsKept(before, snapshot(repo))
  })

  it('원격에서 지운 태그는 로컬에서 지우지 않는다(--no-prune-tags)', async () => {
    const { repo, work } = prepare()
    const { fetcher, repoId } = setup(repo)
    await fetcher.fetch(repoId, { manual: true })
    expect(git(repo, 'tag')).toContain('v2')
    git(work, 'push', '-q', 'origin', '--delete', 'v2')
    git(repo, 'config', 'remote.origin.prunetags', 'true')
    await fetcher.fetch(repoId, { manual: true, confirm: async () => true })
    expect(git(repo, 'tag')).toContain('v2')
  })
})

describe('UNC·네트워크 경로 remote URL 은 어느 플랫폼에서든 거부한다', () => {
  it.each([
    '//host@8080/DavWWWRoot/x.git',
    '//host/share/x.git',
    '\\\\host\\share\\x.git',
    '\\\\?\\UNC\\host\\share\\x.git',
    '\\\\.\\pipe\\x',
    '/\\host/share/x',
    '  \\\\host\\share\\x.git',
    'file://host/share/x.git',
    'file://localhost/share/x.git',
    'file:////host/share/x.git',
    'FILE://host/share/x.git',
    'file:\\\\host\\share\\x.git'
  ])('거부: %s', (url) => expect(unsafeRemoteUrl(url)).not.toBeNull())

  it.each(['file:///C:/repos/x.git', 'file:///tmp/r.git', '/tmp/r.git', 'C:\\repos\\x.git', 'C:/repos/x.git', '../r.git', 'https://example.com/x.git', 'git@github.com:a/b.git'])(
    '허용: %s',
    (url) => expect(unsafeRemoteUrl(url)).toBeNull()
  )

  it('설정의 UNC remote 는 확인 없이 거부하고 접속하지 않는다', async () => {
    const { repo } = makeRepos()
    git(repo, 'config', 'remote.origin.url', '//127.0.0.1@1/DavWWWRoot/x.git')
    const { fetcher, repoId } = setup(repo)
    const st = await fetcher.fetch(repoId, { manual: true, confirm: async () => true })
    expect(st.blocked?.confirmable).toBe(false)
    expect(st.blocked?.keys[0]).toMatch(/UNC/)
  })

  it('submodule URL 도 같은 검사를 받는다', async () => {
    const { repo } = makeRepos()
    git(repo, 'config', 'submodule.lib.url', '\\\\host\\share\\lib.git')
    const ins = await inspectConfig(exec, repo)
    expect(ins.forbidden.map((f) => f.key)).toContain('submodule.lib.url')
  })
})

describe('--show-scope 를 모르는 옛 git(2.26 미만, 예: SSH 서버의 2.17)', () => {
  /** --show-scope 를 넣으면 옛 git 처럼 실패하는 실행기. 실제 호출을 기록한다 */
  const oldGit = (): { exec: GitExecutor; calls: string[][] } => {
    const calls: string[][] = []
    const wrapped: GitExecutor = {
      kind: 'local',
      run: async (repoPath: string, args: string[], opts?: RunOpts) => {
        calls.push(args)
        if (args.includes('--show-scope')) return { stdout: Buffer.alloc(0), stderr: "error: unknown option `show-scope'\n", code: 129 }
        return exec.run(repoPath, args, opts)
      },
      stream: (repoPath: string, args: string[], opts?: RunOpts) => exec.stream(repoPath, args, opts),
      dispose: async () => {}
    }
    return { exec: wrapped, calls }
  }

  it('범위마다 따로 읽어 같은 검사를 한다: 저장소의 위험 설정은 잡고, 전역 core.sshCommand 는 사용자 설정으로 존중한다', async () => {
    const { repo, marker } = makeRepos()
    git(repo, 'config', 'core.sshCommand', payload(marker))
    git(repo, 'config', 'core.excludesFile', '.gitignore-extra')
    execFileSync('git', ['config', '--global', 'core.sshCommand', 'ssh -i mykey'])
    try {
      const { exec: old, calls } = oldGit()
      const ins = await inspectConfig(old, repo)
      expect(calls.some((a) => a.includes('--local'))).toBe(true)
      expect(ins.dangerous.map((d) => d.key)).toEqual(['core.sshcommand'])
      expect(ins.userSshCommand).toBe('ssh -i mykey')
      expect(ins.hasRemote).toBe(true)
      expect(ins.remoteNames).toEqual(['origin'])
    } finally {
      execFileSync('git', ['config', '--global', '--unset-all', 'core.sshCommand'])
    }
  })

  it('extensions.worktreeConfig 가 켜져 있으면 config.worktree 도 읽는다', async () => {
    const { repo } = makeRepos()
    git(repo, 'config', 'extensions.worktreeConfig', 'true')
    git(repo, 'config', '--worktree', 'trace2.eventTarget', '/tmp/x')
    const { exec: old, calls } = oldGit()
    const ins = await inspectConfig(old, repo)
    expect(calls.some((a) => a.includes('--worktree'))).toBe(true)
    expect(ins.dangerous.map((d) => d.key)).toEqual(['trace2.eventtarget'])
  })

  it('worktreeConfig 가 켜져 있어도 config.worktree 가 없거나 --worktree 를 모르면 비어 있는 것으로 보고, 다른 실패는 거부한다', async () => {
    const { repo } = makeRepos()
    git(repo, 'config', 'extensions.worktreeConfig', 'true')
    const worktreeFails = (stderr: string): GitExecutor => ({
      kind: 'local',
      run: async (repoPath: string, args: string[], opts?: RunOpts) => {
        if (args.includes('--show-scope')) return { stdout: Buffer.alloc(0), stderr: "error: unknown option `show-scope'", code: 129 }
        if (args.includes('--worktree')) return { stdout: Buffer.alloc(0), stderr, code: 128 }
        return exec.run(repoPath, args, opts)
      },
      stream: (repoPath: string, args: string[], opts?: RunOpts) => exec.stream(repoPath, args, opts),
      dispose: async () => {}
    })
    // 실제 2.49 의 "파일 없음" 메시지(config.worktree 를 만들지 않은 저장소)
    expect((await inspectConfig(oldGit().exec, repo)).dangerous).toEqual([])
    expect((await inspectConfig(worktreeFails("error: unknown option `worktree'"), repo)).dangerous).toEqual([])
    await expect(inspectConfig(worktreeFails("fatal: bad config line 1 in file .git/config.worktree"), repo)).rejects.toThrow(
      /검사하지 못해/
    )
  })

  it('include 로 끌어온 저장소 설정도 잡는다', async () => {
    const { repo } = makeRepos()
    const inc = join(repo, '..', 'extra.cfg')
    writeFileSync(inc, '[credential]\n\thelper = !evil\n')
    git(repo, 'config', 'include.path', posix(inc))
    const ins = await inspectConfig(oldGit().exec, repo)
    expect(ins.dangerous.map((d) => d.key)).toContain('credential.helper')
  })

  it('local 설정을 읽지 못하면 fetch 하지 않는다', async () => {
    const dir = mkdtempSync(join(root, 'notrepo-'))
    await expect(inspectConfig(oldGit().exec, dir)).rejects.toThrow(/저장소 설정을 검사하지 못해/)
  })

  it('--show-scope 와 무관한 실패는 범위별 읽기로 넘어가지 않고 그대로 실패한다', async () => {
    const calls: string[][] = []
    const failing: GitExecutor = {
      kind: 'local',
      run: async (_p: string, args: string[]) => {
        calls.push(args)
        return { stdout: Buffer.alloc(0), stderr: 'fatal: bad config line 3', code: 128 }
      },
      stream: () => exec.stream('.', []),
      dispose: async () => {}
    }
    await expect(inspectConfig(failing, '.')).rejects.toThrow(/bad config line/)
    expect(calls).toHaveLength(1)
  })
})
