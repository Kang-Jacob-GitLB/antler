// 아바타 404 대체 조회: 작성자의 push 된(원격 추적 ref 에서 닿는) 커밋만 고른다(실제 git, 임시 저장소)
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

let root: string
let work: string
let pushed: string
let localOnly: string
let svc: RepoService
let repoId: string

const git = (cwd: string, ...a: string[]): string => execFileSync('git', ['-C', cwd, '-c', 'commit.gpgsign=false', ...a], { encoding: 'utf8' }).trim()
// 작성자는 환경 변수로 준다 — CI 가 GIT_AUTHOR_* 를 설정해 두면 -c user.email 보다 그쪽이 이긴다
const commitAs = (name: string, email: string, msg: string): string => {
  execFileSync('git', ['-C', work, 'commit', '-q', '--allow-empty', '-m', msg], {
    env: { ...process.env, GIT_AUTHOR_NAME: name, GIT_AUTHOR_EMAIL: email, GIT_COMMITTER_NAME: name, GIT_COMMITTER_EMAIL: email }
  })
  return git(work, 'rev-parse', 'HEAD')
}

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-pushed-'))
  const bare = join(root, 'remote.git')
  work = join(root, 'work')
  execFileSync('git', ['init', '-q', '--bare', '-b', 'main', bare])
  execFileSync('git', ['init', '-q', '-b', 'main', work])
  git(work, 'remote', 'add', 'origin', bare)
  // 정규식 특수문자(+, .)가 든 이메일 — 고정 문자열로 맞춰야 한다
  pushed = commitAs('Dev', 'dev+x@corp.io', 'pushed work')
  commitAs('Other', 'devxx@corp.io', 'look-alike author') // dev+x 를 정규식으로 쓰면 'devxx' 도 걸린다
  git(work, 'push', '-q', 'origin', 'main')
  git(work, 'switch', '-q', '-c', 'feature')
  localOnly = commitAs('Dev', 'dev+x@corp.io', 'local only')
  const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
  repoId = config.addRepo({ sourceId: 'local', path: work, name: 'work' }).id
  svc = new RepoService(config)
}, 60_000)

afterAll(async () => {
  await svc.dispose()
  rmSync(root, { recursive: true, force: true })
})

describe('RepoService.pushedShasByAuthor', { timeout: 30_000 }, () => {
  it('push 된 커밋만 돌려주고 로컬 전용 커밋은 뺀다', async () => {
    const shas = await svc.pushedShasByAuthor(repoId, 'dev+x@corp.io', 3)
    expect(shas).toEqual([pushed])
    expect(shas).not.toContain(localOnly)
  })

  it('이메일은 대소문자 무시·고정 문자열로 맞춘다(비슷한 이메일은 걸리지 않는다)', async () => {
    expect(await svc.pushedShasByAuthor(repoId, 'DEV+X@CORP.IO', 3)).toEqual([pushed])
    expect(await svc.pushedShasByAuthor(repoId, 'nobody@corp.io', 3)).toEqual([])
  })
})
