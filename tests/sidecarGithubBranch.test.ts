import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { handleGithubBranch } from '../src/sidecar/githubBranch'

// Rust 의 GitHub 원격 전용 저장소 fetch(Phase 6b PR 7)가 알아낸 기본 브랜치를 레코드에 기록해 달라는 `githubBranch` 메시지.
// 설정 소유 전환(PR 9) 전까지 레코드를 쓰는 쪽은 이 프로세스의 ConfigStore 라서, 사이드카가 모양·소스·이름을 다시 검사한 뒤에만 기록한다.

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-ghbranch-')), 'antler.config.json')
const github = { id: 'github', kind: 'github', label: 'GitHub' } as const

function setup(): { c: ConfigStore; g: string; local: string } {
  const c = new ConfigStore(tmp())
  const g = c.addRepo({ sourceId: 'github', path: 'C:\\cache\\octocat\\hello.git', name: 'hello', github: { owner: 'octocat', repo: 'hello' } }, github)
  const local = c.addRepo({ sourceId: 'local', path: 'C:\\work\\a', name: 'a' })
  return { c, g: g.id, local: local.id }
}

describe('사이드카 githubBranch 메시지', () => {
  it('github 소스의 저장소이고 쓸 수 있는 이름이면 기본 브랜치를 기록한다(같은 값이면 설정을 바꾸지 않는다)', () => {
    const { c, g } = setup()
    expect(handleGithubBranch(c, { repoId: g, branch: 'release/1.x' })).toBe(true)
    expect(c.repo(g)?.github?.defaultBranch).toBe('release/1.x')
    let changes = 0
    c.onChange = () => void changes++
    expect(handleGithubBranch(c, { repoId: g, branch: 'release/1.x' })).toBe(true)
    expect(changes).toBe(0)
  })

  it('없는 저장소·github 가 아닌 소스·문자열이 아닌 값·쓸 수 없는 브랜치 이름은 기록하지 않는다', () => {
    const { c, g, local } = setup()
    const before = JSON.stringify(c.get())
    const bad: { repoId?: unknown; branch?: unknown }[] = [
      { repoId: 'missing', branch: 'main' },
      { repoId: local, branch: 'main' },
      { repoId: 1, branch: 'main' },
      { repoId: g, branch: 2 },
      { repoId: g },
      { branch: 'main' },
      ...['-x', '/abs', 'a..b', 'a b', 'a\nb', 'x.lock', '', 'a//b', 'trailing/', 'dot.'].map((branch) => ({ repoId: g, branch }))
    ]
    for (const m of bad) expect(handleGithubBranch(c, m), JSON.stringify(m)).toBe(false)
    expect(JSON.stringify(c.get())).toBe(before)
  })
})
