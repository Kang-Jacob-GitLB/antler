// GitHub 원격 전용 저장소의 git 이 캐시 로컬 config 로 TLS 검증을 끄거나 프록시를 바꾸지 못하는지(토큰 환경은 command scope 로 고정한다)
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { childEnv } from '../src/main/executors/processRunner'
import { envHttpsProxy, githubEnv, readNetworkConfig } from '../src/main/git/githubCache'

let root: string
let gitDir: string
let hooks: string
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-ghpin-'))
  hooks = join(root, 'no-hooks')
  mkdirSync(hooks)
  gitDir = join(root, 'cache', 'octocat', 'hello.git')
  mkdirSync(gitDir, { recursive: true })
  const run = (args: string[]): void => {
    execFileSync('git', args, { env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: githubEnv(null, { gitDir, hooksPath: hooks }).GIT_CONFIG_GLOBAL }, stdio: 'ignore' })
  }
  run(['init', '--bare', '-q', gitDir])
  // 로컬 쓰기 권한을 가진 공격자가 심은 설정
  for (const [k, v] of [
    ['http.sslVerify', 'false'],
    ['http.proxy', 'http://attacker:8080'],
    ['http.https://github.com/.sslVerify', 'false'],
    ['http.https://github.com/.proxy', 'http://attacker:8080']
  ] as const) {
    run(['--git-dir', gitDir, 'config', '--local', k, v])
  }
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

/** git 이 실제로 쓰는 값. 끝 줄바꿈만 뗀다(빈 값 = 빈 문자열) */
function effective(env: NodeJS.ProcessEnv, args: string[]): string {
  return execFileSync('git', ['config', ...args], { env, encoding: 'utf8' }).replace(/\r?\n$/, '')
}

const real = (token: string | null, netConfig?: [string, string][]): NodeJS.ProcessEnv =>
  childEnv(process.env, githubEnv(token, { gitDir, hooksPath: hooks, ...(netConfig ? { netConfig } : {}) }))

describe('githubEnv 의 TLS·프록시 고정', () => {
  it('고정 전(음성 대조): 설정을 싣지 않으면 캐시의 로컬 값이 이긴다', () => {
    const env = { ...real('tok'), GIT_CONFIG_COUNT: '0' }
    expect(effective(env, ['--get', 'http.sslVerify'])).toBe('false')
    expect(effective(env, ['--get', 'http.proxy'])).toBe('http://attacker:8080')
    expect(effective(env, ['--get-urlmatch', 'http.sslVerify', 'https://github.com/octocat/hello.git'])).toBe('false')
    expect(effective(env, ['--get-urlmatch', 'http.proxy', 'https://github.com/octocat/hello.git'])).toBe('http://attacker:8080')
  })

  it('고정 후: 로컬 config 가 sslVerify=false·프록시를 심어도 git 이 쓰는 값은 고정값이다', () => {
    const env = real('tok')
    expect(effective(env, ['--get', 'http.sslVerify'])).toBe('true')
    expect(effective(env, ['--get', 'http.proxy'])).toBe('')
    expect(effective(env, ['--get-urlmatch', 'http.sslVerify', 'https://github.com/octocat/hello.git'])).toBe('true')
    expect(effective(env, ['--get-urlmatch', 'http.proxy', 'https://github.com/octocat/hello.git'])).toBe('')
  })

  it('토큰이 없는 환경도 같다(조회 경로의 lazy fetch)', () => {
    const env = real(null)
    expect(effective(env, ['--get', 'http.sslVerify'])).toBe('true')
    expect(effective(env, ['--get-urlmatch', 'http.proxy', 'https://github.com/octocat/hello.git'])).toBe('')
  })

  it('사용자 프록시는 그대로 싣고 로컬 값이 이기지 못한다', () => {
    const env = real('tok', [['http.proxy', 'http://proxy.corp:8080']])
    expect(effective(env, ['--get', 'http.proxy'])).toBe('http://proxy.corp:8080')
    expect(effective(env, ['--get-urlmatch', 'http.proxy', 'https://github.com/octocat/hello.git'])).toBe('http://proxy.corp:8080')
  })

  it('프록시 키는 한 번만 싣는다(netConfig 에 있으면 빈 값을 더하지 않는다)', () => {
    const env = githubEnv('tok', { gitDir, hooksPath: hooks, netConfig: [['http.proxy', 'http://p:1']] })
    const keys = Object.entries(env).filter(([k]) => k.startsWith('GIT_CONFIG_KEY_'))
    expect(keys.filter(([, v]) => v === 'http.proxy')).toHaveLength(1)
    expect(env['GIT_CONFIG_COUNT']).toBe(String(keys.length))
  })
})

describe('환경 변수 프록시 옮기기', () => {
  it('https_proxy → HTTPS_PROXY → all_proxy → ALL_PROXY 순이고 빈 값·너무 긴 값은 버린다', () => {
    expect(envHttpsProxy({})).toBeNull()
    expect(envHttpsProxy({ ALL_PROXY: 'http://a', all_proxy: 'http://b' })).toBe('http://b')
    expect(envHttpsProxy({ HTTPS_PROXY: 'http://c', all_proxy: 'http://b' })).toBe('http://c')
    expect(envHttpsProxy({ https_proxy: '', HTTPS_PROXY: 'http://c' })).toBe('http://c')
    expect(envHttpsProxy({ http_proxy: 'http://only-http' })).toBeNull()
    expect(envHttpsProxy({ HTTPS_PROXY: 'x'.repeat(5000) })).toBeNull()
  })

  it('readNetworkConfig 는 사용자 프록시가 없을 때만 환경 변수 프록시를 싣는다', async () => {
    const base: NodeJS.ProcessEnv = { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: githubEnv(null, { gitDir, hooksPath: hooks }).GIT_CONFIG_GLOBAL }
    delete base.HTTPS_PROXY
    delete base.https_proxy
    delete base.ALL_PROXY
    delete base.all_proxy
    expect(await readNetworkConfig('git', base)).toEqual([])
    expect(await readNetworkConfig('git', { ...base, HTTPS_PROXY: 'http://env-proxy:3128' })).toEqual([['http.proxy', 'http://env-proxy:3128']])
    // git 을 실행하지 못해도 환경 변수 프록시는 옮긴다
    expect(await readNetworkConfig('git-does-not-exist', { HTTPS_PROXY: 'http://env-proxy:3128' })).toEqual([['http.proxy', 'http://env-proxy:3128']])
  })
})
