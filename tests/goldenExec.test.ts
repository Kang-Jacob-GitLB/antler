import { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { buildGitArgs, GIT_ENV, GIT_GLOBAL_ARGS } from '../src/main/executors/GitExecutor'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { buildRemoteCommand } from '../src/main/executors/ssh/quote'
import { WslExecutor, wslArgv } from '../src/main/executors/WslExecutor'
import { fetchEnv, inspectEntries, parseConfigList, runFetch } from '../src/main/git/fetch'
import { REMOTE_ENVS, REMOTE_SAMPLES } from './fixtures/sshCases'

// 실행기의 공통 인자·환경변수·WSL argv 를 골든 JSON 으로 고정한다. Rust 이식(src-tauri/tests/exec.rs)이 같은 값을 쓰는지 본다.
// 골든은 TS 가 기준이다: `UPDATE_GOLDEN=1 npx vitest run tests/goldenExec.test.ts` 로 다시 쓴다.
// export 되지 않은 값(WSL_SPAWN_ENV, 실제 spawn 인자)은 spawn 을 가짜로 바꿔 캡처한다. git 은 실행하지 않는다.

const spawned = vi.hoisted(() => [] as { command: string; argv: string[]; env: Record<string, string | undefined> }[])

vi.mock('node:child_process', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:child_process')>()
  return {
    ...actual,
    spawn: (command: string, argv: string[], options: { env: Record<string, string | undefined> }) => {
      spawned.push({ command, argv, env: options.env })
      const child = new EventEmitter() as EventEmitter & Record<string, unknown>
      child.stdout = new EventEmitter()
      child.stderr = new EventEmitter()
      child.exitCode = null
      child.signalCode = null
      setImmediate(() => child.emit('close', 0))
      return child
    }
  }
})

const GOLDEN = join(__dirname, 'golden', 'exec', 'env.json')
const UPDATE = process.env.UPDATE_GOLDEN === '1'

const SAMPLES: { distro: string; repoPath: string; args: string[] }[] = [
  { distro: 'Ubuntu', repoPath: '/home/me/repo', args: ['log', '-z'] },
  { distro: 'Ubuntu-24.04', repoPath: '/home/me/내 repo', args: [] },
  { distro: 'dev', repoPath: 'proj', args: ['rev-parse', '--show-toplevel'] },
  { distro: 'My Distro', repoPath: `/tmp/a"b'c $HOME`, args: ['-c', 'alias.x=!echo $HOME', 'x', '--', '-weird path'] }
]

/** 키를 정렬하고(env 의 [키, 값] 배열은 순서 그대로) 2칸·LF 로 쓴다 */
function stable(value: unknown): string {
  const emit = (v: unknown, indent: string): string => {
    if (Array.isArray(v)) {
      if (!v.length) return '[]'
      const inner = `${indent}  `
      return `[\n${v.map((x) => inner + emit(x, inner)).join(',\n')}\n${indent}]`
    }
    if (v && typeof v === 'object') {
      const keys = Object.keys(v).sort()
      if (!keys.length) return '{}'
      const inner = `${indent}  `
      return `{\n${keys.map((k) => `${inner}${JSON.stringify(k)}: ${emit((v as Record<string, unknown>)[k], inner)}`).join(',\n')}\n${indent}}`
    }
    return JSON.stringify(v)
  }
  return `${emit(value, '')}\n`
}

// fetch: runFetch 가 실제로 spawn 에 주는 argv·환경(로컬)과 wsl.exe argv(WSL). Rust 의 fetch_args·FetchEnv·wsl_argv_with 가 같은 값을 만드는지 본다.
// ssh 명령이 process.env 에 기대지 않도록 사용자 core.sshCommand 를 주거나(로컬) WSL 로만 본다(WSL 은 Windows 환경변수를 읽지 않는다)
const FETCH_SAMPLES: { kind: 'local' | 'wsl'; remotes: string[]; hooksPath: string; allowFile: boolean; credentialHelpers: boolean; userSsh: string | null }[] = [
  { kind: 'local', remotes: ['origin'], hooksPath: 'C:/Users/me/no-hooks', allowFile: false, credentialHelpers: false, userSsh: 'ssh -F x' },
  { kind: 'local', remotes: ['origin', 'up stream', '-x', 'a:b', '한글'], hooksPath: '/tmp/no-hooks', allowFile: true, credentialHelpers: true, userSsh: 'ssh -i "k k"' },
  { kind: 'wsl', remotes: ['origin'], hooksPath: '/dev/null', allowFile: false, credentialHelpers: false, userSsh: null },
  { kind: 'wsl', remotes: ['origin', '--upload-pack=evil', 'x y'], hooksPath: '/dev/null', allowFile: true, credentialHelpers: true, userSsh: 'ssh -J h' }
]

async function captureFetch(): Promise<unknown[]> {
  const out: unknown[] = []
  for (const s of FETCH_SAMPLES) {
    spawned.length = 0
    const ins = {
      ...inspectEntries(s.userSsh ? parseConfigList(`global\0core.sshcommand\n${s.userSsh}\0`) : []),
      remoteNames: s.remotes
    }
    const exec = s.kind === 'local' ? new LocalExecutor() : new WslExecutor('Ubuntu')
    await runFetch(exec, '/home/me/repo', ins, { hooksPath: s.hooksPath, allowFile: s.allowFile, credentialHelpers: s.credentialHelpers })
    const keys = [...new Set([...Object.keys(GIT_ENV), ...Object.keys(fetchEnv({ sshCommand: '', allowFile: false }))])]
    out.push({
      ...s,
      calls: spawned.map((c) => ({
        command: c.command,
        argv: c.argv,
        // 로컬만 의미가 있다(WSL 은 argv 의 env K=V 가 리눅스 쪽 환경이다). 값이 없으면 '<없음>', 빈 값은 ''
        env: s.kind === 'local' ? keys.map((k) => [k, c.env[k] ?? '<없음>']) : []
      }))
    })
  }
  spawned.length = 0
  return out
}

// ssh: 원격 셸이 받는 명령 문자열의 원문 바이트(인용이 곧 주입 방어선이다). 환경은 GIT_ENV 단독과 fetch 환경 둘 — Rust 의 `build_remote_command` 는 닫힌 `FetchEnv` 만 받는다.
// NUL 이 든 입력은 던진 메시지를 남긴다
function captureSsh(): unknown[] {
  return REMOTE_ENVS.flatMap((e) =>
    REMOTE_SAMPLES.map((s) => {
      const env = { ...GIT_ENV, ...(e.fetch ? fetchEnv(e.fetch) : {}) }
      try {
        return { env: e.label, fetch: e.fetch, repoPath: s.repoPath, args: s.args, command: buildRemoteCommand(s.repoPath, s.args, env) }
      } catch (err) {
        return { env: e.label, fetch: e.fetch, repoPath: s.repoPath, args: s.args, error: (err as Error).message }
      }
    })
  )
}

async function capture(): Promise<unknown> {
  spawned.length = 0
  await new LocalExecutor().run('/r', ['log'])
  await new WslExecutor('Ubuntu').run('/home/me/repo', ['log', '-z'])
  const [local, wsl] = spawned
  // 부모 환경에 덧씌워진 값만 본다 — GIT_ENV 의 키는 spawn 환경에서 읽는다
  const pick = (env: Record<string, string | undefined>): string[][] => Object.keys(GIT_ENV).map((k) => [k, env[k] ?? '<없음>'])
  const fetch = await captureFetch()
  return {
    fetch,
    ssh: captureSsh(),
    globalArgs: GIT_GLOBAL_ARGS,
    // 순서가 WSL argv 의 `env K=V…` 순서를 정한다
    env: Object.entries(GIT_ENV),
    // 로컬 실행기가 실제로 spawn 에 준 값
    local: { command: local!.command, argv: local!.argv, env: pick(local!.env) },
    // wsl.exe 자신의 환경(리눅스 쪽 git 의 환경이 아니다)
    wsl: { command: wsl!.command, argv: wsl!.argv, wslUtf8: wsl!.env['WSL_UTF8'] },
    samples: SAMPLES.map((s) => ({ ...s, buildGitArgs: buildGitArgs(s.repoPath, s.args), wslArgv: wslArgv(s.distro, s.repoPath, s.args) }))
  }
}

describe('실행기 골든(env.json)', () => {
  beforeEach(() => {
    spawned.length = 0
  })

  it('TS 실행기의 공통 인자·환경·WSL argv 가 골든과 같다', async () => {
    const got = stable(await capture())
    if (UPDATE) {
      mkdirSync(join(__dirname, 'golden', 'exec'), { recursive: true })
      writeFileSync(GOLDEN, got)
      return
    }
    expect(existsSync(GOLDEN), 'UPDATE_GOLDEN=1 npx vitest run tests/goldenExec.test.ts 로 골든을 만든다').toBe(true)
    expect(got).toBe(readFileSync(GOLDEN, 'utf8').replace(/\r\n/g, '\n'))
  })

  it('로컬 실행기는 spawn 환경에 GIT_ENV 를 그대로 준다', async () => {
    await new LocalExecutor().run('/r', ['log'])
    for (const [k, v] of Object.entries(GIT_ENV)) expect(spawned[0]!.env[k]).toBe(v)
  })
})
