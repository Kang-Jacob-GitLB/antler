// SSH 테스트 공용 도구. 실제 원격 호스트에는 절대 접속하지 않는다 — 모든 통합 테스트는 127.0.0.1 의
// 프로세스 내 ssh2 Server 와 테스트가 만든 임시 키·config·known_hosts 만 쓴다.
import { spawn, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import net from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Server, utils, type ParsedKey } from 'ssh2'

/** POSIX 셸 인용 해제(작은따옴표·큰따옴표·백슬래시). 원격 셸이 하는 일을 흉내 내 argv 를 복원한다. */
export function unquote(cmd: string): string[] {
  const args: string[] = []
  let cur = ''
  let has = false
  for (let i = 0; i < cmd.length; i++) {
    const ch = cmd[i]!
    if (ch === "'") {
      const end = cmd.indexOf("'", i + 1)
      if (end < 0) throw new Error('닫히지 않은 작은따옴표')
      cur += cmd.slice(i + 1, end)
      i = end
      has = true
    } else if (ch === '"') {
      i++
      for (; i < cmd.length && cmd[i] !== '"'; i++) {
        if (cmd[i] === '\\' && '"\\$`'.includes(cmd[i + 1] ?? '')) i++
        cur += cmd[i]
      }
      has = true
    } else if (ch === '\\') {
      i++
      cur += cmd[i] ?? ''
      has = true
    } else if (ch === ' ' || ch === '\t' || ch === '\n') {
      if (has) args.push(cur)
      cur = ''
      has = false
    } else {
      cur += ch
      has = true
    }
  }
  if (has) args.push(cur)
  return args
}

export interface KeyPair {
  privatePem: string
  publicText: string
  parsed: ParsedKey
  blob: Buffer
}

export function newKeyPair(): KeyPair {
  // ssh2 의 ed25519 키 생성은 약 0.3% 확률로 31바이트 공개키(파싱 불가)를 낸다 — 파싱되는 키가 나올 때까지 다시 만든다
  for (;;) {
    const kp = utils.generateKeyPairSync('ed25519')
    const parsed = utils.parseKey(kp.public)
    const priv = utils.parseKey(kp.private)
    if (parsed instanceof Error || Array.isArray(parsed) || priv instanceof Error) continue
    return { privatePem: kp.private, publicText: kp.public, parsed, blob: parsed.getPublicSSH() }
  }
}

function killTree(pid: number | undefined, child: { kill(): void }): void {
  if (process.platform === 'win32' && pid !== undefined) {
    spawn('taskkill', ['/pid', String(pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' }).on('error', () => child.kill())
  } else child.kill()
}

export interface TestServer {
  port: number
  hostKey: KeyPair
  /** 지금까지 받은 SSH 연결 수 */
  connections: () => number
  /** 실행 중인 git 자식 프로세스 수 */
  activeChildren: () => number
  /** 열려 있는 모든 클라이언트 연결을 서버 쪽에서 끊는다 */
  dropClients: () => void
  close: () => Promise<void>
}

export async function startServer(opts: { authorized: KeyPair[]; hostKey?: KeyPair }): Promise<TestServer> {
  const hostKey = opts.hostKey ?? newKeyPair()
  let connections = 0
  let active = 0
  const clients = new Set<{ end(): void }>()
  const server = new Server({ hostKeys: [hostKey.privatePem] }, (client) => {
    connections++
    clients.add(client)
    // sshd 처럼 연결이 끊기면 그 연결의 세션 프로세스도 정리한다
    const kills = new Set<() => void>()
    client.on('close', () => {
      clients.delete(client)
      for (const k of [...kills]) k()
    })
    client.on('error', () => {})
    client.on('authentication', (ctx) => {
      const ok = ctx.method === 'publickey' && opts.authorized.find((k) => k.blob.equals(ctx.key.data))
      if (!ok) return ctx.reject(['publickey'])
      if (ctx.signature && ok.parsed.verify(ctx.blob!, ctx.signature, ctx.hashAlgo) !== true) return ctx.reject()
      ctx.accept()
    })
    client.on('ready', () => {
      client.on('session', (accept) => {
        const session = accept()
        session.on('exec', (acceptExec, _reject, info) => {
          const stream = acceptExec()
          const argv = unquote(info.command)
          const env: Record<string, string> = {}
          let i = 1 // argv[0] === 'env'
          for (; i < argv.length && argv[i] !== 'git'; i++) {
            const eq = argv[i]!.indexOf('=')
            env[argv[i]!.slice(0, eq)] = argv[i]!.slice(eq + 1)
          }
          const child = spawn('git', argv.slice(i + 1), { env: { ...process.env, ...env }, windowsHide: true })
          active++
          child.stdout.pipe(stream, { end: false })
          child.stderr.pipe(stream.stderr, { end: false })
          child.on('close', (code) => {
            active--
            kills.delete(kill)
            stream.exit(code ?? 255)
            stream.end()
          })
          const kill = (): void => {
            killTree(child.pid, child)
            // 소비되지 않고 쌓인 stdout 때문에 close 가 영영 안 오는 것을 막는다
            child.stdout.destroy()
            child.stderr.destroy()
          }
          kills.add(kill)
          session.on('signal', (acceptSig) => {
            acceptSig?.()
            kill()
          })
          stream.on('close', kill)
        })
      })
      // ProxyJump 용 direct-tcpip: 대상 포트로 그대로 이어 준다
      client.on('tcpip', (accept, reject, info) => {
        const sock = net.connect(info.destPort, info.destIP)
        sock.on('connect', () => {
          const ch = accept()
          ch.pipe(sock).pipe(ch)
          ch.on('close', () => sock.destroy())
        })
        sock.on('error', () => reject())
      })
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as net.AddressInfo).port
  return {
    port,
    hostKey,
    connections: () => connections,
    activeChildren: () => active,
    dropClients: () => clients.forEach((c) => c.end()),
    close: () =>
      new Promise<void>((resolve) => {
        clients.forEach((c) => c.end())
        server.close(() => resolve())
      })
  }
}

export interface SshEnv {
  home: string
  configPath: string
  knownHostsPath: string
  keyPath: string
}

/** 임시 홈(~/.ssh 포함)을 만들고 클라이언트 개인키를 써 둔다. */
export function makeHome(clientKey: KeyPair): SshEnv {
  const home = mkdtempSync(join(tmpdir(), 'antler-ssh-'))
  const dir = join(home, '.ssh')
  mkdirSync(dir)
  const keyPath = join(dir, 'id_test')
  writeFileSync(keyPath, clientKey.privatePem, { mode: 0o600 })
  return { home, configPath: join(dir, 'config'), knownHostsPath: join(dir, 'known_hosts'), keyPath }
}

export function knownHostsLine(port: number, key: KeyPair): string {
  return `[127.0.0.1]:${port} ssh-ed25519 ${key.blob.toString('base64')}\n`
}

/** 한글 커밋 메시지·큰 바이너리가 든 임시 저장소 */
export function makeRepo(bigBytes = 0): string {
  const dir = mkdtempSync(join(tmpdir(), 'antler-sshrepo-'))
  const git = (...a: string[]): void => {
    const r = spawnSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], {
      cwd: dir,
      encoding: 'utf8'
    })
    if (r.status !== 0) throw new Error(`git ${a.join(' ')}: ${r.stderr}`)
  }
  git('init', '-q', '-b', 'main')
  writeFileSync(join(dir, 'a.txt'), 'hello\n')
  if (bigBytes > 0) writeFileSync(join(dir, 'big.bin'), Buffer.alloc(bigBytes, 'x'))
  git('add', '-A')
  git('commit', '-q', '-m', '첫 커밋: 한글 메시지')
  return dir
}
