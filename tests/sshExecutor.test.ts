// SshExecutor 통합 테스트 — 127.0.0.1 의 프로세스 내 ssh2 Server 만 쓴다(실제 원격·사용자 ~/.ssh 는 건드리지 않는다).
import { writeFileSync } from 'node:fs'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { AbortError, GitError } from '../src/main/executors/GitExecutor'
import { SshExecutor, listSshHosts } from '../src/main/executors/SshExecutor'
import { knownHostsLine, makeHome, makeRepo, newKeyPair, startServer, type SshEnv, type TestServer } from './sshHarness'

const BIG = 24 * 1024 * 1024
const clientKey = newKeyPair()
let repo: string
let server: TestServer
let env: SshEnv

function writeConfig(extra = ''): void {
  writeFileSync(
    env.configPath,
    `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${env.keyPath}\n  IdentitiesOnly yes\n${extra}`
  )
}

function executor(spec = 'test', e = env): SshExecutor {
  return new SshExecutor(spec, { configPath: e.configPath, knownHostsPath: e.knownHostsPath, agent: null, homeDir: e.home })
}

/** 앞선 테스트가 남긴 서버 쪽 git 프로세스가 정리될 때까지 기다린다 */
async function idle(): Promise<void> {
  for (let i = 0; i < 100 && server.activeChildren() > 0; i++) await new Promise((r) => setTimeout(r, 50))
}

async function drain(it: AsyncIterable<Buffer>): Promise<number> {
  let n = 0
  for await (const c of it) n += c.length
  return n
}

beforeAll(async () => {
  repo = makeRepo(BIG)
  server = await startServer({ authorized: [clientKey] })
  env = makeHome(clientKey)
  writeConfig()
  writeFileSync(env.knownHostsPath, knownHostsLine(server.port, server.hostKey))
})

afterAll(async () => {
  await server.close()
})

describe('SshExecutor (in-process ssh2 server)', { timeout: 30_000 }, () => {
  it('run: rev-parse / 한글 커밋 메시지, 연결은 재사용된다', async () => {
    const ex = executor()
    const before = server.connections()
    const r = await ex.run(repo, ['rev-parse', '--is-inside-work-tree'])
    expect(r.code).toBe(0)
    expect(r.stdout.toString().trim()).toBe('true')
    const log = await ex.run(repo, ['log', '-1', '--format=%s'])
    expect(log.stdout.toString('utf8').trim()).toBe('첫 커밋: 한글 메시지')
    await Promise.all([1, 2, 3].map(() => ex.run(repo, ['rev-parse', 'HEAD'])))
    expect(server.connections() - before).toBe(1)
    await ex.dispose()
  })

  it('run: 실패 코드와 stderr 를 그대로 돌려준다', async () => {
    const ex = executor()
    const r = await ex.run(repo, ['rev-parse', '--verify', 'refs/heads/없는_브랜치'])
    expect(r.code).not.toBe(0)
    expect(r.stderr.length).toBeGreaterThan(0)
    await ex.dispose()
  })

  it('run: 공백·따옴표가 든 인자도 그대로 전달된다', async () => {
    const ex = executor()
    const r = await ex.run(repo, ['log', '-1', "--format=it's $HOME `x` 한글 %s"])
    expect(r.stdout.toString('utf8').trim()).toBe("it's $HOME `x` 한글 첫 커밋: 한글 메시지")
    await ex.dispose()
  })

  it('run: maxBuffer 초과는 GitError', async () => {
    const ex = executor()
    await expect(ex.run(repo, ['cat-file', 'blob', 'HEAD:big.bin'], { maxBuffer: 1024 })).rejects.toBeInstanceOf(GitError)
    await ex.dispose()
  })

  it('run: 이미 취소된 signal 이면 AbortError', async () => {
    const ex = executor()
    const ac = new AbortController()
    ac.abort()
    await expect(ex.run(repo, ['rev-parse', 'HEAD'], { signal: ac.signal })).rejects.toBeInstanceOf(AbortError)
    await ex.dispose()
  })

  it('stream: 큰 출력을 느린 소비자에게도 온전히 전달하고 원격은 backpressure 로 멈춘다', async () => {
    const ex = executor()
    await idle()
    let total = 0
    let first = true
    for await (const c of ex.stream(repo, ['cat-file', 'blob', 'HEAD:big.bin'])) {
      total += c.length
      if (first) {
        first = false
        await new Promise((r) => setTimeout(r, 800))
        // 소비를 멈춘 동안 24MB 가 이미 다 흘러 나갔다면 backpressure 가 없는 것이다
        expect(server.activeChildren()).toBe(1)
      }
    }
    expect(total).toBe(BIG)
    await ex.dispose()
  })

  it('stream: 비정상 종료는 마지막에 GitError', async () => {
    const ex = executor()
    const got: Buffer[] = []
    let err: unknown
    try {
      for await (const c of ex.stream(repo, ['log', 'refs/heads/없는_브랜치'])) got.push(c)
    } catch (e) {
      err = e
    }
    expect(err).toBeInstanceOf(GitError)
    expect((err as GitError).code).not.toBe(0)
    expect((err as GitError).stderr).toMatch(/없는_브랜치|unknown revision|ambiguous/)
    await ex.dispose()
  })

  it('stream: 중간 abort 는 AbortError 이고 원격 프로세스도 정리된다', async () => {
    const ex = executor()
    const ac = new AbortController()
    let err: unknown
    try {
      for await (const _ of ex.stream(repo, ['cat-file', 'blob', 'HEAD:big.bin'], { signal: ac.signal })) {
        void _
        ac.abort()
      }
    } catch (e) {
      err = e
    }
    expect(err).toBeInstanceOf(AbortError)
    for (let i = 0; i < 50 && server.activeChildren() > 0; i++) await new Promise((r) => setTimeout(r, 100))
    expect(server.activeChildren()).toBe(0)
    // 같은 연결로 계속 쓸 수 있다
    expect((await ex.run(repo, ['rev-parse', 'HEAD'])).code).toBe(0)
    await ex.dispose()
  })

  it('stream: 소비자가 break 해도 원격 프로세스를 남기지 않는다', async () => {
    const ex = executor()
    for await (const _ of ex.stream(repo, ['cat-file', 'blob', 'HEAD:big.bin'])) {
      void _
      break
    }
    for (let i = 0; i < 50 && server.activeChildren() > 0; i++) await new Promise((r) => setTimeout(r, 100))
    expect(server.activeChildren()).toBe(0)
    await ex.dispose()
  })

  it('끊어진 연결은 다음 호출에서 지연 재연결한다', async () => {
    const ex = executor()
    await ex.run(repo, ['rev-parse', 'HEAD'])
    const before = server.connections()
    server.dropClients()
    await new Promise((r) => setTimeout(r, 300))
    expect((await ex.run(repo, ['rev-parse', 'HEAD'])).code).toBe(0)
    expect(server.connections() - before).toBe(1)
    await ex.dispose()
  })

  it('동시 채널이 한도를 넘어도 모두 처리한다', async () => {
    const ex = executor()
    const rs = await Promise.all(Array.from({ length: 20 }, () => ex.run(repo, ['rev-parse', 'HEAD'])))
    expect(rs.every((r) => r.code === 0)).toBe(true)
    await ex.dispose()
  })

  it('호출당 왕복 시간(로컬, ms)', async () => {
    const ex = executor()
    await ex.run(repo, ['rev-parse', 'HEAD']) // 연결·핸드셰이크 제외
    const n = 20
    const t0 = performance.now()
    for (let i = 0; i < n; i++) await ex.run(repo, ['rev-parse', 'HEAD'])
    const per = (performance.now() - t0) / n
    console.log(`[ssh] exec 왕복 평균 ${per.toFixed(1)}ms (git 프로세스 기동 포함, 로컬 in-process 서버)`)
    expect(per).toBeLessThan(2000)
    await ex.dispose()
  })

  it('user@host:port 직접 입력', async () => {
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host *\n  IdentityFile ${e2.keyPath}\n`)
    writeFileSync(e2.knownHostsPath, knownHostsLine(server.port, server.hostKey))
    const ex = executor(`tester@127.0.0.1:${server.port}`, e2)
    expect((await ex.run(repo, ['rev-parse', 'HEAD'])).code).toBe(0)
    await ex.dispose()
  })
})

describe('SshExecutor 거부 경로', () => {
  it('known_hosts 의 키와 다른 호스트 키는 거부한다(MITM 의심)', async () => {
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${e2.keyPath}\n`)
    writeFileSync(e2.knownHostsPath, knownHostsLine(server.port, newKeyPair()))
    const ex = executor('test', e2)
    await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/다릅니다.*중간자|중간자/)
    await ex.dispose()
  })

  it('알 수 없는 호스트는 거부하고 ssh CLI 로 수락하라고 안내한다(자동 등록 없음)', async () => {
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${e2.keyPath}\n`)
    writeFileSync(e2.knownHostsPath, '')
    const ex = executor('test', e2)
    const err = await ex.run(repo, ['rev-parse', 'HEAD']).catch((e: Error) => e)
    expect((err as Error).message).toMatch(/알 수 없는 호스트.*ssh CLI/)
    expect((err as Error).message).not.toContain('PRIVATE')
    await ex.dispose()
    const { readFileSync } = await import('node:fs')
    expect(readFileSync(e2.knownHostsPath, 'utf8')).toBe('')
  })

  it('서버가 모르는 클라이언트 키는 인증 오류', async () => {
    const stranger = newKeyPair()
    const e2 = makeHome(stranger)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${e2.keyPath}\n`)
    writeFileSync(e2.knownHostsPath, knownHostsLine(server.port, server.hostKey))
    const ex = executor('test', e2)
    const err = await ex.run(repo, ['rev-parse', 'HEAD']).catch((e: Error) => e)
    expect((err as Error).message).toMatch(/인증에 실패/)
    expect((err as Error).message).not.toContain('PRIVATE')
    await ex.dispose()
  })

  it('인증 수단이 전혀 없으면(키 없음·agent 없음) 즉시 안내한다', async () => {
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  IdentityFile ${e2.home}/없는키\n`)
    const ex = executor('test', e2)
    await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/인증 수단이 없습니다/)
    await ex.dispose()
  })

  it('연결 거부는 명확한 메시지', async () => {
    const dead = await startServer({ authorized: [] })
    const port = dead.port
    await dead.close()
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${port}\n  User t\n  IdentityFile ${e2.keyPath}\n`)
    const ex = executor('test', e2)
    await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/연결이 거부/)
    await ex.dispose()
  })

  it('dispose 뒤에는 사용할 수 없다', async () => {
    const ex = executor()
    await ex.dispose()
    await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/종료/)
  })

  it('원격 로그인 셸이 POSIX 가 아니면(cmd 등) 사용자 값이 든 명령을 보내기 전에 거부한다', async () => {
    const winbox = await startServer({ authorized: [clientKey], shell: 'cmd' })
    try {
      const e2 = makeHome(clientKey)
      writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${winbox.port}\n  User t\n  IdentityFile ${e2.keyPath}\n`)
      writeFileSync(e2.knownHostsPath, `${knownHostsLine(winbox.port, winbox.hostKey)}\n`)
      const ex = executor('test', e2)
      await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/POSIX 셸이 아닙니다/)
      await ex.dispose()
    } finally {
      await winbox.close()
    }
  })
})

describe('ProxyJump', () => {
  it('점프 호스트를 거쳐 대상에 접속한다(각 호스트 키를 따로 검증)', async () => {
    const jump = await startServer({ authorized: [clientKey] })
    const e2 = makeHome(clientKey)
    writeFileSync(
      e2.configPath,
      `Host jump\n  HostName 127.0.0.1\n  Port ${jump.port}\n  User jumper\n  IdentityFile ${e2.keyPath}\n` +
        `Host target\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${e2.keyPath}\n  ProxyJump jump\n`
    )
    writeFileSync(e2.knownHostsPath, knownHostsLine(jump.port, jump.hostKey) + knownHostsLine(server.port, server.hostKey))
    const ex = executor('target', e2)
    const jumpBefore = jump.connections()
    const r = await ex.run(repo, ['log', '-1', '--format=%s'])
    expect(r.stdout.toString('utf8').trim()).toBe('첫 커밋: 한글 메시지')
    expect(jump.connections() - jumpBefore).toBe(1)
    expect(await drain(ex.stream(repo, ['cat-file', 'blob', 'HEAD:a.txt']))).toBe(6)
    await ex.dispose()
    await jump.close()
  })

  it('점프 호스트 키가 틀리면 점프 단계에서 거부한다', async () => {
    const jump = await startServer({ authorized: [clientKey] })
    const e2 = makeHome(clientKey)
    writeFileSync(
      e2.configPath,
      `Host jump\n  HostName 127.0.0.1\n  Port ${jump.port}\n  IdentityFile ${e2.keyPath}\n` +
        `Host target\n  HostName 127.0.0.1\n  Port ${server.port}\n  IdentityFile ${e2.keyPath}\n  ProxyJump jump\n`
    )
    writeFileSync(e2.knownHostsPath, knownHostsLine(jump.port, newKeyPair()) + knownHostsLine(server.port, server.hostKey))
    const ex = executor('target', e2)
    await expect(ex.run(repo, ['rev-parse', 'HEAD'])).rejects.toThrow(/점프 호스트 jump.*중간자/)
    await ex.dispose()
    await jump.close()
  })
})

describe('listSshHosts', () => {
  it('config 의 구체 별칭을 돌려준다(없는 파일은 빈 배열)', async () => {
    const hosts = await listSshHosts(env.configPath)
    expect(hosts.map((h) => h.alias)).toEqual(['test'])
    expect(hosts[0]).toMatchObject({ hostName: '127.0.0.1', user: 'tester', port: server.port })
    expect(await listSshHosts(env.configPath + '.none')).toEqual([])
  })
})
