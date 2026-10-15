// SSH 테스트 공용 도구. 실제 원격 호스트에는 절대 접속하지 않는다 — 모든 통합 테스트는 127.0.0.1 의
// 프로세스 내 ssh2 Server 와 테스트가 만든 임시 키·config·known_hosts 만 쓴다.
import { spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readdirSync, statSync, writeFileSync, type Stats } from 'node:fs'
import net from 'node:net'
import { tmpdir } from 'node:os'
import { join, posix } from 'node:path'
import { Server, utils, type Attributes, type ParsedKey, type SFTPWrapper } from 'ssh2'
import { SHELL_PROBE, SHELL_PROBE_EXPECT } from '../src/main/executors/ssh/connection'

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

/**
 * shell: 원격 로그인 셸 흉내. 'posix' 면 셸 확인 명령에 POSIX 처럼 답하고, 'cmd' 면 Windows cmd 처럼
 * 작은따옴표를 문자 그대로 돌려준다(SshConnection 의 셸 확인이 거부해야 한다).
 */
export async function startServer(opts: {
  authorized: KeyPair[]
  hostKey?: KeyPair
  shell?: 'posix' | 'cmd'
  /** SFTP 서브시스템을 켠다: 가상 '/' 를 이 로컬 폴더로, 홈은 /home/tester 로 둔다 */
  sftpRoot?: string
}): Promise<TestServer> {
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
        session.on('sftp', (acceptSftp, rejectSftp) => {
          if (!opts.sftpRoot) return rejectSftp()
          serveSftp(acceptSftp(), opts.sftpRoot)
        })
        session.on('exec', (acceptExec, _reject, info) => {
          const stream = acceptExec()
          if (info.command === SHELL_PROBE) {
            stream.write(opts.shell === 'cmd' ? "'an''tler'\r\n" : `${SHELL_PROBE_EXPECT}\n`)
            stream.exit(0)
            stream.end()
            return
          }
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

/** 읽기 전용 최소 SFTP 서버(REALPATH·OPENDIR·READDIR·STAT·LSTAT·CLOSE). 가상 경로는 POSIX, 홈 = /home/tester */
function serveSftp(sftp: SFTPWrapper, root: string): void {
  const HOME = '/home/tester'
  const real = (v: string): string => join(root, ...v.split('/').filter(Boolean))
  const virt = (p: string): string => posix.resolve(HOME, p)
  const attrsOf = (st: Stats): Attributes => ({
    mode: (st.isDirectory() ? 0o040000 : 0o100000) | 0o755,
    uid: 0,
    gid: 0,
    size: st.size,
    atime: Math.floor(st.atimeMs / 1000),
    mtime: Math.floor(st.mtimeMs / 1000)
  })
  const dirs = new Map<number, { path: string; sent: boolean }>()
  let next = 1
  sftp.on('REALPATH', (id, p) => {
    const v = virt(p)
    sftp.name(id, [{ filename: v, longname: v, attrs: {} as Attributes }])
  })
  const stat = (id: number, p: string): void => {
    try {
      sftp.attrs(id, attrsOf(statSync(real(virt(p)))))
    } catch {
      sftp.status(id, utils.sftp.STATUS_CODE.NO_SUCH_FILE)
    }
  }
  sftp.on('STAT', stat)
  sftp.on('LSTAT', stat)
  sftp.on('OPENDIR', (id, p) => {
    const v = virt(p)
    if (!existsSync(real(v))) return sftp.status(id, utils.sftp.STATUS_CODE.NO_SUCH_FILE)
    const h = next++
    dirs.set(h, { path: v, sent: false })
    const buf = Buffer.alloc(4)
    buf.writeUInt32BE(h)
    sftp.handle(id, buf)
  })
  sftp.on('READDIR', (id, handle) => {
    const d = dirs.get(handle.readUInt32BE(0))
    if (!d) return sftp.status(id, utils.sftp.STATUS_CODE.FAILURE)
    if (d.sent) return sftp.status(id, utils.sftp.STATUS_CODE.EOF)
    d.sent = true
    const names = readdirSync(real(d.path)).map((n) => ({ filename: n, longname: n, attrs: attrsOf(statSync(join(real(d.path), n))) }))
    sftp.name(id, names)
  })
  sftp.on('CLOSE', (id, handle) => {
    dirs.delete(handle.readUInt32BE(0))
    sftp.status(id, utils.sftp.STATUS_CODE.OK)
  })
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
