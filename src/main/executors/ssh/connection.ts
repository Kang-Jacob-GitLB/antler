// ssh2 연결 수명 관리: 지연 연결·재사용·재연결, ProxyJump 체인, 인증 수단 구성, 채널 동시성 제한, 유휴 종료.
// Windows OpenSSH 는 ControlMaster 를 지원하지 않으므로 프로세스 안에서 연결 하나를 직접 유지한다.
//
// 인증 순서: agent → IdentityFile 키(없으면 ~/.ssh/id_ed25519, id_ecdsa, id_rsa). 비밀번호·keyboard-interactive 는 쓰지 않는다.
// 암호화된 개인키는 passphrase 를 저장·질문하지 않으므로 건너뛰고(agent 사용을 안내) 사유를 실패 메시지에 싣는다.
import { existsSync, readFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import type { Duplex } from 'node:stream'
import { BaseAgent, Client, createAgent, utils, type AnyAuthMethod, type ClientChannel, type ParsedKey, type ServerHostKeyAlgorithm } from 'ssh2'
import { AbortError } from '../GitExecutor'
import { loadSshConfig, resolveHost, type ResolvedHost } from './config'
import { knownHostKeyAlgorithms, readKnownHosts, verifyHostKey } from './knownHosts'

export class SshConnectError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'SshConnectError'
  }
}

export interface SshConnectionOptions {
  hostSpec: string
  configPath: string
  knownHostsPath: string
  /** undefined: 설정/기본값 · null: agent 사용 안 함 · string: 소켓·파이프 경로 */
  agent?: string | null
  homeDir: string
  readyTimeout?: number
  keepaliveInterval?: number
  idleTimeout?: number
  /** sshd MaxSessions(기본 10) 안쪽으로 유지할 동시 채널 수 */
  maxChannels?: number
}

export interface ExecChannel {
  channel: ClientChannel
  /** 채널 슬롯 반환(여러 번 불러도 한 번만 반영) */
  release(): void
}

/** 원격 프로세스에 KILL 을 요청하고 채널을 닫는다. 응답 없는 서버를 위해 잠시 뒤 강제 파기한다. */
export function killChannel(ch: ClientChannel): void {
  if (ch.destroyed) return
  try {
    ch.signal('KILL')
  } catch {
    // 이미 닫힌 채널
  }
  try {
    ch.close()
  } catch {
    // 이미 닫힌 채널
  }
  setTimeout(() => {
    if (!ch.destroyed) ch.destroy()
  }, 1000).unref()
}

const DEFAULT_KEY_NAMES = ['id_ed25519', 'id_ecdsa', 'id_rsa']
const WIN_AGENT_PIPE = '\\\\.\\pipe\\openssh-ssh-agent'

/** IdentitiesOnly 용: 허용된 공개키 blob 과 일치하는 agent 키만 노출한다. */
class FilteredAgent extends BaseAgent {
  constructor(
    private readonly inner: BaseAgent,
    private readonly allowed: Buffer[]
  ) {
    super()
  }

  getIdentities(cb: Parameters<BaseAgent['getIdentities']>[0]): void {
    this.inner.getIdentities((err, keys) => {
      if (err || !keys) return cb(err ?? new Error('agent 키 목록 없음'))
      cb(
        undefined,
        keys.filter((k) => {
          const parsed = utils.parseKey(k as Buffer)
          const key = Array.isArray(parsed) ? parsed[0] : parsed
          if (!key || key instanceof Error) return false
          const blob = key.getPublicSSH()
          return this.allowed.some((a) => a.equals(blob))
        })
      )
    })
  }

  sign(pubKey: never, data: Buffer, options: never, cb?: never): void {
    ;(this.inner.sign as (...a: unknown[]) => void)(pubKey, data, options, cb)
  }
}

function firstKey(parsed: ParsedKey | ParsedKey[] | Error): ParsedKey | Error {
  return Array.isArray(parsed) ? (parsed[0] ?? new Error('빈 키 파일')) : parsed
}

function defaultAgentPath(): string | undefined {
  if (process.platform === 'win32') return existsSync(WIN_AGENT_PIPE) ? WIN_AGENT_PIPE : 'pageant'
  return process.env.SSH_AUTH_SOCK || undefined
}

class Waiter {
  constructor(readonly grant: () => void) {}
}

export class SshConnection {
  private chain: Client[] = []
  private ready: Client | null = null
  private connecting: Promise<Client> | null = null
  private active = 0
  private readonly waiters: Waiter[] = []
  private idleTimer: NodeJS.Timeout | null = null
  private disposed = false

  constructor(private readonly opts: SshConnectionOptions) {}

  private get maxChannels(): number {
    return this.opts.maxChannels ?? 6
  }

  /** 연결을 보장하고 exec 채널을 연다. 동시 채널은 세마포어로 제한한다. */
  async exec(command: string, signal?: AbortSignal): Promise<ExecChannel> {
    await this.acquireSlot(signal)
    let released = false
    const release = (): void => {
      if (released) return
      released = true
      this.releaseSlot()
    }
    try {
      const client = await this.withAbort(this.connect(), signal)
      const channel = await new Promise<ClientChannel>((resolve, reject) => {
        client.exec(command, (err, ch) => (err ? reject(this.wrapExecError(err)) : resolve(ch)))
      })
      if (signal?.aborted) {
        killChannel(channel)
        throw new AbortError()
      }
      channel.once('close', release)
      return { channel, release }
    } catch (e) {
      release()
      throw e
    }
  }

  async dispose(): Promise<void> {
    this.disposed = true
    this.teardown()
    for (const w of this.waiters.splice(0)) w.grant()
  }

  private withAbort<T>(p: Promise<T>, signal?: AbortSignal): Promise<T> {
    if (!signal) return p
    return new Promise<T>((resolve, reject) => {
      const onAbort = (): void => reject(new AbortError())
      signal.addEventListener('abort', onAbort, { once: true })
      p.then(resolve, reject).finally(() => signal.removeEventListener('abort', onAbort))
    })
  }

  private acquireSlot(signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) return Promise.reject(new AbortError())
    if (this.disposed) return Promise.reject(new SshConnectError('SSH 연결이 이미 종료되었습니다'))
    this.clearIdle()
    if (this.active < this.maxChannels) {
      this.active++
      return Promise.resolve()
    }
    return new Promise<void>((resolve, reject) => {
      const w = new Waiter(() => {
        signal?.removeEventListener('abort', onAbort)
        if (this.disposed) reject(new SshConnectError('SSH 연결이 이미 종료되었습니다'))
        else resolve() // 슬롯을 넘겨받는다(active 유지)
      })
      const onAbort = (): void => {
        const i = this.waiters.indexOf(w)
        if (i >= 0) this.waiters.splice(i, 1)
        reject(new AbortError())
      }
      signal?.addEventListener('abort', onAbort, { once: true })
      this.waiters.push(w)
    })
  }

  private releaseSlot(): void {
    const next = this.waiters.shift()
    if (next) return next.grant()
    this.active--
    if (this.active === 0) this.startIdle()
  }

  private startIdle(): void {
    this.clearIdle()
    if (!this.ready) return
    this.idleTimer = setTimeout(() => {
      if (this.active === 0) this.teardown()
    }, this.opts.idleTimeout ?? 5 * 60_000)
    this.idleTimer.unref()
  }

  private clearIdle(): void {
    if (this.idleTimer) clearTimeout(this.idleTimer)
    this.idleTimer = null
  }

  private teardown(): void {
    this.clearIdle()
    const chain = this.chain
    this.chain = []
    this.ready = null
    for (const c of chain.reverse()) {
      try {
        c.destroy()
      } catch {
        // 이미 닫힘
      }
    }
  }

  private connect(): Promise<Client> {
    if (this.ready) return Promise.resolve(this.ready)
    if (!this.connecting) {
      this.connecting = this.doConnect().finally(() => {
        this.connecting = null
      })
    }
    return this.connecting
  }

  private async doConnect(): Promise<Client> {
    // 연결할 때마다 설정을 다시 읽는다(사용자가 ~/.ssh/config 를 고친 뒤 재연결하면 반영)
    const config = loadSshConfig(this.opts.configPath, { homeDir: this.opts.homeDir })
    const target = resolveHost(config, this.opts.hostSpec)
    const hops = target.proxyJump.map((h) => resolveHost(config, h.spec))
    const nodes = [...hops, target]
    const clients: Client[] = []
    try {
      let sock: Duplex | undefined
      for (let i = 0; i < nodes.length; i++) {
        const node = nodes[i]!
        const via = i < hops.length ? `점프 호스트 ${node.alias}` : undefined
        const client = await this.connectOne(node, sock, via)
        clients.push(client)
        const next = nodes[i + 1]
        if (next) sock = await this.forward(client, node, next)
      }
    } catch (e) {
      for (const c of clients) c.destroy()
      throw e
    }
    if (this.disposed) {
      for (const c of clients) c.destroy()
      throw new SshConnectError('SSH 연결이 이미 종료되었습니다')
    }
    const last = clients[clients.length - 1]!
    this.chain = clients
    this.ready = last
    for (const c of clients) {
      c.on('error', () => {}) // 연결 후의 소켓 오류는 close 로 정리한다
      c.on('close', () => {
        if (this.ready === last) this.teardown()
      })
    }
    return last
  }

  private forward(client: Client, from: ResolvedHost, to: ResolvedHost): Promise<Duplex> {
    return new Promise((resolve, reject) => {
      client.forwardOut('127.0.0.1', 0, to.hostName, to.port, (err, stream) => {
        if (err) {
          reject(new SshConnectError(`점프 호스트 ${from.alias} 에서 ${to.hostName}:${to.port} 로 연결하지 못했습니다: ${err.message}`))
        } else resolve(stream)
      })
    })
  }

  private resolveAgent(node: ResolvedHost): string | undefined {
    const setting = this.opts.agent !== undefined ? this.opts.agent : node.identityAgent
    if (setting === null) return undefined
    return setting ?? defaultAgentPath()
  }

  private publicBlobs(files: string[]): Buffer[] {
    const out: Buffer[] = []
    for (const f of files) {
      for (const source of [`${f}.pub`, f]) {
        try {
          const key = firstKey(utils.parseKey(readFileSync(source)))
          if (!(key instanceof Error)) {
            out.push(key.getPublicSSH())
            break
          }
        } catch {
          // 없으면 다음 후보
        }
      }
    }
    return out
  }

  /** 시도할 인증 수단 목록. 건너뛴 사유는 notes 에 쌓는다. */
  private buildAuth(node: ResolvedHost, notes: string[]): AnyAuthMethod[] {
    const auth: AnyAuthMethod[] = []
    const explicit = node.identityFiles.length > 0
    const files = explicit ? node.identityFiles : DEFAULT_KEY_NAMES.map((n) => join(this.opts.homeDir, '.ssh', n))

    const agentPath = this.resolveAgent(node)
    if (agentPath) {
      let agent: BaseAgent = createAgent(agentPath)
      if (node.identitiesOnly) agent = new FilteredAgent(agent, this.publicBlobs(files))
      auth.push({ type: 'agent' as const, username: node.user, agent })
    }
    for (const f of files) {
      let buf: Buffer
      try {
        buf = readFileSync(f)
      } catch {
        if (explicit) notes.push(`키 파일을 읽을 수 없어 건너뜀: ${f}`)
        continue
      }
      const key = firstKey(utils.parseKey(buf))
      if (key instanceof Error) {
        notes.push(
          /encrypted/i.test(key.message)
            ? `암호화된 키라 건너뜀(passphrase 는 저장·질문하지 않음): ${basename(f)} — ssh-add 로 agent 에 등록하세요.`
            : `지원하지 않는 키 형식이라 건너뜀: ${basename(f)}`
        )
        continue
      }
      auth.push({ type: 'publickey' as const, username: node.user, key })
    }
    return auth
  }

  private connectOne(node: ResolvedHost, sock: Duplex | undefined, via?: string): Promise<Client> {
    const prefix = via ? `[${via}] ` : ''
    const notes: string[] = []
    const auth = this.buildAuth(node, notes)
    if (auth.length === 0) {
      return Promise.reject(
        new SshConnectError(
          `${prefix}사용할 수 있는 인증 수단이 없습니다 (${node.user}@${node.hostName}). ` +
            `ssh-agent 를 실행해 키를 등록하거나 암호 없는 개인키를 IdentityFile 로 지정하세요. ${notes.join(' ')}`.trim()
        )
      )
    }
    const readyTimeout = this.opts.readyTimeout ?? 15_000
    const algs = knownHostKeyAlgorithms(readKnownHosts(this.opts.knownHostsPath), node.hostName, node.port)
    let verdict: string | undefined

    return new Promise<Client>((resolve, reject) => {
      const client = new Client()
      let settled = false
      const fail = (e: SshConnectError): void => {
        if (settled) return
        settled = true
        client.destroy()
        reject(e)
      }
      client.on('ready', () => {
        if (settled) return
        settled = true
        resolve(client)
      })
      client.on('error', (err: Error & { level?: string; code?: string }) => {
        if (err.level === 'agent') {
          notes.push(`agent 사용 불가(${err.message})`)
          return
        }
        fail(new SshConnectError(prefix + this.describeError(err, node, notes, verdict, readyTimeout)))
      })
      client.on('close', () => {
        fail(new SshConnectError(prefix + (verdict ?? `연결이 닫혔습니다 (${node.hostName}:${node.port})`)))
      })
      // password·tryKeyboard 는 의도적으로 지정하지 않는다(키/agent 인증만)
      client.connect({
        host: node.hostName,
        port: node.port,
        username: node.user,
        sock,
        authHandler: auth,
        tryKeyboard: false,
        readyTimeout,
        keepaliveInterval: this.opts.keepaliveInterval ?? 15_000,
        keepaliveCountMax: 3,
        ...(algs.length ? { algorithms: { serverHostKey: algs as ServerHostKeyAlgorithm[] } } : {}),
        hostVerifier: (key: Buffer): boolean => {
          const r = verifyHostKey(this.opts.knownHostsPath, node.hostName, node.port, key)
          if (!r.ok) verdict = r.message
          return r.ok
        }
      })
    })
  }

  private describeError(
    err: Error & { level?: string; code?: string },
    node: ResolvedHost,
    notes: string[],
    verdict: string | undefined,
    readyTimeout: number
  ): string {
    if (verdict) return verdict
    const where = `${node.hostName}:${node.port}`
    if (err.level === 'client-authentication') {
      return (
        `인증에 실패했습니다 (${node.user}@${where}). 비밀번호 인증은 지원하지 않습니다 — ` +
        `서버에 공개키를 등록하고 ssh-agent 에 키를 올리거나 IdentityFile 을 지정하세요. ${notes.join(' ')}`.trim()
      )
    }
    if (err.code === 'ECONNREFUSED') return `연결이 거부되었습니다 (${where}). sshd 가 떠 있는지, 포트가 맞는지 확인하세요.`
    if (err.code === 'ENOTFOUND' || err.code === 'EAI_AGAIN') return `호스트 이름을 찾을 수 없습니다 (${node.hostName}).`
    if (err.level === 'client-timeout' || err.code === 'ETIMEDOUT') {
      return `연결 시간이 초과되었습니다 (${where}, ${Math.round(readyTimeout / 1000)}초).`
    }
    return `SSH 연결에 실패했습니다 (${where}): ${err.message}`
  }

  private wrapExecError(err: Error): Error {
    return new SshConnectError(`원격 명령을 시작하지 못했습니다: ${err.message}`)
  }
}
