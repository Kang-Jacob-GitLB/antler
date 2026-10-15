// 자식 프로세스 실행 공통부. LocalExecutor 와 (M4) WslExecutor 가 공유한다.
import { spawn, type ChildProcess } from 'node:child_process'
import { AbortError, DEFAULT_MAX_BUFFER, GitError, type RunOpts, type RunResult } from './GitExecutor'

export interface SpawnSpec {
  command: string
  argv: string[]
  env?: Record<string, string>
  /** 에러 메시지에 쓸 git 인자 */
  gitArgs: string[]
}

function launch(spec: SpawnSpec, opts: RunOpts): ChildProcess {
  return spawn(spec.command, spec.argv, {
    env: { ...process.env, ...spec.env, ...opts.env },
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe']
  })
}

function killTree(child: ChildProcess): void {
  if (child.exitCode === null && !child.killed) child.kill()
}

export function runProcess(spec: SpawnSpec, opts: RunOpts = {}): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    if (opts.signal?.aborted) return reject(new AbortError())
    const child = launch(spec, opts)
    const maxBuffer = opts.maxBuffer ?? DEFAULT_MAX_BUFFER
    const out: Buffer[] = []
    const err: Buffer[] = []
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
        resolve({ stdout: Buffer.concat(out), stderr: Buffer.concat(err).toString('utf8'), code: code ?? -1 })
      )
    })
  })
}

export async function* streamProcess(spec: SpawnSpec, opts: RunOpts = {}): AsyncIterable<Buffer> {
  if (opts.signal?.aborted) throw new AbortError()
  const child = launch(spec, opts)
  const err: Buffer[] = []
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
      const stderr = Buffer.concat(err).toString('utf8')
      throw new GitError(stderr.trim() || `git exited with ${code}`, code, stderr, spec.gitArgs)
    }
  } finally {
    // 소비자가 중간에 break 해도 프로세스를 남기지 않는다.
    opts.signal?.removeEventListener('abort', onAbort)
    killTree(child)
  }
}
