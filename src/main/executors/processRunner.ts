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

function launch(spec: SpawnSpec, opts: RunOpts): ChildProcess {
  return spawn(spec.command, spec.argv, {
    env: { ...process.env, ...spec.env, ...opts.env },
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
