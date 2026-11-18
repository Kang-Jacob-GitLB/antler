// 자식 프로세스 실행 공통부. LocalExecutor 와 (M4) WslExecutor 가 공유한다.
import { spawn, type ChildProcess } from 'node:child_process'
import { AbortError, DEFAULT_MAX_BUFFER, GitError, SCRUB_PARENT_ENV_KEY, type RunOpts, type RunResult } from './GitExecutor'

export interface SpawnSpec {
  command: string
  argv: string[]
  env?: Record<string, string>
  /** 에러 메시지에 쓸 git 인자 */
  gitArgs: string[]
}

/** stderr 는 앞부분만 4MB 까지 담는다(악성 저장소·서버가 끝없이 써서 메모리를 채우지 못하게). 파이프는 계속 비운다 */
export const MAX_STDERR = 4 * 1024 * 1024

export function stderrCollector(): { push(chunk: Buffer): void; text(): string } {
  const parts: Buffer[] = []
  let size = 0
  return {
    push(chunk) {
      const room = MAX_STDERR - size
      if (room <= 0) return
      parts.push(chunk.length > room ? chunk.subarray(0, room) : chunk)
      size += Math.min(chunk.length, room)
    },
    text: () => Buffer.concat(parts).toString('utf8')
  }
}

/**
 * 토큰을 환경 변수로 싣는 git(GitHub 원격 전용 저장소)이 부모 환경에서 물려받으면 안 되는 이름.
 *  - GIT_SSL_NO_VERIFY: 값이 비어 있어도 TLS 검증을 꺼 중간자가 Authorization 헤더를 받는다
 *  - GIT_TRACE*(GIT_TRACE2* · GIT_TRACE_REDACT 포함) · GIT_CURL_VERBOSE: 헤더·설정값(토큰)을 파일·stderr 에 남긴다
 *  - GIT_CONFIG*: 시스템·전역 설정 격리를 깬다(COUNT/KEY/VALUE/PARAMETERS 는 우리 값이 다시 채운다)
 *  - 저장소·객체 위치(GIT_DIR · GIT_COMMON_DIR · GIT_WORK_TREE · GIT_INDEX_FILE · GIT_OBJECT_DIRECTORY · GIT_ALTERNATE_OBJECT_DIRECTORIES ·
 *    GIT_NAMESPACE …): init·config 가 사용자 저장소로 새어 `.git/config` 를 덮어쓸 수 있다
 *  - GIT_EXEC_PATH · GIT_TEMPLATE_DIR: 다른 git 하위 명령·템플릿을 쓰게 한다
 * Windows 환경 변수는 대소문자를 구분하지 않으므로 이름은 대문자로 맞춰 비교한다.
 */
const SCRUBBED_PARENT_ENV =
  /^GIT_(SSL_NO_VERIFY|TRACE.*|CURL_VERBOSE|CONFIG.*|DIR|COMMON_DIR|WORK_TREE|INDEX_FILE|OBJECT_DIRECTORY|ALTERNATE_OBJECT_DIRECTORIES|NAMESPACE|EXEC_PATH|TEMPLATE_DIR|CEILING_DIRECTORIES|PREFIX|SHALLOW_FILE|GRAFT_FILE|REPLACE_REF_BASE)$/

/**
 * 자식 프로세스 환경 = 부모 + spec + opts. spec·opts 에 SCRUB_PARENT_ENV_KEY 가 있으면 부모에서 위험 변수를 지운다.
 * 부모의 `git_dir` 와 우리 `GIT_DIR` 가 한 객체에 같이 들어가면 Windows 에서 어느 쪽이 이기는지 불확실하므로,
 * 호출자가 정한 이름은 대소문자 무관하게 부모 쪽을 지운다.
 */
export function childEnv(parent: NodeJS.ProcessEnv, spec: Record<string, string> = {}, extra: Record<string, string> = {}): NodeJS.ProcessEnv {
  const own = { ...spec, ...extra }
  if (!(SCRUB_PARENT_ENV_KEY in own)) return { ...parent, ...own }
  delete own[SCRUB_PARENT_ENV_KEY]
  const ours = new Set(Object.keys(own).map((k) => k.toUpperCase()))
  const env: NodeJS.ProcessEnv = {}
  for (const [k, v] of Object.entries(parent)) {
    const name = k.toUpperCase()
    if (SCRUBBED_PARENT_ENV.test(name) || ours.has(name)) continue
    env[k] = v
  }
  return Object.assign(env, own)
}

function launch(spec: SpawnSpec, opts: RunOpts): ChildProcess {
  return spawn(spec.command, spec.argv, {
    env: childEnv(process.env, spec.env, opts.env),
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe']
  })
}

/**
 * 프로세스 트리를 종료한다. Windows 에서 PATH 의 git 이 Git for Windows 래퍼(cmd\git.exe)면
 * 실제 git.exe 는 손자 프로세스라 child.kill() 로는 남으므로 taskkill /T 를 쓴다.
 */
function killTree(child: ChildProcess): void {
  if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) return
  if (process.platform === 'win32') {
    const tk = spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' })
    tk.on('error', () => child.kill())
  } else {
    child.kill()
  }
}

export function runProcess(spec: SpawnSpec, opts: RunOpts = {}): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    if (opts.signal?.aborted) return reject(new AbortError())
    const child = launch(spec, opts)
    const maxBuffer = opts.maxBuffer ?? DEFAULT_MAX_BUFFER
    const out: Buffer[] = []
    const err = stderrCollector()
    let size = 0
    let settled = false

    const finish = (fn: () => void): void => {
      if (settled) return
      settled = true
      opts.signal?.removeEventListener('abort', onAbort)
      fn()
    }
    const onAbort = (): void => {
      killTree(child)
      finish(() => reject(new AbortError()))
    }
    opts.signal?.addEventListener('abort', onAbort, { once: true })

    child.stdout!.on('data', (chunk: Buffer) => {
      size += chunk.length
      if (size > maxBuffer) {
        killTree(child)
        finish(() => reject(new GitError(`output exceeded ${maxBuffer} bytes`, -1, '', spec.gitArgs)))
        return
      }
      out.push(chunk)
    })
    child.stderr!.on('data', (chunk: Buffer) => err.push(chunk))
    child.on('error', (e) => finish(() => reject(e)))
    child.on('close', (code) => {
      finish(() =>
        resolve({ stdout: Buffer.concat(out), stderr: err.text(), code: code ?? -1 })
      )
    })
  })
}

export async function* streamProcess(spec: SpawnSpec, opts: RunOpts = {}): AsyncIterable<Buffer> {
  if (opts.signal?.aborted) throw new AbortError()
  const child = launch(spec, opts)
  const err = stderrCollector()
  child.stderr!.on('data', (chunk: Buffer) => err.push(chunk))
  const exit = new Promise<number>((resolve, reject) => {
    child.on('error', reject)
    child.on('close', (code) => resolve(code ?? -1))
  })
  exit.catch(() => {}) // 소비자가 중간에 끊은 뒤 발생한 에러가 unhandled 로 새지 않게 한다
  const onAbort = (): void => killTree(child)
  opts.signal?.addEventListener('abort', onAbort, { once: true })
  try {
    for await (const chunk of child.stdout!) {
      yield chunk as Buffer
    }
    const code = await exit
    if (opts.signal?.aborted) throw new AbortError()
    if (code !== 0) {
      const stderr = err.text()
      throw new GitError(stderr.trim() || `git exited with ${code}`, code, stderr, spec.gitArgs)
    }
  } finally {
    // 소비자가 중간에 break 해도 프로세스를 남기지 않는다.
    opts.signal?.removeEventListener('abort', onAbort)
    killTree(child)
  }
}
