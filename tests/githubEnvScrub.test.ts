// GitHub 원격 전용 저장소의 git 이 부모(앱 프로세스) 환경의 TLS 끄기·추적·저장소 위치 변수를 물려받지 않는지
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { SCRUB_PARENT_ENV_KEY } from '../src/main/executors/GitExecutor'
import { childEnv } from '../src/main/executors/processRunner'
import { githubEnv, initGithubRepo } from '../src/main/git/githubCache'

let root: string
let noHooks: string
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-ghscrub-'))
  noHooks = join(root, 'no-hooks')
  mkdirSync(noHooks)
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

/** 부모 환경에 둘 위험 변수. 일부러 소문자 이름도 섞는다(Windows 환경 변수는 대소문자 무시) */
const HOSTILE: Record<string, string> = {
  GIT_SSL_NO_VERIFY: '',
  git_trace_curl: '1',
  GIT_TRACE: '1',
  GIT_TRACE2_EVENT: '1',
  GIT_TRACE_REDACT: '0',
  GIT_CURL_VERBOSE: '1',
  GIT_NAMESPACE: 'evil',
  GIT_ALTERNATE_OBJECT_DIRECTORIES: '/nonexistent'
}

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

const ghEnv = (dir: string): Record<string, string> => githubEnv('tok', { gitDir: dir, hooksPath: noHooks.replace(/\\/g, '/') })

describe('childEnv', () => {
  const parent = { PATH: '/bin', GIT_SSL_NO_VERIFY: '', git_trace_curl: '1', Git_Dir: '/other', GIT_COMMON_DIR: '/other', HTTPS_PROXY: 'http://p', GIT_SSL_CAINFO: '/ca.pem' }

  it('표식이 있으면 위험 변수를 대소문자 무관하게 지우고 나머지(PATH·프록시·CA)는 남긴다', () => {
    const env = childEnv(parent, { [SCRUB_PARENT_ENV_KEY]: '1', GIT_DIR: '/mine' })
    expect(Object.keys(env).filter((k) => /^git_(ssl_no_verify|trace|common_dir)/i.test(k))).toEqual([])
    expect(Object.keys(env).filter((k) => k.toUpperCase() === 'GIT_DIR')).toEqual(['GIT_DIR'])
    expect(env['GIT_DIR']).toBe('/mine')
    expect(env[SCRUB_PARENT_ENV_KEY]).toBeUndefined()
    expect(env).toMatchObject({ PATH: '/bin', HTTPS_PROXY: 'http://p', GIT_SSL_CAINFO: '/ca.pem' })
  })

  it('표식이 없으면 기존 동작 그대로 부모를 물려준다(일반 local·wsl·ssh 실행은 바뀌지 않는다)', () => {
    expect(childEnv(parent, { GIT_X: '1' })).toEqual({ ...parent, GIT_X: '1' })
  })
})

describe('GitHub 캐시 git 의 환경(실제 git)', () => {
  /** `!env` alias 로 git 이 실제로 받은 환경을 읽는다 */
  const dumpEnv = async (exec: LocalExecutor, dir: string): Promise<string[]> => {
    const r = await exec.run(dir, ['-c', 'alias.dumpenv=!env', 'dumpenv'])
    expect(r.code).toBe(0)
    return r.stdout.toString('utf8').split(/\r?\n/).map((l) => l.split('=')[0]!.toUpperCase())
  }

  it('부모의 TLS 끄기·추적 변수가 git 에 닿지 않는다', async () => {
    const dir = join(root, 'a.git')
    mkdirSync(dir)
    await initGithubRepo(new LocalExecutor('git', ghEnv), dir, { owner: 'o', repo: 'r' })
    const names = await withEnv(HOSTILE, () => dumpEnv(new LocalExecutor('git', ghEnv), dir))
    for (const bad of ['GIT_SSL_NO_VERIFY', 'GIT_TRACE', 'GIT_TRACE_CURL', 'GIT_TRACE2_EVENT', 'GIT_TRACE_REDACT', 'GIT_CURL_VERBOSE', 'GIT_NAMESPACE', 'GIT_ALTERNATE_OBJECT_DIRECTORIES']) {
      expect(names, bad).not.toContain(bad)
    }
    expect(names).toContain('GIT_CONFIG_COUNT')
  })

  it('음성 대조: 표식을 뺀 같은 환경은 위험 변수를 그대로 물려준다', async () => {
    const dir = join(root, 'b.git')
    mkdirSync(dir)
    await initGithubRepo(new LocalExecutor('git', ghEnv), dir, { owner: 'o', repo: 'r' })
    const legacy = (d: string): Record<string, string> => {
      const { [SCRUB_PARENT_ENV_KEY]: _drop, ...rest } = ghEnv(d)
      return rest
    }
    const names = await withEnv({ GIT_SSL_NO_VERIFY: '', GIT_TRACE_REDACT: '0', GIT_CURL_VERBOSE: '1' }, () => dumpEnv(new LocalExecutor('git', legacy), dir))
    expect(names).toEqual(expect.arrayContaining(['GIT_SSL_NO_VERIFY', 'GIT_TRACE_REDACT', 'GIT_CURL_VERBOSE']))
  })

  it('init 이 부모의 GIT_COMMON_DIR·GIT_INDEX_FILE·GIT_WORK_TREE 를 따라 다른 저장소 config 를 덮어쓰지 않는다', async () => {
    const victim = join(root, 'victim')
    execFileSync('git', ['init', '-q', '--bare', victim])
    const before = readFileSync(join(victim, 'config'), 'utf8')
    const dir = join(root, 'c.git')
    mkdirSync(dir)
    await withEnv({ GIT_COMMON_DIR: victim, GIT_INDEX_FILE: join(victim, 'idx'), GIT_WORK_TREE: victim }, () =>
      initGithubRepo(new LocalExecutor('git', ghEnv), dir, { owner: 'o', repo: 'r' }, { url: 'https://github.com/o/r.git' })
    )
    expect(readFileSync(join(victim, 'config'), 'utf8')).toBe(before)
    expect(readFileSync(join(dir, 'config'), 'utf8')).toContain('github.com/o/r')
  })
})
