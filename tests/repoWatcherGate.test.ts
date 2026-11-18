import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { cachedProbe, WslExecutor } from '../src/main/executors/WslExecutor'
import { isWatchableGitDir, RepoWatcher } from '../src/main/services/RepoWatcher'

let root: string
let repo: string

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-watchgate-'))
  repo = join(root, 'repo')
  execFileSync('git', ['init', '-q', '-b', 'main', repo])
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('감시 게이트 — 링크·네트워크·NT 경로를 따라가지 않는다', () => {
  /** 심볼릭 링크를 못 만드는 환경(권한 없는 Windows)이면 null */
  const tryLink = (target: string, path: string, type: 'file' | 'dir' | 'junction'): boolean => {
    try {
      symlinkSync(target, path, type)
      return true
    } catch {
      return false
    }
  }

  it('실제 git 디렉터리만 감시한다 — HEAD 가 없거나 파일이 아니면 거부', () => {
    const gitDir = join(repo, '.git')
    expect(isWatchableGitDir(gitDir)).toBe(true)
    const empty = mkdtempSync(join(root, 'nohead-'))
    expect(isWatchableGitDir(empty)).toBe(false)
    mkdirSync(join(empty, 'HEAD'))
    expect(isWatchableGitDir(empty)).toBe(false)
    expect(isWatchableGitDir(join(root, 'none'))).toBe(false)
  })

  it('네트워크(UNC)·NT 네임스페이스 경로는 stat 하기 전에 거부한다', () => {
    for (const p of ['\\host\share\.git', '//host/share/.git', '\??\UNC\host\share\.git', '//??/UNC/host/share', 'C:\\x\\??\\y']) {
      expect(isWatchableGitDir(p), p).toBe(false)
    }
  })

  it('HEAD 가 심볼릭 링크거나 디렉터리 자체가 링크면 거부한다', () => {
    const real = mkdtempSync(join(root, 'real-'))
    writeFileSync(join(real, 'HEAD'), 'ref: refs/heads/main\n')
    const headLink = mkdtempSync(join(root, 'headlink-'))
    if (tryLink(join(real, 'HEAD'), join(headLink, 'HEAD'), 'file')) expect(isWatchableGitDir(headLink)).toBe(false)
    const dirLink = join(root, 'dirlink')
    if (tryLink(real, dirLink, 'junction')) expect(isWatchableGitDir(dirLink)).toBe(false)
  })
})

describe('WSL 배포판 등록 관문', () => {
  /** repoKind 가 'wsl' 인 RepoService 대역. 시그니처 호출 수를 센다 */
  const fakeWsl = (): { repos: never; sigs: () => number } => {
    let n = 0
    const repos = {
      afterFirstPage: () => Promise.resolve(),
      repoKind: () => 'wsl',
      signature: () => {
        n++
        return Promise.resolve('sig')
      }
    }
    return { repos: repos as never, sigs: () => n }
  }

  it('등록된 배포판이 없으면 기준 시그니처도 포커스 확인도 하지 않는다(wsl.exe 호출 0)', async () => {
    const { repos, sigs } = fakeWsl()
    let probes = 0
    const watcher = new RepoWatcher(repos, () => {}, () => true, async () => {
      probes++
      return false
    })
    await watcher.watch('r1')
    watcher.setFocused(true)
    await new Promise((r) => setTimeout(r, 50))
    expect(sigs()).toBe(0)
    expect(probes).toBeGreaterThan(0)
    watcher.dispose()
  })

  it('등록돼 있으면 시그니처를 계산한다', async () => {
    const { repos, sigs } = fakeWsl()
    const watcher = new RepoWatcher(repos, () => {}, () => true, async () => true)
    await watcher.watch('r1')
    expect(sigs()).toBe(1)
    watcher.setFocused(true)
    await new Promise((r) => setTimeout(r, 50))
    expect(sigs()).toBe(2)
    watcher.dispose()
  })

  it('확인 결과는 TTL 동안 기억하고 지나면 다시 본다', async () => {
    let t = 0
    let probes = 0
    const probe = cachedProbe(async () => ++probes === 1, 60_000, () => t)
    expect(await probe()).toBe(true)
    t = 59_000
    expect(await probe()).toBe(true)
    expect(probes).toBe(1)
    t = 61_000
    expect(await probe()).toBe(false)
    expect(probes).toBe(2)
  })

  it('WslExecutor 는 등록된 배포판이 없으면 wsl.exe 를 띄우지 않고 실패를 돌려준다', async () => {
    const exec = new WslExecutor('Ubuntu', async () => false)
    const r = await exec.run('/home/me/repo', ['status'])
    expect(r.code).toBe(0xffffffff)
    expect(r.stderr).toContain('등록된 배포판이 없습니다')
    const it = exec.stream('/home/me/repo', ['log'])[Symbol.asyncIterator]()
    await expect(it.next()).rejects.toThrow('등록된 배포판이 없습니다')
  })
})
