// 악성 저장소 설정이 읽기 전용 명령 중에 코드를 실행하지 못하는지 확인한다(무해한 마커 파일로만 검사).
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { commitDetail, logArgs, refs, refsSignature } from '../src/main/git/commands'
import { wipStatus } from '../src/main/git/wip'

let root: string
let repo: string
let marker: string
const exec = new LocalExecutor()
const MISSING = 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef'

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-hostile-'))
  repo = join(root, 'repo')
  marker = join(root, 'PWNED')
  const g = (...a: string[]) => execFileSync('git', ['-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t', ...a], { stdio: 'ignore' })
  execFileSync('git', ['init', '-q', '-b', 'main', repo])
  g('commit', '-q', '--allow-empty', '-m', 'ok')
  // 페이로드: 실행되면 마커 파일을 만든다(git 이 셸로 실행하는 설정 값)
  const touch = `node -e "require('fs').writeFileSync(process.argv[1],'x')" "${marker.replace(/\\/g, '/')}"`
  writeFileSync(join(root, 'payload.sh'), `#!/bin/sh\n${touch}\nexit 1\n`)
  // partial clone(promisor) 저장소처럼 꾸미고, 없는 객체를 가리키는 ref 를 심는다 → 조회 시 lazy fetch 유도
  g('config', 'core.repositoryformatversion', '1')
  g('config', 'extensions.partialClone', 'origin')
  g('config', 'remote.origin.promisor', 'true')
  g('config', 'remote.origin.url', 'ssh://example.invalid/x')
  g('config', 'core.sshCommand', touch)
  g('config', 'remote.origin.uploadpack', touch)
  g('config', 'core.fsmonitor', touch)
  // update-ref 는 없는 객체를 거부하므로 loose ref 파일을 직접 쓴다
  writeFileSync(join(repo, '.git', 'refs', 'heads', 'evil'), `${MISSING}\n`)
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('악성 저장소 설정 — 방어 하나씩', () => {
  // 옛 git(WSL 2.43 등)은 GIT_NO_LAZY_FETCH 를 모르므로 GIT_ALLOW_PROTOCOL 만으로도 막혀야 한다
  const probe = (extraEnv: Record<string, string>): boolean => {
    rmSync(marker, { force: true })
    try {
      execFileSync('git', ['-C', repo, 'for-each-ref', '--format=%(objecttype)'], {
        env: { ...process.env, GIT_TERMINAL_PROMPT: '0', ...extraEnv },
        stdio: 'ignore'
      })
    } catch {
      /* 실패해도 된다 — 마커가 생겼는지만 본다 */
    }
    return existsSync(marker)
  }
  it('방어가 없으면 실행된다(재현 확인)', () => expect(probe({})).toBe(true))
  it('GIT_ALLOW_PROTOCOL=none 만으로 막힌다', () => expect(probe({ GIT_ALLOW_PROTOCOL: 'none' })).toBe(false))
  it('GIT_NO_LAZY_FETCH=1 만으로 막힌다(git 2.45.1+)', () => expect(probe({ GIT_NO_LAZY_FETCH: '1' })).toBe(false))
})

describe('악성 저장소 설정', () => {
  it('ref 조회·시그니처·log·상세 중 어떤 설정 명령도 실행되지 않는다', async () => {
    await refs(exec, repo).catch(() => {})
    await refsSignature(exec, repo).catch(() => {})
    await exec.run(repo, logArgs(undefined, 0)).catch(() => {})
    await commitDetail(exec, repo, MISSING).catch(() => {})
    await exec.run(repo, ['cat-file', '-t', MISSING]).catch(() => {})
    // 작업 트리 조회(WIP)도 같은 설정을 읽는다
    await wipStatus(exec, repo).catch(() => {})
    expect(existsSync(marker)).toBe(false)
    // git 프로세스를 여러 번 순차로 띄우므로 전체 테스트가 동시에 돌 때는 기본 5초를 넘길 수 있다
  }, 30_000)
})
