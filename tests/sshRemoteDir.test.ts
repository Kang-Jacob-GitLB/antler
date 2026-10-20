// 원격 폴더 탐색(SFTP) 테스트 — 127.0.0.1 의 프로세스 내 ssh2 Server 와 임시 폴더만 쓴다.
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { SshExecutor } from '../src/main/executors/SshExecutor'
import { scanFromListing } from '../src/main/services/repoScan'
import { knownHostsLine, makeHome, newKeyPair, startServer, type SshEnv, type TestServer } from './sshHarness'

const clientKey = newKeyPair()
let server: TestServer
let env: SshEnv
let root: string

function executor(): SshExecutor {
  return new SshExecutor('test', { configPath: env.configPath, knownHostsPath: env.knownHostsPath, agent: null, homeDir: env.home })
}

beforeAll(async () => {
  // 가상 원격: /home/tester/{work/{app(.git), lib}, .hidden, notes.txt}
  root = mkdtempSync(join(tmpdir(), 'antler-sftp-'))
  const home = join(root, 'home', 'tester')
  mkdirSync(join(home, 'work', 'app', '.git'), { recursive: true })
  mkdirSync(join(home, 'work', 'lib'), { recursive: true })
  mkdirSync(join(home, '.hidden'), { recursive: true })
  writeFileSync(join(home, 'notes.txt'), 'x')
  server = await startServer({ authorized: [clientKey], sftpRoot: root })
  env = makeHome(clientKey)
  writeFileSync(
    env.configPath,
    `Host test\n  HostName 127.0.0.1\n  Port ${server.port}\n  User tester\n  IdentityFile ${env.keyPath}\n  IdentitiesOnly yes\n`
  )
  writeFileSync(env.knownHostsPath, knownHostsLine(server.port, server.hostKey))
})

afterAll(async () => {
  await server.close()
})

describe('scanFromListing (SSH 폴더 안 저장소 찾기)', () => {
  it('원격 폴더 바로 아래의 저장소를 절대 경로로 돌려주고, 폴더 자체가 저장소면 하위는 비운다', async () => {
    const ex = executor()
    try {
      expect(scanFromListing(await ex.listDirs('~/work'))).toEqual({
        path: '/home/tester/work',
        isRepo: false,
        repos: [{ name: 'app', path: '/home/tester/work/app' }],
        truncated: false
      })
      expect(scanFromListing(await ex.listDirs('~/work/app'))).toMatchObject({ isRepo: true, repos: [] })
    } finally {
      await ex.dispose()
    }
  })

  it('저장소 여부를 확인하지 못한 폴더가 있으면 truncated 로 알린다', () => {
    const r = scanFromListing({ path: '/w', parent: '/', isRepo: false, dirs: [{ name: 'a', isRepo: true }, { name: 'b' }], truncated: false })
    expect(r.repos).toEqual([{ name: 'a', path: '/w/a' }])
    expect(r.truncated).toBe(true)
  })
})

describe('SshExecutor.listDirs (SFTP)', () => {
  it('빈 경로·~ 는 홈을 열고, 숨김 폴더와 파일은 빼고 폴더만 돌려준다', async () => {
    const ex = executor()
    for (const p of ['', '~']) {
      const r = await ex.listDirs(p)
      expect(r.path).toBe('/home/tester')
      expect(r.parent).toBe('/home')
      expect(r.isRepo).toBe(false)
      expect(r.dirs.map((d) => d.name)).toEqual(['work'])
    }
    await ex.dispose()
  })

  it('하위 폴더의 git 저장소 여부를 표시하고 ~/ 경로를 홈 기준으로 푼다', async () => {
    const ex = executor()
    const r = await ex.listDirs('~/work')
    expect(r.path).toBe('/home/tester/work')
    expect(r.dirs).toEqual([
      { name: 'app', isRepo: true },
      { name: 'lib', isRepo: false }
    ])
    const app = await ex.listDirs('/home/tester/work/app')
    expect(app.isRepo).toBe(true)
    await ex.dispose()
  })

  it('루트의 parent 는 null', async () => {
    const ex = executor()
    const r = await ex.listDirs('/')
    expect(r.path).toBe('/')
    expect(r.parent).toBeNull()
    expect(r.dirs.map((d) => d.name)).toEqual(['home'])
    await ex.dispose()
  })

  it('없는 폴더는 오류', async () => {
    const ex = executor()
    await expect(ex.listDirs('/nope')).rejects.toThrow()
    await ex.dispose()
  })

  it('SFTP 를 막은 서버는 경로를 직접 입력하라고 안내한다', async () => {
    const plain = await startServer({ authorized: [clientKey] })
    const e2 = makeHome(clientKey)
    writeFileSync(e2.configPath, `Host test\n  HostName 127.0.0.1\n  Port ${plain.port}\n  User tester\n  IdentityFile ${e2.keyPath}\n`)
    writeFileSync(e2.knownHostsPath, knownHostsLine(plain.port, plain.hostKey))
    const ex = new SshExecutor('test', { configPath: e2.configPath, knownHostsPath: e2.knownHostsPath, agent: null, homeDir: e2.home })
    await expect(ex.listDirs('~')).rejects.toThrow(/SFTP/)
    await ex.dispose()
    await plain.close()
  })
})
