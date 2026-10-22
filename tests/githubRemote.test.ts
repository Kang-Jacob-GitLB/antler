// GitHub 원격 전용 저장소: owner/repo 검증 · 캐시 경로 · 설정 정확 일치 검사 · 실행 환경 · ConfigStore · 추가/삭제 ·
// 실제 git 으로 partial clone fetch + lazy fetch. 네트워크는 쓰지 않는다(file:// 소스 저장소와 로컬 bare 저장소만).
import { execFileSync } from 'node:child_process'
import { appendFileSync, closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import type { FetchStatus, GithubRepoLink } from '../src/shared/types'
import { GIT_ENV } from '../src/main/executors/GitExecutor'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { commitDetail, compareCommits } from '../src/main/git/commands'
import {
  classifyFailure,
  fetchConfigArgs,
  inspectEntries,
  parseConfigList,
  runFetch,
  versionAtLeast,
  gitVersion,
  type ConfigEntry
} from '../src/main/git/fetch'
import {
  cachePathMatchesLink,
  checkGithubConfig,
  ensureGithubHead,
  expectedGithubConfig,
  githubCachePath,
  githubCacheSlug,
  githubEnv,
  githubRemoteUrl,
  initGithubRepo,
  inspectGithubRepo,
  pickNetworkConfig,
  readNetworkConfig,
  redactToken,
  sanitizeGithubLink,
  validateGithubLink
} from '../src/main/git/githubCache'
import { ConfigStore, sourceFor } from '../src/main/services/ConfigStore'
import { FetchService } from '../src/main/services/FetchService'
import { RepoService } from '../src/main/services/RepoService'
import { RepoWatcher } from '../src/main/services/RepoWatcher'

let root: string
/** 훅 차단용 빈 디렉터리(githubEnv 의 core.hooksPath) */
let noHooks: string
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-ghremote-'))
  noHooks = join(root, 'no-hooks')
  mkdirSync(noHooks)
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

const tmpDir = (name: string): string => mkdtempSync(join(root, `${name}-`))
/** 테스트용 github 환경(캐시 폴더 dir 로 고정). 프로토콜은 file 로 바꿀 수 있다 */
const envOf = (dir: string, o: { allowProtocol?: string; token?: string | null } = {}): Record<string, string> =>
  githubEnv(o.token ?? null, { gitDir: dir, hooksPath: noHooks.replace(/\\/g, '/'), ...(o.allowProtocol ? { allowProtocol: o.allowProtocol } : {}) })
/** 환경 변수를 잠시 바꿔 fn 을 실행하고 되돌린다(사용자 전역 설정이 오염된 상황을 흉내 낸다) */
async function withEnv<T>(vars: Record<string, string>, fn: () => Promise<T>): Promise<T> {
  const saved = Object.fromEntries(Object.keys(vars).map((k) => [k, process.env[k]]))
  Object.assign(process.env, vars)
  try {
    return await fn()
  } finally {
    for (const [k, v] of Object.entries(saved)) {
      if (v === undefined) delete process.env[k]
      else process.env[k] = v
    }
  }
}
const link = (owner = 'octocat', repo = 'Hello-World', defaultBranch?: string): GithubRepoLink => ({ owner, repo, ...(defaultBranch ? { defaultBranch } : {}) })

/** git config --list --show-scope -z 와 같은 모양의 항목 */
const entry = (key: string, value: string | null, scope = 'local'): ConfigEntry => ({ scope, key, value })
/** 정상적인 캐시 저장소의 로컬 설정(Windows 의 git init --bare 가 쓰는 core.* 포함) */
function healthy(l = link()): ConfigEntry[] {
  return [
    entry('core.repositoryformatversion', '1'),
    entry('core.filemode', 'false'),
    entry('core.bare', 'true'),
    entry('core.symlinks', 'false'),
    entry('core.ignorecase', 'true'),
    entry('extensions.partialclone', 'origin'),
    entry('remote.origin.url', githubRemoteUrl(l)),
    entry('remote.origin.promisor', 'true'),
    entry('remote.origin.partialclonefilter', 'blob:none')
  ]
}

describe('owner/repo 검증', () => {
  it('정상 이름을 받고 기본 브랜치는 부가 정보로 다룬다', () => {
    expect(validateGithubLink({ owner: 'octocat', repo: 'Hello-World' })).toEqual({ owner: 'octocat', repo: 'Hello-World' })
    expect(validateGithubLink({ owner: 'a_b.c', repo: 'x.y_z-1', defaultBranch: 'release/1.x' })).toMatchObject({ defaultBranch: 'release/1.x' })
    // 쓸 수 없는 브랜치 이름은 버린다(첫 fetch 때 원격에 묻는다)
    expect(validateGithubLink({ owner: 'o', repo: 'r', defaultBranch: '-evil' })).toEqual({ owner: 'o', repo: 'r' })
    expect(validateGithubLink({ owner: 'o', repo: 'r', defaultBranch: 'a..b' })).toEqual({ owner: 'o', repo: 'r' })
    expect(validateGithubLink({ owner: 'o', repo: 'r', defaultBranch: 'x y' })).toEqual({ owner: 'o', repo: 'r' })
  })

  it.each([
    ['빈 값', { owner: '', repo: 'r' }],
    ['점', { owner: '.', repo: 'r' }],
    ['점점', { owner: 'o', repo: '..' }],
    ['- 로 시작', { owner: '-o', repo: 'r' }],
    ['- 로 시작(repo)', { owner: 'o', repo: '-r' }],
    ['슬래시', { owner: 'o/x', repo: 'r' }],
    ['역슬래시', { owner: 'o', repo: 'r\\x' }],
    ['공백', { owner: 'o', repo: 'r x' }],
    ['한글', { owner: '사용자', repo: 'r' }],
    ['NUL', { owner: 'o\0', repo: 'r' }],
    ['끝의 점', { owner: 'o.', repo: 'r' }],
    ['Windows 예약 이름', { owner: 'con', repo: 'r' }],
    ['Windows 예약 이름(repo)', { owner: 'o', repo: 'NUL.txt' }],
    ['.git 으로 끝나는 repo', { owner: 'o', repo: 'r.git' }],
    ['owner 길이 초과', { owner: 'a'.repeat(40), repo: 'r' }],
    ['repo 길이 초과', { owner: 'o', repo: 'r'.repeat(101) }],
    ['문자열이 아님', { owner: 1, repo: 'r' }],
    ['객체가 아님', null]
  ])('거부: %s', (_name, input) => {
    expect(() => validateGithubLink(input)).toThrow()
    expect(sanitizeGithubLink(input)).toBeNull()
  })

  it('길이 상한 경계는 받는다', () => {
    expect(() => validateGithubLink({ owner: 'a'.repeat(39), repo: 'r'.repeat(100) })).not.toThrow()
  })
})

describe('캐시 경로', () => {
  const base = join(tmpdir(), 'antler-cache-shape')
  it('소문자 <owner>/<repo>.git 으로 정해진다', () => {
    const p = githubCachePath(base, link('OctoCat', 'Hello-World'))
    expect(p).toBe(join(base, 'octocat', 'hello-world.git'))
    expect(githubCacheSlug(base, p)).toEqual({ owner: 'octocat', repo: 'hello-world' })
    expect(cachePathMatchesLink(base, p, link('OCTOCAT', 'hello-WORLD'))).toBe(true)
    expect(cachePathMatchesLink(base, p, link('octocat', 'other'))).toBe(false)
  })

  it.each([
    ['base 자체', () => base],
    ['한 단계', () => join(base, 'octocat')],
    ['세 단계', () => join(base, 'a', 'b', 'c.git')],
    ['.git 접미사 없음', () => join(base, 'a', 'b')],
    ['대문자(정규 경로가 아님)', () => join(base, 'Octocat', 'hello.git')],
    ['base 밖(..)', () => join(base, '..', 'a', 'b.git')],
    ['base 밖(형제 폴더)', () => join(tmpdir(), 'antler-cache-shape2', 'a', 'b.git')],
    ['빈 repo 이름', () => join(base, 'a', '.git')],
    ['- 로 시작', () => join(base, '-a', 'b.git')]
  ])('모양이 틀리면 null: %s', (_n, p) => {
    expect(githubCacheSlug(base, p())).toBeNull()
  })

  it('프로덕션 URL 은 https://github.com 으로 고정이고 테스트만 바꾼다', () => {
    expect(githubRemoteUrl(link('o', 'r'))).toBe('https://github.com/o/r.git')
    expect(githubRemoteUrl(link('o', 'r'), 'file:///x')).toBe('file:///x/o/r.git')
  })
})

describe('설정 정확 일치 검사', () => {
  const expected = (): Map<string, string> => expectedGithubConfig(link())
  const check = (entries: ConfigEntry[]): string[] => checkGithubConfig(entries, expected())

  it('init 이 만든 그대로면 통과한다', () => expect(check(healthy())).toEqual([]))

  it('git config --list --show-scope -z 출력을 파싱한 항목도 통과한다', () => {
    const out = healthy().map((e) => `local\0${e.key}\n${e.value}\0`).join('')
    expect(check(parseConfigList(out))).toEqual([])
  })

  it('시스템·전역 scope 의 항목은 보지 않는다(사용자가 정한 것)', () => {
    expect(check([...healthy(), entry('credential.helper', 'manager', 'global'), entry('url.x.insteadof', 'y', 'system')])).toEqual([])
  })

  it.each([
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
    ['user.name', 'x']
  ])('키가 하나 더 있으면 차단한다: %s', (key, value) => {
    const problems = check([...healthy(), entry(key, value)])
    expect(problems).toHaveLength(1)
    expect(problems[0]).toContain(key)
  })

  it('url 이 달라지면 차단한다(허용 목록은 remote.*.url 을 아무 값이나 통과시키지만 여기서는 아니다)', () => {
    const tampered = healthy().map((e) => (e.key === 'remote.origin.url' ? { ...e, value: 'https://evil.example/o/r.git' } : e))
    expect(check(tampered)).toEqual(['remote.origin.url (예상과 다름)'])
    // 다른 저장소의 URL 도 마찬가지
    const other = healthy(link('octocat', 'Spoon-Knife'))
    expect(check(other)).toEqual(['remote.origin.url (예상과 다름)'])
  })

  it('필수 키가 없으면 차단한다(promisor 누락 등)', () => {
    for (const key of expected().keys()) {
      expect(check(healthy().filter((e) => e.key !== key))).toEqual([`${key} (없음)`])
    }
  })

  it('값이 바뀌면 차단한다(filter·promisor·partialclone·형식 버전)', () => {
    const change = (key: string, value: string): string[] => check(healthy().map((e) => (e.key === key ? { ...e, value } : e)))
    expect(change('remote.origin.partialclonefilter', 'blob:limit=1m')).toEqual(['remote.origin.partialclonefilter (예상과 다름)'])
    expect(change('remote.origin.promisor', 'false')).toEqual(['remote.origin.promisor (예상과 다름)'])
    expect(change('extensions.partialclone', 'evil')).toEqual(['extensions.partialclone (예상과 다름)'])
    expect(change('core.repositoryformatversion', '0')).toEqual(['core.repositoryformatversion (예상과 다름)'])
  })

  it('같은 키가 두 번 있으면 차단한다(뒤의 값이 이긴다)', () => {
    expect(check([...healthy(), entry('remote.origin.url', 'https://evil.example/x.git')])).toEqual(
      expect.arrayContaining(['remote.origin.url (예상과 다름)', 'remote.origin.url (중복)'])
    )
  })

  it('core.bare 는 true 일 때만 허용한다(이 함수에서만 — 일반 허용 목록은 그대로 false 만 받는다)', () => {
    expect(check(healthy().map((e) => (e.key === 'core.bare' ? { ...e, value: 'false' } : e)))).toEqual(['core.bare (예상과 다름)'])
    expect(inspectEntries([entry('core.bare', 'true')]).dangerous.map((d) => d.key)).toEqual(['core.bare'])
  })

  it('worktree scope 의 항목도 같은 기준으로 본다', () => {
    expect(check([...healthy(), entry('core.sshcommand', 'evil', 'worktree')])).toHaveLength(1)
  })

  it('init 이 형식에 따라 덧붙이는 extensions.refstorage·objectformat 은 정해진 값일 때만 허용한다(git 3.0 기본은 reftable)', () => {
    expect(check([...healthy(), entry('extensions.refstorage', 'reftable')])).toEqual([])
    expect(check([...healthy(), entry('extensions.refstorage', 'files')])).toEqual([])
    expect(check([...healthy(), entry('extensions.objectformat', 'sha1')])).toEqual([])
    expect(check([...healthy(), entry('extensions.refstorage', 'evil')])).toEqual(['extensions.refstorage (예상과 다름)'])
    expect(check([...healthy(), entry('extensions.objectformat', 'sha256')])).toEqual(['extensions.objectformat (예상과 다름)'])
    // 필수 키가 아니므로 없어도 된다(위 healthy 가 이미 그렇다). 다른 extensions.* 는 여전히 막힌다
    expect(check([...healthy(), entry('extensions.noop', 'true')])).toHaveLength(1)
  })
})

describe('githubEnv', () => {
  const dir = join(tmpdir(), 'antler-env-shape', 'remote-repos', 'octocat', 'hello.git')
  /** command scope 로 넘어간 설정을 {키: 값} 으로 푼다 */
  const configOf = (env: Record<string, string>): Record<string, string> => {
    const n = Number(env['GIT_CONFIG_COUNT'])
    return Object.fromEntries(Array.from({ length: n }, (_, i) => [env[`GIT_CONFIG_KEY_${i}`]!, env[`GIT_CONFIG_VALUE_${i}`]!]))
  }

  it('lazy fetch 를 열고 https 만 허용하며 자격 증명 도우미를 비운다', () => {
    const env = envOf(dir)
    expect(GIT_ENV['GIT_NO_LAZY_FETCH']).toBe('1')
    expect(env['GIT_NO_LAZY_FETCH']).toBe('0')
    expect(env['GIT_ALLOW_PROTOCOL']).toBe('https')
    expect(env['GIT_LFS_SKIP_SMUDGE']).toBe('1')
    expect(env['GIT_TERMINAL_PROMPT']).toBe('0')
    expect(configOf(env)['credential.helper']).toBe('')
    // 읽기 전용 보호는 그대로
    expect(env['GIT_NO_REPLACE_OBJECTS']).toBe('1')
    expect(env['GIT_LITERAL_PATHSPECS']).toBe('1')
  })

  it('저장소를 고정하고 사용자 시스템·전역 설정을 끊는다', () => {
    const env = envOf(dir)
    expect(env['GIT_DIR']).toBe(resolve(dir))
    // 상위 탐색 한계는 캐시 폴더의 부모(슬래시 표기)
    expect(env['GIT_CEILING_DIRECTORIES']).toBe(dirname(resolve(dir)).replace(/\\/g, '/'))
    expect(env['GIT_CONFIG_NOSYSTEM']).toBe('1')
    expect(env['GIT_CONFIG_GLOBAL']).toBe(process.platform === 'win32' ? 'NUL' : '/dev/null')
    expect(env['GIT_CONFIG_PARAMETERS']).toBe('')
  })

  it('하드닝 설정을 command scope 로 준다', () => {
    const cfg = configOf(envOf(dir))
    expect(cfg).toMatchObject({
      'maintenance.auto': 'false',
      'gc.auto': '0',
      'core.hooksPath': noHooks.replace(/\\/g, '/'),
      'transfer.bundleURI': 'false',
      'http.followRedirects': 'false'
    })
  })

  it('토큰은 github.com 에만 붙는 Authorization 헤더로 간다', () => {
    const env = envOf(dir, { token: 'ghp_secret' })
    expect(configOf(env)['http.https://github.com/.extraheader']).toBe(`Authorization: Basic ${Buffer.from('x-access-token:ghp_secret').toString('base64')}`)
    // 토큰은 이 값 밖에는 어디에도 없다
    const holders = Object.entries(env).filter(([, v]) => v.includes('ghp_secret') || v.includes(Buffer.from('x-access-token:ghp_secret').toString('base64')))
    expect(holders).toHaveLength(1)
    expect(holders[0]![0]).toMatch(/^GIT_CONFIG_VALUE_\d+$/)
  })

  it('쓸 수 없는 토큰(개행 등)은 헤더에 넣지 않는다', () => {
    expect(configOf(envOf(dir, { token: 'a\nb' }))).not.toHaveProperty(['http.https://github.com/.extraheader'])
    expect(configOf(envOf(dir, { token: '' }))).not.toHaveProperty(['http.https://github.com/.extraheader'])
  })

  it('상속된 GIT_CONFIG_COUNT·PARAMETERS 는 쓰지 않는다(사용자의 insteadOf 등이 섞이지 않는다)', async () => {
    const env = await withEnv(
      { GIT_CONFIG_COUNT: '1', GIT_CONFIG_KEY_0: 'url.ssh://x/.insteadof', GIT_CONFIG_VALUE_0: 'https://github.com/', GIT_CONFIG_PARAMETERS: "'url.ssh://x/.insteadof=https://github.com/'" },
      async () => envOf(dir)
    )
    // 우리 값이 같은 칸을 덮어쓰고 개수도 우리 것이다
    expect(configOf(env)).not.toHaveProperty(['url.ssh://x/.insteadof'])
    expect(env['GIT_CONFIG_PARAMETERS']).toBe('')
  })

  it('사용자 설정에서 가져온 네트워크 키를 command scope 로 옮긴다', () => {
    const env = githubEnv('t', { gitDir: dir, hooksPath: '/h', netConfig: [['http.proxy', 'http://proxy.corp:8080'], ['http.sslcainfo', '/etc/ca.pem']] })
    expect(configOf(env)).toMatchObject({ 'http.proxy': 'http://proxy.corp:8080', 'http.sslcainfo': '/etc/ca.pem', 'credential.helper': '' })
  })

  it('프로토콜은 테스트에서만 바꾼다', () => {
    expect(envOf(dir, { allowProtocol: 'file' })['GIT_ALLOW_PROTOCOL']).toBe('file')
  })

  it('네트워크 키만 고른다: 시스템·전역 scope 의 프록시·인증서, 보안을 낮추는 키·insteadOf·credential 은 뺀다', () => {
    const picked = pickNetworkConfig([
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
      entry('http.proxy', 'http://from-env', 'command')
    ])
    expect(Object.fromEntries(picked)).toEqual({
      'http.proxy': 'http://proxy.corp:8080',
      'http.sslcainfo': 'C:/ca.pem',
      'http.sslcapath': '/certs',
      'http.sslbackend': 'schannel',
      'http.schannelcheckrevoke': 'false',
      'http.schannelusesslcainfo': 'true',
      'http.noproxy': 'localhost'
    })
  })

  it('readNetworkConfig 는 실제 git 설정 파일에서 네트워크 키만 읽고, 환경의 GIT_CONFIG_* 에 오염되지 않는다', async () => {
    const file = join(tmpDir('netcfg'), 'gitconfig')
    writeFileSync(
      file,
      ['[http]', '\tproxy = http://proxy.corp:8080', '\tsslCAInfo = /etc/ca.pem', '\tsslVerify = false', '[url "ssh://x/"]', '\tinsteadOf = https://github.com/', '[credential]', '\thelper = evil', ''].join('\n')
    )
    const got = await readNetworkConfig('git', {
      ...process.env,
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: file,
      // 환경으로 주입된 설정(command scope)은 가져오지 않는다
      GIT_CONFIG_COUNT: '1',
      GIT_CONFIG_KEY_0: 'http.proxy',
      GIT_CONFIG_VALUE_0: 'http://from-env:1'
    })
    expect(Object.fromEntries(got)).toEqual({ 'http.proxy': 'http://proxy.corp:8080', 'http.sslcainfo': '/etc/ca.pem' })
    // git 을 실행하지 못하면 빈 목록이다
    expect(await readNetworkConfig('git-does-not-exist')).toEqual([])
  })

  it('redactToken 은 토큰과 base64 형태를 가린다', () => {
    const b64 = Buffer.from('x-access-token:ghp_secret').toString('base64')
    expect(redactToken(`fatal: ghp_secret and ${b64}`, 'ghp_secret')).toBe('fatal: *** and ***')
    expect(redactToken('plain', null)).toBe('plain')
  })
})

describe('ConfigStore: github 소스·링크', () => {
  const tmp = (): string => join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json')
  const add = (c: ConfigStore, l: GithubRepoLink, cache = join(root, 'cache')) =>
    c.addRepo({ sourceId: 'github', path: githubCachePath(cache, l), name: l.repo, github: l }, sourceFor('github'))

  it('sourceFor("github") 는 고정 id·라벨이다', () => {
    expect(sourceFor('github')).toEqual({ id: 'github', kind: 'github', label: 'GitHub' })
  })

  it('저장·재로드 왕복에서 링크와 소스가 보존된다', () => {
    const file = tmp()
    const c = new ConfigStore(file)
    const r = add(c, link('octocat', 'Hello-World', 'main'))
    const again = new ConfigStore(file)
    expect(again.get().sources.map((s) => s.id)).toEqual(['local', 'github'])
    expect(again.repo(r.id)?.github).toEqual({ owner: 'octocat', repo: 'Hello-World', defaultBranch: 'main' })
  })

  it('같은 소스·경로는 중복 추가되지 않는다', () => {
    const c = new ConfigStore(tmp())
    const a = add(c, link('octocat', 'Hello-World'))
    const b = add(c, link('OCTOCAT', 'hello-world'))
    expect(b.id).toBe(a.id)
    expect(c.get().repos).toHaveLength(1)
  })

  it('마지막 github 저장소를 지우면 github 소스도 사라진다', () => {
    const c = new ConfigStore(tmp())
    const a = add(c, link('o', 'a'))
    const b = add(c, link('o', 'b'))
    expect(c.removeRepo(a.id).sources.map((s) => s.id)).toEqual(['local', 'github'])
    expect(c.removeRepo(b.id).sources.map((s) => s.id)).toEqual(['local'])
  })

  it('잘못된 링크의 github 저장소는 버리고, 다른 소스에 붙은 링크는 필드만 버린다', () => {
    const file = tmp()
    const good = { id: 'g1', sourceId: 'github', path: '/c/o/r.git', name: 'r', github: { owner: 'o', repo: 'r' } }
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        sources: [{ id: 'local', kind: 'local', label: 'Local' }, { id: 'github', kind: 'ssh', label: 'x', sshHost: 'evil' }],
        repos: [
          good,
          { id: 'g2', sourceId: 'github', path: '/c/x', name: 'x', github: { owner: '../..', repo: 'r' } },
          { id: 'g3', sourceId: 'github', path: '/c/y', name: 'y' },
          { id: 'g4', sourceId: 'github', path: '/c/z', name: 'z', github: { owner: 'o', repo: 'z', defaultBranch: '--bad' } },
          { id: 'l1', sourceId: 'local', path: '/w/a', name: 'a', github: { owner: 'o', repo: 'a' } }
        ],
        ui: { theme: 'system', locale: 'ko', paneWidths: [248, 380] }
      })
    )
    const c = new ConfigStore(file)
    expect(c.get().repos.map((r) => r.id)).toEqual(['g1', 'g4', 'l1'])
    // 부가 정보인 기본 브랜치만 틀린 것은 브랜치만 버린다
    expect(c.repo('g4')?.github).toEqual({ owner: 'o', repo: 'z' })
    expect(c.repo('l1')?.github).toBeUndefined()
    // 파일에서 바뀐 github 소스는 믿지 않고 정해진 모양으로 다시 만든다
    expect(c.get().sources).toEqual([
      { id: 'local', kind: 'local', label: 'Local' },
      { id: 'github', kind: 'github', label: 'GitHub' }
    ])
  })

  it('저장소가 없으면 파일에 남은 github 소스는 정리된다', () => {
    const file = tmp()
    writeFileSync(file, JSON.stringify({ version: 1, sources: [{ id: 'local', kind: 'local', label: 'Local' }, { id: 'github', kind: 'github', label: 'GitHub' }], repos: [], ui: {} }))
    expect(new ConfigStore(file).get().sources.map((s) => s.id)).toEqual(['local'])
  })

  it('setGithubDefaultBranch 는 기본 브랜치를 기록하되 쓸 수 없는 이름은 무시한다', () => {
    const c = new ConfigStore(tmp())
    const r = add(c, link('o', 'r'))
    c.setGithubDefaultBranch(r.id, '-x')
    expect(c.repo(r.id)?.github?.defaultBranch).toBeUndefined()
    c.setGithubDefaultBranch(r.id, 'trunk')
    expect(c.repo(r.id)?.github?.defaultBranch).toBe('trunk')
  })

  it('cacheDir 를 주면 로드할 때 github 저장소의 path 를 이 PC 의 캐시 폴더로 다시 정하고, 같은 저장소의 중복은 하나만 남긴다', () => {
    const file = tmp()
    const cache = join(root, 'new-pc', 'remote-repos')
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        sources: [{ id: 'local', kind: 'local', label: 'Local' }],
        repos: [
          { id: 'g1', sourceId: 'github', path: 'C:\\Users\\old\\AppData\\remote-repos\\octocat\\hello.git', name: 'hello', github: { owner: 'OctoCat', repo: 'Hello' } },
          { id: 'g2', sourceId: 'github', path: '/home/other/remote-repos/octocat/hello.git', name: 'hello2', github: { owner: 'octocat', repo: 'hello' } },
          { id: 'l1', sourceId: 'local', path: '/w/a', name: 'a' }
        ],
        ui: {}
      })
    )
    const c = new ConfigStore(file, { cacheDir: cache })
    expect(c.get().repos.map((r) => r.id)).toEqual(['g1', 'l1'])
    expect(c.repo('g1')?.path).toBe(join(cache, 'octocat', 'hello.git'))
    expect(c.repo('l1')?.path).toBe('/w/a')
    // cacheDir 가 없으면 저장된 경로를 그대로 둔다
    expect(new ConfigStore(file).repo('g1')?.path).toBe('C:\\Users\\old\\AppData\\remote-repos\\octocat\\hello.git')
  })

  it('repos 배열의 null·비객체 항목은 그 항목만 건너뛰고 설정 전체를 손상 처리하지 않는다', () => {
    const file = tmp()
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        sources: [{ id: 'local', kind: 'local', label: 'Local' }],
        repos: [null, 3, 'x', [], { id: 'l1', sourceId: 'local', path: '/w/a', name: 'a' }],
        ui: { theme: 'dark' }
      })
    )
    const c = new ConfigStore(file)
    expect(c.get().repos.map((r) => r.id)).toEqual(['l1'])
    expect(c.get().ui.theme).toBe('dark')
    expect(c.loadFailed).toBe(false)
  })

  it('설정 파일이 손상됐거나 읽을 수 없으면 loadFailed 이고, 없으면(첫 실행) 아니다', () => {
    const bad = tmp()
    writeFileSync(bad, '{ not json')
    expect(new ConfigStore(bad).loadFailed).toBe(true)
    expect(new ConfigStore(tmp()).loadFailed).toBe(false)
    // 파일 자리에 폴더가 있으면 읽기 오류(EISDIR)
    const dirAsFile = join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json')
    mkdirSync(dirAsFile)
    expect(new ConfigStore(dirAsFile).loadFailed).toBe(true)
  })
})

// ── 실제 git ──────────────────────────────────────────────────────────────

const git = (cwd: string, ...args: string[]): string =>
  execFileSync('git', ['-C', cwd, '-c', 'user.name=t', '-c', 'user.email=t@t', ...args], { encoding: 'utf8' }).trim()

/** file:// 로 받을 수 있는 소스 저장소(partial clone 필터를 허용). 커밋 둘, 파일 하나가 두 번 바뀐다 */
function makeSource(dir: string): { url: string; first: string; second: string } {
  execFileSync('git', ['init', '-q', '-b', 'main', dir])
  git(dir, 'config', 'uploadpack.allowFilter', 'true')
  git(dir, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
  writeFileSync(join(dir, 'f.txt'), 'one\n')
  git(dir, 'add', 'f.txt')
  git(dir, 'commit', '-q', '-m', 'first')
  const first = git(dir, 'rev-parse', 'HEAD')
  writeFileSync(join(dir, 'f.txt'), 'one\ntwo\n')
  git(dir, 'commit', '-q', '-am', 'second')
  return { url: pathToFileURL(dir).href, first, second: git(dir, 'rev-parse', 'HEAD') }
}

describe('실제 git: 초기화 → fetch(blob:none) → lazy fetch', () => {
  let src: { url: string; first: string; second: string }
  let version: [number, number, number] | null = null
  // 테스트 전용: 프로토콜은 file, URL 은 소스 저장소. 프로덕션 경로(githubEnv 기본값·githubRemoteUrl 기본값)는 https+github.com 이다
  const testEnv = (dir: string): Record<string, string> => envOf(dir, { allowProtocol: 'file' })
  const lazyOn = new LocalExecutor('git', testEnv)
  const lazyOff = new LocalExecutor()
  const l = link('octocat', 'hello')

  beforeAll(async () => {
    src = makeSource(join(root, 'source'))
    version = await gitVersion(lazyOff)
  })

  // 한 번의 fetch 를 실제 FetchService.runGithub 와 같은 인자로 돌린다
  const fetchOnce = (dir: string): Promise<void> =>
    runFetch(lazyOn, dir, { ...inspectEntries([]), remoteNames: ['origin'] }, { hooksPath: '/dev/null', allowFile: false, credentialHelpers: false, env: testEnv(dir), filter: 'blob:none' })

  it('init 결과가 정확 일치 검사를 통과하고 fetch 뒤에도 통과한다', async () => {
    const dir = join(root, 'cache1')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    // 사용자의 init 템플릿·훅이 복사되지 않는다
    expect(existsSync(join(dir, 'hooks', 'pre-commit.sample'))).toBe(false)
    expect(await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).toEqual([])
    // 프로덕션 URL 로는 맞지 않는다(테스트 URL 로 바꾼 저장소가 실수로 통과하지 않는다)
    expect((await inspectGithubRepo(lazyOn, dir, l)).join()).toContain('remote.origin.url')

    await fetchOnce(dir)
    expect(git(dir, 'rev-parse', 'refs/remotes/origin/main')).toBe(src.second)
    // fetch --filter 가 설정을 덧쓰지 않았는지: 두 번째 fetch 전에 다시 검사해도 통과한다
    expect(await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).toEqual([])
    await fetchOnce(dir)
    expect(await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).toEqual([])
  })

  it('받은 뒤에 설정이 바뀌면 검사가 막는다', async () => {
    const dir = join(root, 'cache2')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    git(dir, 'config', '--local', 'core.sshCommand', 'evil')
    expect((await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).join()).toContain('core.sshcommand')
    git(dir, 'config', '--local', '--unset', 'core.sshCommand')
    git(dir, 'config', '--local', 'remote.origin.promisor', 'false')
    expect((await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).join()).toContain('remote.origin.promisor')
  })

  it('HEAD 를 받아 온 기본 브랜치로 맞춘다(origin/HEAD 가 있든 없든)', async () => {
    const dir = join(root, 'cache3')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    await fetchOnce(dir)
    // git 2.48+ 는 fetch 때 refs/remotes/origin/HEAD 를 만든다. 없는 경우(옛 git)도 같게 되는지 지우고 확인한다
    execFileSync('git', ['-C', dir, 'update-ref', '-d', '--no-deref', 'refs/remotes/origin/HEAD'], { stdio: 'ignore' })
    expect(await ensureGithubHead(lazyOn, dir, l, { env: testEnv(dir) })).toBe('main')
    expect(git(dir, 'symbolic-ref', 'HEAD')).toBe('refs/remotes/origin/main')
    expect(git(dir, 'symbolic-ref', 'refs/remotes/origin/HEAD')).toBe('refs/remotes/origin/main')
    expect(git(dir, 'rev-parse', 'HEAD')).toBe(src.second)
    // 레코드의 기본 브랜치가 실제와 다르면(이름이 바뀜) 원격에 물어 알아낸다
    expect(await ensureGithubHead(lazyOn, dir, { ...l, defaultBranch: 'gone' }, { env: testEnv(dir) })).toBe('main')
  })

  it('원격에서 기본 브랜치 이름이 바뀌면(옛 브랜치가 사라짐) 다시 알아내 HEAD 를 옮긴다', async () => {
    const source = join(root, 'source-rename')
    const s = makeSource(source)
    const dir = join(root, 'cache-rename')
    mkdirSync(dir)
    const link2 = { ...l, defaultBranch: 'main' }
    await initGithubRepo(lazyOn, dir, link2, { url: s.url })
    await fetchOnce(dir)
    expect(await ensureGithubHead(lazyOn, dir, link2, { env: testEnv(dir) })).toBe('main')
    git(source, 'branch', '-m', 'main', 'trunk')
    await fetchOnce(dir) // --prune 이 refs/remotes/origin/main 을 지운다
    expect((await lazyOn.run(dir, ['rev-parse', '--verify', '--quiet', 'refs/remotes/origin/main'])).code).not.toBe(0)
    // 레코드(main)가 가리키는 ref 가 없으니 origin/HEAD → ls-remote 순서로 trunk 를 찾는다
    expect(await ensureGithubHead(lazyOn, dir, link2, { env: testEnv(dir) })).toBe('trunk')
    expect(git(dir, 'symbolic-ref', 'HEAD')).toBe('refs/remotes/origin/trunk')
    expect(git(dir, 'rev-parse', 'HEAD')).toBe(s.second)
  })

  it('기본 브랜치를 미리 알면 init 이 HEAD 를 가리켜 두고, 받기 전에도 조회가 빈 결과다', async () => {
    const dir = join(root, 'cache4')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, { ...l, defaultBranch: 'main' }, { url: src.url })
    expect(git(dir, 'symbolic-ref', 'HEAD')).toBe('refs/remotes/origin/main')
  })

  it('lazy fetch: 허용 환경에서만 받지 않은 blob 이 필요한 명령이 성공한다', async (ctx) => {
    if (!versionAtLeast(version, [2, 45, 1])) return ctx.skip() // GIT_NO_LAZY_FETCH 는 git 2.45.1+
    const dir = join(root, 'cache5')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    await fetchOnce(dir)
    await ensureGithubHead(lazyOn, dir, l, { env: testEnv(dir) })
    // 커밋·트리는 있고 blob 은 없다
    expect(git(dir, 'rev-list', '--objects', '--missing=print', 'HEAD')).toMatch(/^\?/m)

    const numstat = ['diff-tree', '-r', '--numstat', '--no-commit-id', src.first, src.second]
    // 기본 환경(GIT_NO_LAZY_FETCH=1, 전송 금지): 로그는 되지만 blob 이 필요한 numstat 은 실패한다
    const log = await lazyOff.run(dir, ['log', '--format=%s', 'HEAD'])
    expect(log.stdout.toString('utf8').trim().split('\n')).toEqual(['second', 'first'])
    const blocked = await lazyOff.run(dir, numstat)
    expect(blocked.code).not.toBe(0)
    // github 환경: 필요한 blob 만 그때 받아 성공한다
    const open = await lazyOn.run(dir, numstat)
    expect(open.stderr).toBe('')
    expect(open.code).toBe(0)
    expect(open.stdout.toString('utf8')).toContain('1\t0\tf.txt')
    // 받은 뒤에는 기본 환경에서도 읽힌다
    expect((await lazyOff.run(dir, numstat)).code).toBe(0)
  })

  it('커밋 상세(commitDetail)도 github 환경에서는 blob 을 받아 와서 만든다', async (ctx) => {
    if (!versionAtLeast(version, [2, 45, 1])) return ctx.skip()
    const dir = join(root, 'cache6')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    await fetchOnce(dir)
    await expect(commitDetail(lazyOff, dir, src.second)).rejects.toThrow()
    const d = await commitDetail(lazyOn, dir, src.second)
    expect(d.files.map((f) => f.path)).toEqual(['f.txt'])
    expect(d.additions).toBe(1)
  })

  it('fetch 인자에 필터가 들어가고 토큰은 인자에 없다', () => {
    const cfg = fetchConfigArgs('/dev/null', { credentialHelpers: false })
    expect(cfg).toContain('credential.helper=')
    const env = envOf(join(root, 'o', 'r.git'), { token: 'ghp_secret' })
    expect(cfg.join(' ')).not.toContain('ghp_secret')
    expect(JSON.stringify(cfg)).not.toContain(Buffer.from('x-access-token:ghp_secret').toString('base64'))
    expect(Object.values(env).some((v) => v.includes('Basic '))).toBe(true)
  })

  it('커밋 상세·비교: github 저장소가 오프라인이면(lenient) 던지지 않고 통계 없는 파일 목록을 돌려준다', async (ctx) => {
    if (!versionAtLeast(version, [2, 45, 1])) return ctx.skip()
    // 이름 변경(+약간의 수정)이 든 커밋: -M 유사도 검사도 blob 이 필요하다
    const source = join(root, 'source-rename-commit')
    execFileSync('git', ['init', '-q', '-b', 'main', source])
    git(source, 'config', 'uploadpack.allowFilter', 'true')
    git(source, 'config', 'uploadpack.allowAnySHA1InWant', 'true')
    writeFileSync(join(source, 'a.txt'), 'line1\nline2\nline3\nline4\nline5\nline6\n')
    writeFileSync(join(source, 'b.txt'), 'x\n')
    git(source, 'add', '.')
    git(source, 'commit', '-q', '-m', 'one')
    const one = git(source, 'rev-parse', 'HEAD')
    renameSync(join(source, 'a.txt'), join(source, 'a2.txt'))
    appendFileSync(join(source, 'a2.txt'), 'line7\n')
    appendFileSync(join(source, 'b.txt'), 'y\n')
    git(source, 'add', '-A')
    git(source, 'commit', '-q', '-m', 'two')
    const two = git(source, 'rev-parse', 'HEAD')

    const dir = join(root, 'cache-offline')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: pathToFileURL(source).href })
    await fetchOnce(dir)
    // 연결이 끊긴 상태: 받아 올 곳이 없다
    git(dir, 'config', '--local', 'remote.origin.url', pathToFileURL(join(root, 'does-not-exist')).href)

    await expect(commitDetail(lazyOn, dir, two)).rejects.toThrow()
    await expect(compareCommits(lazyOn, dir, one, two)).rejects.toThrow()
    for (const d of [await commitDetail(lazyOn, dir, two, undefined, true), await compareCommits(lazyOn, dir, one, two, undefined, true)]) {
      expect(d.files.map((f) => f.path).sort()).toEqual(['a.txt', 'a2.txt', 'b.txt'])
      // 통계를 알 수 없다: 파일은 additions/deletions 가 null 이고 binary 는 false(바이너리와 구분), 합계는 0
      expect(d.files.every((f) => f.additions === null && f.deletions === null && !f.binary)).toBe(true)
      expect([d.additions, d.deletions]).toEqual([0, 0])
    }
    // 온라인이 되면(blob 을 받을 수 있으면) 완전한 결과가 나온다
    git(dir, 'config', '--local', 'remote.origin.url', pathToFileURL(source).href)
    const full = await commitDetail(lazyOn, dir, two, undefined, true)
    expect(full.files.find((f) => f.path === 'b.txt')).toMatchObject({ additions: 1, deletions: 0 })
    expect(full.files.find((f) => f.status === 'R')).toMatchObject({ path: 'a2.txt', oldPath: 'a.txt' })
  })
})

describe('실제 git: 사용자 git 설정과 격리', () => {
  let src: { url: string; first: string; second: string }
  let version: [number, number, number] | null = null
  const lazyOn = new LocalExecutor('git', (dir) => envOf(dir, { allowProtocol: 'file' }))
  const l = link('octocat', 'iso')

  beforeAll(async () => {
    src = makeSource(join(root, 'source-iso'))
    version = await gitVersion(new LocalExecutor())
  })

  const fetchOnce = (dir: string): Promise<void> =>
    runFetch(lazyOn, dir, { ...inspectEntries([]), remoteNames: ['origin'] }, { hooksPath: '/dev/null', allowFile: false, credentialHelpers: false, env: envOf(dir, { allowProtocol: 'file' }), filter: 'blob:none' })

  /** 추가 → 설정 검사 → fetch → 기본 브랜치 → log → numstat(lazy fetch) 이 한 번에 되는지 */
  async function wholeFlow(dir: string): Promise<void> {
    mkdirSync(dir, { recursive: true })
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    expect(await inspectGithubRepo(lazyOn, dir, l, undefined, src.url)).toEqual([])
    await fetchOnce(dir)
    expect(await ensureGithubHead(lazyOn, dir, l, { env: envOf(dir, { allowProtocol: 'file' }) })).toBe('main')
    const log = await lazyOn.run(dir, ['log', '--format=%s', 'HEAD'])
    expect(log.code).toBe(0)
    expect(log.stdout.toString('utf8').trim().split('\n')).toEqual(['second', 'first'])
    if (versionAtLeast(version, [2, 45, 1])) {
      const numstat = await lazyOn.run(dir, ['diff-tree', '-r', '--numstat', '--no-commit-id', src.first, src.second])
      expect(numstat.stderr).toBe('')
      expect(numstat.stdout.toString('utf8')).toContain('1\t0\tf.txt')
    }
  }

  it('전역 설정이 insteadOf(전송 금지에 걸리는 ssh)·safe.bareRepository=explicit·reftable 기본값이어도 끝까지 동작한다', async () => {
    const globalFile = join(tmpDir('hostile-global'), 'gitconfig')
    writeFileSync(
      globalFile,
      [`[url "ssh://nowhere.invalid/"]`, `\tinsteadOf = ${src.url}`, '[safe]', '\tbareRepository = explicit', '[init]', '\tdefaultRefFormat = reftable', ''].join('\n')
    )
    const dir = join(root, 'cache-hostile-global')
    await withEnv({ GIT_CONFIG_GLOBAL: globalFile }, async () => {
      // 대조: 격리하지 않은 환경에서는 이 전역 설정이 bare 저장소 접근을 막는다(git 2.38+)
      if (versionAtLeast(version, [2, 38, 0])) {
        const control = join(root, 'control-bare')
        execFileSync('git', ['init', '-q', '--bare', control])
        expect((await new LocalExecutor().run(control, ['rev-parse', '--git-dir'])).code).not.toBe(0)
      }
      await wholeFlow(dir)
    })
  })

  it('init 이 reftable 저장소를 만들어도(git 3.0 기본) 설정 검사를 통과하고 fetch·log 가 된다', async (ctx) => {
    if (!versionAtLeast(version, [2, 45, 0])) return ctx.skip() // GIT_DEFAULT_REF_FORMAT 은 git 2.45+
    const dir = join(root, 'cache-reftable')
    await withEnv({ GIT_DEFAULT_REF_FORMAT: 'reftable' }, () => wholeFlow(dir))
    const fmt = await lazyOn.run(dir, ['config', '--local', 'extensions.refstorage'])
    expect(fmt.stdout.toString('utf8').trim()).toBe('reftable')
  })

  it('실제로 적용되는 http.followRedirects 는 false 다(fetch 의 -c 가 환경 설정을 덮어쓰지 않는다)', async () => {
    const dir = join(root, 'cache-redirects')
    mkdirSync(dir)
    await initGithubRepo(lazyOn, dir, l, { url: src.url })
    const effective = async (cfg: string[]): Promise<string> => (await lazyOn.run(dir, [...cfg, 'config', '--get', 'http.followRedirects'])).stdout.toString('utf8').trim()
    expect(await effective([])).toBe('false')
    // 일반 저장소용 기본값은 initial 이라 그대로 쓰면 환경의 false 를 이긴다 — github 경로는 false 를 명시한다
    expect(await effective(fetchConfigArgs('/dev/null'))).toBe('initial')
    expect(await effective(fetchConfigArgs('/dev/null', { followRedirects: 'false' }))).toBe('false')
  })

  it('캐시 폴더가 비었거나 깨져 있으면 상위 폴더의 저장소로 올라가지 않고 실패한다(config 쓰기 포함)', async () => {
    const outer = join(root, 'outer-repo')
    execFileSync('git', ['init', '-q', outer])
    const dir = join(outer, 'remote-repos', 'octocat', 'iso.git')
    mkdirSync(dir, { recursive: true })
    const before = readFileSync(join(outer, '.git', 'config'), 'utf8')
    const write = await lazyOn.run(dir, ['config', '--local', 'remote.origin.promisor', 'true'])
    expect(write.code).not.toBe(0)
    expect((await lazyOn.run(dir, ['log', '--format=%H'])).code).not.toBe(0)
    expect((await lazyOn.run(dir, ['rev-parse', '--git-dir'])).code).not.toBe(0)
    // 설정 검사도 상위 저장소의 설정을 보지 않으므로 fetch 앞에서 차단된다(필수 키 없음)
    expect((await inspectGithubRepo(lazyOn, dir, l).catch(() => ['read failed'])).length).toBeGreaterThan(0)
    expect(readFileSync(join(outer, '.git', 'config'), 'utf8')).toBe(before)
    // 대조: 격리가 없으면 같은 명령이 상위 저장소에 쓴다
    expect((await new LocalExecutor().run(dir, ['config', '--local', 'remote.origin.promisor', 'true'])).code).toBe(0)
    expect(readFileSync(join(outer, '.git', 'config'), 'utf8')).not.toBe(before)
  })
})

// ── 서비스 ────────────────────────────────────────────────────────────────

describe('RepoService: 원격 전용 저장소', () => {
  let svc: RepoService | null = null
  let watcher: RepoWatcher | null = null
  afterEach(async () => {
    watcher?.dispose()
    watcher = null
    await svc?.dispose()
    svc = null
  })

  function setup(token: string | null = null): { svc: RepoService; config: ConfigStore; cacheDir: string } {
    const cacheDir = join(tmpDir('remote-repos'), 'remote-repos')
    const config = new ConfigStore(join(tmpDir('cfg'), 'antler.config.json'))
    svc = new RepoService(config, { cacheDir, githubToken: () => token, noHooksDir: noHooks })
    return { svc, config, cacheDir }
  }

  it('같은 경로를 동시에 추가해도 서로의 폴더를 지우지 않고 하나만 등록된다', async () => {
    const { svc, config, cacheDir } = setup()
    const results = await Promise.all([
      svc.addGithubRepos({ repos: [link('Race', 'Repo')] }),
      svc.addGithubRepos({ repos: [link('race', 'repo')], group: 'g' }),
      svc.addGithubRepos({ repos: [link('race', 'repo'), link('race', 'other')] })
    ])
    expect(results.flatMap((r) => r.failed)).toEqual([])
    const ids = new Set(results.flatMap((r) => r.added.filter((a) => a.name.toLowerCase() === 'repo').map((a) => a.id)))
    expect(ids.size).toBe(1)
    expect(config.get().repos).toHaveLength(2)
    const dir = join(cacheDir, 'race', 'repo.git')
    expect(git(dir, 'rev-parse', '--is-bare-repository')).toBe('true')
    expect(await inspectGithubRepo(new LocalExecutor(), dir, link('Race', 'Repo'))).toEqual([])
  })

  it('캐시에 bare 저장소를 만들어 등록하고, 같은 저장소를 다시 추가하면 기존 항목을 돌려준다', async () => {
    const { svc, config, cacheDir } = setup()
    const res = await svc.addGithubRepos({ repos: [link('OctoCat', 'Hello-World', 'main')], group: 'gh' })
    expect(res.failed).toEqual([])
    expect(res.added).toHaveLength(1)
    const repo = res.added[0]!
    const dir = join(cacheDir, 'octocat', 'hello-world.git')
    expect(repo).toMatchObject({ sourceId: 'github', path: dir, name: 'Hello-World', group: 'gh', github: { owner: 'OctoCat', repo: 'Hello-World', defaultBranch: 'main' } })
    expect(config.get().sources.map((s) => s.id)).toEqual(['local', 'github'])
    expect(git(dir, 'rev-parse', '--is-bare-repository')).toBe('true')
    expect(git(dir, 'config', '--local', 'remote.origin.url')).toBe('https://github.com/OctoCat/Hello-World.git')
    expect(git(dir, 'symbolic-ref', 'HEAD')).toBe('refs/remotes/origin/main')
    // 저장소 설정이 정확 일치 검사를 통과한다(Windows 의 init 이 덧붙이는 core.* 포함)
    expect(await inspectGithubRepo(new LocalExecutor(), dir, repo.github!)).toEqual([])

    const again = await svc.addGithubRepos({ repos: [link('octocat', 'hello-world')] })
    expect(again.added[0]!.id).toBe(repo.id)
    expect(config.get().repos).toHaveLength(1)
  })

  it('다른 org 에 같은 이름의 저장소가 이미 있으면 owner 를 붙여 이름을 짓는다', async () => {
    const { svc } = setup()
    const res = await svc.addGithubRepos({ repos: [link('acme', 'web'), link('globex', 'Web'), link('acme', 'api')] })
    expect(res.added.map((r) => r.name)).toEqual(['web', 'globex/Web', 'api'])
  })

  it('잘못된 항목은 failed 로 담고 나머지는 추가하며, 실패한 폴더는 남기지 않는다', async () => {
    const { svc, config, cacheDir } = setup()
    const res = await svc.addGithubRepos({ repos: [link('..', 'x'), link('good', 'repo'), { owner: 'o', repo: 'a/b' } as GithubRepoLink] })
    expect(res.added.map((r) => r.name)).toEqual(['repo'])
    expect(res.failed.map((f) => f.fullName)).toEqual(['../x', 'o/a/b'])
    expect(config.get().repos).toHaveLength(1)
    expect(existsSync(join(cacheDir, '..', 'x.git'))).toBe(false)
  })

  it('init 이 실패하면 만든 폴더를 정리한다', async () => {
    const { svc, config, cacheDir } = setup()
    // 소유자 자리에 파일이 있으면 폴더를 만들 수 없다
    mkdirSync(cacheDir, { recursive: true })
    writeFileSync(join(cacheDir, 'blocked'), 'x')
    const res = await svc.addGithubRepos({ repos: [link('blocked', 'r')] })
    expect(res.added).toEqual([])
    expect(res.failed).toHaveLength(1)
    expect(config.get().repos).toEqual([])
    expect(config.get().sources.map((s) => s.id)).toEqual(['local'])
  })

  it('반쯤 만들다 남은 폴더(등록 안 됨)는 지우고 새로 만든다', async () => {
    const { svc, cacheDir } = setup()
    const dir = join(cacheDir, 'o', 'stale.git')
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, 'junk'), 'x')
    const res = await svc.addGithubRepos({ repos: [link('o', 'stale')] })
    expect(res.failed).toEqual([])
    expect(existsSync(join(dir, 'junk'))).toBe(false)
    expect(git(dir, 'rev-parse', '--is-bare-repository')).toBe('true')
  })

  it('개수 상한과 입력 모양을 확인한다', async () => {
    const { svc } = setup()
    await expect(svc.addGithubRepos({ repos: Array.from({ length: 201 }, (_, i) => link('o', `r${i}`)) })).rejects.toThrow()
    await expect(svc.addGithubRepos({ repos: 'x' } as never)).rejects.toThrow()
  })

  it('repos:add · scan · env:test 경로로는 github 환경을 만들 수 없다', async () => {
    const { svc, config } = setup()
    await expect(svc.addRepo({ kind: 'github', path: 'C:\\Users' })).rejects.toThrow()
    await expect(svc.scanRepos({ kind: 'github', path: root })).rejects.toThrow()
    await expect(svc.testEnv({ kind: 'github' })).rejects.toThrow()
    expect(config.get().repos).toEqual([])
  })

  it('아직 받지 않은 저장소는 조회가 에러 대신 빈 결과다', async () => {
    const { svc } = setup()
    for (const defaultBranch of ['main', undefined]) {
      const { added } = await svc.addGithubRepos({ repos: [link('o', defaultBranch ? 'withbranch' : 'nobranch', defaultBranch)] })
      const id = added[0]!.id
      expect(await svc.hasNoRefs(id)).toBe(true)
      expect((await svc.log(id, { skip: 0, count: 50 })).commits).toEqual([])
      const refs = await svc.refs(id)
      expect([refs.local, refs.remotes, refs.tags, refs.stashes, refs.submodules]).toEqual([[], [], [], [], []])
      expect(refs.worktrees).toEqual([]) // bare 저장소 자신이 작업 트리로 나오지 않는다
      expect(await svc.refsDetails(id)).toEqual({ tracking: {}, submodules: [] })
      expect((await svc.activity(id, undefined)).total).toBe(0)
      expect((await svc.search(id, 'abc')).hashes).toEqual([])
      expect(await svc.signature(id)).toMatch(/^[0-9a-f]{40}$/)
    }
  })

  it('작업 트리 조회는 빈 결과이고 ghRepo 는 레코드에서 바로 돌려준다', async () => {
    const { svc } = setup()
    const id = (await svc.addGithubRepos({ repos: [link('OctoCat', 'Hello-World')] })).added[0]!.id
    const wip = await svc.wipStatus(id)
    expect([wip.staged, wip.unstaged, wip.untracked, wip.conflicted]).toEqual([[], [], [], 0])
    expect((await svc.wipDiff(id, { area: 'unstaged', path: 'a.txt' })).hunks).toEqual([])
    expect(await svc.ghRepo(id)).toEqual({ owner: 'OctoCat', repo: 'Hello-World', remote: 'origin' })
  })

  it('github 환경(토큰)은 캐시 경로의 github 저장소에만 열린다', async () => {
    const { svc, config, cacheDir } = setup('ghp_secret')
    const id = (await svc.addGithubRepos({ repos: [link('o', 'r')] })).added[0]!.id
    expect(svc.githubTarget(id)?.path).toBe(join(cacheDir, 'o', 'r.git'))
    const fetchEnv = svc.githubFetchEnv(join(cacheDir, 'o', 'r.git'))
    expect(Object.values(fetchEnv).some((v) => v.includes('Basic '))).toBe(true)
    expect(fetchEnv['GIT_DIR']).toBe(join(cacheDir, 'o', 'r.git'))
    expect(svc.redactSecrets('boom ghp_secret')).toBe('boom ***')
    // 캐시 밖 경로나 링크와 맞지 않는 경로의 github 레코드는 대상이 아니다
    const outside = config.addRepo({ sourceId: 'github', path: join(root, 'elsewhere.git'), name: 'e', github: link('o', 'e') }, sourceFor('github'))
    const mismatched = config.addRepo({ sourceId: 'github', path: join(cacheDir, 'o', 'r2.git'), name: 'm', github: link('o', 'other') })
    expect(svc.isGithub(outside.id)).toBe(true)
    expect(svc.githubTarget(outside.id)).toBeNull()
    expect(svc.githubTarget(mismatched.id)).toBeNull()
    // 일반 저장소는 무관하다
    const plain = config.addRepo({ sourceId: 'local', path: root, name: 'plain' })
    expect(svc.githubTarget(plain.id)).toBeNull()
  })

  it('RepoWatcher 가 bare 캐시 저장소의 ref 변화를 알아챈다', async () => {
    const { svc, cacheDir } = setup()
    const { added } = await svc.addGithubRepos({ repos: [link('o', 'watched')] })
    const dir = join(cacheDir, 'o', 'watched.git')
    const changes: string[] = []
    watcher = new RepoWatcher(svc, (id) => changes.push(id))
    await watcher.watch(added[0]!.id)
    const tree = execFileSync('git', ['-C', dir, 'hash-object', '-t', 'tree', '-w', '--stdin'], { input: '', encoding: 'utf8' }).trim()
    const commit = git(dir, 'commit-tree', tree, '-m', 'x')
    git(dir, 'update-ref', 'refs/remotes/origin/main', commit)
    const until = Date.now() + 5_000
    while (changes.length === 0 && Date.now() < until) await new Promise((r) => setTimeout(r, 50))
    expect(changes).toEqual([added[0]!.id])
  })

  describe('삭제', () => {
    it('캐시 폴더를 지우고 빈 소유자 폴더도 정리한다', async () => {
      const { svc, cacheDir } = setup()
      const a = (await svc.addGithubRepos({ repos: [link('o', 'a'), link('o', 'b')] })).added
      expect(svc.removeGithubCache(a[0]!.id)).toBe(true)
      expect(existsSync(join(cacheDir, 'o', 'a.git'))).toBe(false)
      expect(existsSync(join(cacheDir, 'o', 'b.git'))).toBe(true) // 형제는 그대로
      expect(existsSync(join(cacheDir, 'o'))).toBe(true)
      expect(svc.removeGithubCache(a[1]!.id)).toBe(true)
      expect(existsSync(join(cacheDir, 'o'))).toBe(false)
      expect(existsSync(cacheDir)).toBe(true)
    })

    it('모양이 틀린 경로는 지우지 않는다', () => {
      const { svc, config, cacheDir } = setup()
      const keep = (p: string): void => {
        mkdirSync(p, { recursive: true })
        writeFileSync(join(p, 'keep.txt'), 'x')
      }
      const cases = [
        join(root, 'outside-repo.git'), // 캐시 밖
        cacheDir, // 캐시 자체
        join(cacheDir, 'o'), // 소유자 폴더
        join(cacheDir, 'Upper', 'r.git'), // 정규 경로(소문자)가 아님
        join(cacheDir, 'a', 'b', 'c.git'), // 깊이가 다름
        join(cacheDir, 'o', 'plain') // .git 접미사 없음
      ]
      for (const [i, p] of cases.entries()) {
        keep(p)
        const r = config.addRepo({ sourceId: 'github', path: p, name: `x${i}`, github: link('o', `r${i}`) }, sourceFor('github'))
        expect(svc.removeGithubCache(r.id)).toBe(false)
        expect(existsSync(join(p, 'keep.txt'))).toBe(true)
      }
      // 일반 저장소 레코드는 어떤 경우에도 지우지 않는다
      const plainDir = join(root, 'plain-local')
      keep(plainDir)
      const plain = config.addRepo({ sourceId: 'local', path: plainDir, name: 'p' })
      expect(svc.removeGithubCache(plain.id)).toBe(false)
      expect(existsSync(join(plainDir, 'keep.txt'))).toBe(true)
    })

    it('소유자 폴더가 링크면 따라가지 않는다', async (ctx) => {
      const { svc, config, cacheDir } = setup()
      const target = tmpDir('link-target')
      mkdirSync(join(target, 'r.git'))
      writeFileSync(join(target, 'r.git', 'keep.txt'), 'x')
      mkdirSync(cacheDir, { recursive: true })
      try {
        symlinkSync(target, join(cacheDir, 'o'), 'junction')
      } catch {
        return ctx.skip() // 링크를 만들 수 없는 환경
      }
      const r = config.addRepo({ sourceId: 'github', path: join(cacheDir, 'o', 'r.git'), name: 'r', github: link('o', 'r') }, sourceFor('github'))
      expect(svc.removeGithubCache(r.id)).toBe(false)
      expect(existsSync(join(target, 'r.git', 'keep.txt'))).toBe(true)
    })

    it('레코드의 owner/repo 와 캐시 경로가 맞지 않으면 지우지 않는다(다른 저장소의 캐시를 가리키는 레코드)', async () => {
      const { svc, config, cacheDir } = setup()
      const [a] = (await svc.addGithubRepos({ repos: [link('o', 'a')] })).added
      // 같은 폴더를 다르게 적은 경로(끝의 /)로 중복 판정을 피해 등록한다
      const liar = config.addRepo({ sourceId: 'github', path: `${a!.path}/`, name: 'liar', github: link('o', 'liar') })
      expect(svc.removeGithubCache(liar.id)).toBe(false)
      expect(existsSync(join(cacheDir, 'o', 'a.git'))).toBe(true)
    })

    it('폴더를 지우지 못해도(다른 프로세스가 쥐고 있음) 던지지 않는다', async () => {
      const { svc, cacheDir } = setup()
      const [a] = (await svc.addGithubRepos({ repos: [link('o', 'busy')] })).added
      // 열린 파일 핸들(Windows 에서 삭제를 막는다). 다른 OS 는 지워지므로 결과만 확인한다
      const fd = openSync(join(cacheDir, 'o', 'busy.git', 'HEAD'), 'r')
      try {
        expect(() => svc.removeGithubCache(a!.id)).not.toThrow()
      } finally {
        closeSync(fd)
      }
    })
  })

  describe('시작 시 고아 캐시 정리', () => {
    it('어떤 레코드도 가리키지 않는 <owner>/<repo>.git 만 지우고 빈 소유자 폴더를 정리하며, 다른 것은 건드리지 않는다', async () => {
      const { svc, cacheDir } = setup()
      const kept = (await svc.addGithubRepos({ repos: [link('o', 'kept')] })).added[0]!
      const orphan = (p: string): void => {
        mkdirSync(p, { recursive: true })
        writeFileSync(join(p, 'HEAD'), 'x')
      }
      orphan(join(cacheDir, 'o', 'orphan.git')) // 형제가 등록돼 있어도 지운다
      orphan(join(cacheDir, 'gone', 'a.git')) // 소유자 폴더째
      orphan(join(cacheDir, 'gone', 'b.git'))
      mkdirSync(join(cacheDir, 'empty-owner'), { recursive: true }) // 비어 있는 소유자 폴더
      // 모양이 다른 것은 그대로
      const keepers = [join(cacheDir, 'Upper', 'r.git'), join(cacheDir, 'o', 'plain'), join(cacheDir, 'deep', 'a', 'b.git')]
      for (const p of keepers) {
        mkdirSync(p, { recursive: true })
        writeFileSync(join(p, 'keep.txt'), 'x')
      }
      writeFileSync(join(cacheDir, 'stray.txt'), 'x')

      expect(await svc.cleanupGithubCache()).toBe(3)
      expect(existsSync(join(cacheDir, 'o', 'orphan.git'))).toBe(false)
      expect(existsSync(join(cacheDir, 'gone'))).toBe(false)
      expect(existsSync(join(cacheDir, 'empty-owner'))).toBe(false)
      expect(existsSync(kept.path)).toBe(true)
      expect(git(kept.path, 'rev-parse', '--is-bare-repository')).toBe('true')
      for (const p of keepers) expect(existsSync(join(p, 'keep.txt'))).toBe(true)
      expect(existsSync(join(cacheDir, 'stray.txt'))).toBe(true)
      // 캐시 폴더가 없어도 조용히 끝난다
      expect(await setup().svc.cleanupGithubCache()).toBe(0)
    })

    it('로드 때 버려진 레코드(링크가 틀림)의 폴더도 고아로 지운다', async () => {
      const cacheDir = join(tmpDir('remote-repos'), 'remote-repos')
      mkdirSync(join(cacheDir, 'o', 'dropped.git'), { recursive: true })
      const file = join(tmpDir('cfg'), 'antler.config.json')
      writeFileSync(file, JSON.stringify({ version: 1, sources: [], repos: [{ id: 'x', sourceId: 'github', path: join(cacheDir, 'o', 'dropped.git'), name: 'd', github: { owner: '../..', repo: 'd' } }], ui: {} }))
      const config = new ConfigStore(file, { cacheDir })
      svc = new RepoService(config, { cacheDir, githubToken: () => null, noHooksDir: noHooks })
      expect(config.get().repos).toEqual([])
      expect(await svc.cleanupGithubCache()).toBe(1)
      expect(existsSync(join(cacheDir, 'o'))).toBe(false)
    })

    it('설정 파일을 읽지 못한 채 시작했으면(저장소 목록이 빈 것이 사실이 아니다) 아무것도 지우지 않는다', async () => {
      const cacheDir = join(tmpDir('remote-repos'), 'remote-repos')
      mkdirSync(join(cacheDir, 'o', 'precious.git'), { recursive: true })
      const file = join(tmpDir('cfg'), 'antler.config.json')
      writeFileSync(file, '{ corrupt')
      const config = new ConfigStore(file, { cacheDir })
      svc = new RepoService(config, { cacheDir, githubToken: () => null, noHooksDir: noHooks })
      expect(await svc.cleanupGithubCache()).toBe(0)
      expect(existsSync(join(cacheDir, 'o', 'precious.git'))).toBe(true)
    })

    it('소유자 폴더가 링크면 따라가지 않는다', async (ctx) => {
      const { svc, cacheDir } = setup()
      const target = tmpDir('link-target2')
      mkdirSync(join(target, 'r.git'))
      writeFileSync(join(target, 'r.git', 'keep.txt'), 'x')
      mkdirSync(cacheDir, { recursive: true })
      try {
        symlinkSync(target, join(cacheDir, 'o'), 'junction')
      } catch {
        return ctx.skip()
      }
      expect(await svc.cleanupGithubCache()).toBe(0)
      expect(existsSync(join(target, 'r.git', 'keep.txt'))).toBe(true)
    })
  })

  describe('fetch 실패 분류', () => {
    const fail = (msg: string): Error => new Error(msg)
    it.each([
      ["remote: Repository not found.\nfatal: repository 'https://github.com/o/r.git/' not found"],
      ['fatal: could not read Username for \'https://github.com\': terminal prompts disabled'],
      ["fatal: unable to access 'https://github.com/o/r.git/': The requested URL returned error: 403"],
      ['remote: Invalid username or token. Password authentication is not supported for Git operations.']
    ])('github 저장소는 인증 문제로 본다(백오프하지 않고 멈춤): %s', (msg) => {
      expect(classifyFailure(fail(msg), { github: true })).toBe('auth')
    })

    it('일반 저장소의 분류는 그대로다', () => {
      expect(classifyFailure(fail('remote: Repository not found.'))).toBe('network')
      expect(classifyFailure(fail('The requested URL returned error: 403'))).toBe('network')
      expect(classifyFailure(fail('fatal: could not read Username'))).toBe('auth')
      expect(classifyFailure(fail('Could not resolve host: github.com'), { github: true })).toBe('network')
    })
  })

  describe('fetch 차단', () => {
    it('설정이 달라진 저장소는 확인 창 없이 차단하고 네트워크를 쓰지 않는다', async () => {
      const { svc, config } = setup('ghp_secret')
      const { added } = await svc.addGithubRepos({ repos: [link('o', 'tampered')] })
      const repo = added[0]!
      git(repo.path, 'config', '--local', 'core.sshCommand', 'evil')
      const statuses: FetchStatus[] = []
      const fetcher = new FetchService(config, svc, (s) => statuses.push(s), () => {})
      try {
        const status = await fetcher.fetch(repo.id, {
          manual: true,
          confirm: () => {
            throw new Error('확인 창을 띄우면 안 된다')
          }
        })
        expect(status.blocked?.confirmable).toBe(false)
        expect(status.blocked?.keys.join()).toContain('core.sshcommand')
        expect(status.lastOkAt).toBeNull()
      } finally {
        fetcher.dispose()
      }
    })

    it('캐시 경로가 저장소와 맞지 않는 레코드는 일반 저장소 흐름으로 가지 않고 차단한다', async () => {
      const { svc, config } = setup()
      const bad = config.addRepo({ sourceId: 'github', path: tmpDir('not-cache'), name: 'bad', github: link('o', 'bad') }, sourceFor('github'))
      const fetcher = new FetchService(config, svc, () => {}, () => {})
      try {
        const status = await fetcher.fetch(bad.id, { manual: true, confirm: () => Promise.resolve(true) })
        expect(status.blocked?.confirmable).toBe(false)
        expect(status.lastOkAt).toBeNull()
      } finally {
        fetcher.dispose()
      }
    })
  })
})
