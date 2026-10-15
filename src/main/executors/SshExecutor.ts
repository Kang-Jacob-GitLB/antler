import { homedir } from 'node:os'
import { join } from 'node:path'
import type { SshHostInfo } from '@shared/types'
import {
  AbortError,
  DEFAULT_MAX_BUFFER,
  GIT_ENV,
  GitError,
  type GitExecutor,
  type RunOpts,
  type RunResult
} from './GitExecutor'
import { listHosts, loadSshConfig } from './ssh/config'
import { killChannel, SshConnection } from './ssh/connection'
import { buildRemoteCommand } from './ssh/quote'

export interface SshExecutorOptions {
  /** 기본 ~/.ssh/config */
  configPath?: string
  /** 기본 ~/.ssh/known_hosts */
  knownHostsPath?: string
  /** undefined: IdentityAgent/기본 agent · null: agent 사용 안 함 · string: 소켓/파이프 경로 */
  agent?: string | null
  /** ~ 와 기본 키(~/.ssh/id_*) 위치의 기준. 기본 os.homedir() */
  homeDir?: string
}

const CONNECTION_LOST = 'SSH 연결이 끊겨 원격 git 이 끝나기 전에 종료되었습니다'
/** stderr 가 끝없이 커지는 것을 막는 상한 */
const MAX_STDERR = 4 * 1024 * 1024

/**
 * ssh2 로 원격 호스트의 git 을 실행한다. hostSpec 은 ~/.ssh/config 별칭 또는 user@host[:port].
 * 연결은 executor 당 하나를 재사용하고(끊기면 다음 호출에서 재연결), 채널은 동시 6개로 제한한다.
 * 인증은 agent 와 암호 없는 키 파일뿐이며 비밀번호는 쓰지 않는다.
 */
export class SshExecutor implements GitExecutor {
  readonly kind = 'ssh' as const
  private readonly conn: SshConnection

  constructor(
    readonly hostSpec: string,
    opts: SshExecutorOptions = {}
  ) {
    const homeDir = opts.homeDir ?? homedir()
    this.conn = new SshConnection({
      hostSpec,
      homeDir,
      configPath: opts.configPath ?? join(homeDir, '.ssh', 'config'),
      knownHostsPath: opts.knownHostsPath ?? join(homeDir, '.ssh', 'known_hosts'),
      agent: opts.agent
    })
  }

  async run(repoPath: string, args: string[], opts: RunOpts = {}): Promise<RunResult> {
    if (opts.signal?.aborted) throw new AbortError()
    const cmd = buildRemoteCommand(repoPath, args, { ...GIT_ENV, ...opts.env })
    const { channel, release } = await this.conn.exec(cmd, opts.signal)
    const maxBuffer = opts.maxBuffer ?? DEFAULT_MAX_BUFFER
    return new Promise<RunResult>((resolve, reject) => {
      const out: Buffer[] = []
      const err: Buffer[] = []
      let size = 0
      let errSize = 0
      let code: number | null = null
      let settled = false

      const finish = (fn: () => void): void => {
        if (settled) return
        settled = true
        opts.signal?.removeEventListener('abort', onAbort)
        fn()
      }
      const onAbort = (): void => {
        killChannel(channel)
        finish(() => reject(new AbortError()))
      }
      opts.signal?.addEventListener('abort', onAbort, { once: true })

      channel.on('data', (chunk: Buffer) => {
        if (settled) return
        size += chunk.length
        if (size > maxBuffer) {
          killChannel(channel)
          finish(() => reject(new GitError(`output exceeded ${maxBuffer} bytes`, -1, '', args)))
          return
        }
        out.push(chunk)
      })
      channel.stderr.on('data', (chunk: Buffer) => {
        errSize += chunk.length
        if (errSize <= MAX_STDERR) err.push(chunk)
      })
      channel.on('exit', (c: number | null) => {
        code = c ?? -1 // 시그널로 죽은 경우
      })
      channel.on('error', (e: Error) => finish(() => reject(e)))
      channel.on('close', () => {
        release()
        finish(() => {
          if (code === null) reject(new Error(CONNECTION_LOST))
          else resolve({ stdout: Buffer.concat(out), stderr: Buffer.concat(err).toString('utf8'), code })
        })
      })
    })
  }

  /**
   * stdout 을 흘려준다. 채널(Readable)을 for-await 로 읽으므로 소비자가 느리면 읽기가 멈추고,
   * ssh2 가 윈도우 갱신을 미루어 원격 git 의 출력도 SSH 흐름 제어로 멈춘다(메모리 무한 증가 없음).
   */
  async *stream(repoPath: string, args: string[], opts: RunOpts = {}): AsyncIterable<Buffer> {
    if (opts.signal?.aborted) throw new AbortError()
    const cmd = buildRemoteCommand(repoPath, args, { ...GIT_ENV, ...opts.env })
    const { channel, release } = await this.conn.exec(cmd, opts.signal)

    const err: Buffer[] = []
    let errSize = 0
    let code: number | null = null
    channel.stderr.on('data', (chunk: Buffer) => {
      errSize += chunk.length
      if (errSize <= MAX_STDERR) err.push(chunk)
    })
    channel.on('exit', (c: number | null) => {
      code = c ?? -1
    })
    const closed = new Promise<void>((resolve) =>
      channel.once('close', () => {
        release()
        resolve()
      })
    )
    const onAbort = (): void => killChannel(channel)
    opts.signal?.addEventListener('abort', onAbort, { once: true })
    try {
      try {
        // destroyOnReturn: false — 소비자가 break 하면 for-await 가 채널을 먼저 파기해 버려
        // 원격에 KILL/close 를 보내지 못하므로, 정리는 아래 finally 의 killChannel 이 맡는다
        for await (const chunk of channel.iterator({ destroyOnReturn: false })) yield chunk as Buffer
      } catch (e) {
        if (opts.signal?.aborted) throw new AbortError()
        // 종료 코드를 이미 받았다면 닫힘 순서 문제일 뿐이므로 아래 판정으로 넘어간다
        if ((e as NodeJS.ErrnoException).code !== 'ERR_STREAM_PREMATURE_CLOSE') throw e
      }
      await closed
      if (opts.signal?.aborted) throw new AbortError()
      if (code === null) throw new Error(CONNECTION_LOST)
      if (code !== 0) {
        const stderr = Buffer.concat(err).toString('utf8')
        throw new GitError(stderr.trim() || `git exited with ${code}`, code, stderr, args)
      }
    } finally {
      // 소비자가 중간에 break 해도 원격 프로세스를 남기지 않는다. 슬롯은 close 를 기다리지 않고 바로 돌려준다
      // (닫히는 중인 채널이 잠시 하나 더 열려 있을 수 있지만, 슬롯이 새어 모든 요청이 멈추는 것보다 낫다)
      opts.signal?.removeEventListener('abort', onAbort)
      killChannel(channel)
      release()
    }
  }

  dispose(): Promise<void> {
    return this.conn.dispose()
  }
}

/** UI 용 호스트 목록(~/.ssh/config 의 구체 별칭). 파일이 없으면 빈 배열. */
export async function listSshHosts(configPath?: string): Promise<SshHostInfo[]> {
  const homeDir = homedir()
  const config = loadSshConfig(configPath ?? join(homeDir, '.ssh', 'config'), { homeDir })
  return listHosts(config)
}
