import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { remotes } from '../src/main/git/commands'
import { parseGithubRemote, pickGithubRemote } from '../src/main/github/remoteUrl'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

describe('parseGithubRemote', () => {
  const ok = { owner: 'octo', repo: 'hello' }
  it.each([
    'https://github.com/octo/hello',
    'https://github.com/octo/hello.git',
    'https://github.com/octo/hello/',
    'https://github.com/octo/hello.git/',
    'http://github.com/octo/hello.git',
    'https://user:pass@github.com/octo/hello.git',
    'https://ghp_token@github.com/octo/hello',
    'https://GitHub.COM/octo/hello',
    'https://www.github.com/octo/hello',
    'git@github.com:octo/hello.git',
    'git@github.com:octo/hello',
    'git@github.com:/octo/hello.git',
    'github.com:octo/hello.git',
    'ssh://git@github.com/octo/hello.git',
    'ssh://git@github.com:22/octo/hello.git',
    'git://github.com/octo/hello',
    '  https://github.com/octo/hello.git  '
  ])('%s', (url) => {
    expect(parseGithubRemote(url)).toEqual(ok)
  })

  it('점이 든 저장소 이름은 .git 만 벗긴다', () => {
    expect(parseGithubRemote('https://github.com/octo/hello.js.git')).toEqual({ owner: 'octo', repo: 'hello.js' })
    expect(parseGithubRemote('https://github.com/octo/.github')).toEqual({ owner: 'octo', repo: '.github' })
  })

  it.each([
    '',
    'https://gitlab.com/octo/hello.git',
    'https://github.com.evil.com/octo/hello',
    'https://evil.com/github.com/octo/hello',
    'https://github.com@evil.com/octo/hello',
    'git@gitlab.com:octo/hello.git',
    'git@ghe.example.com:octo/hello.git',
    'https://github.com/octo',
    'https://github.com/',
    'https://github.com/octo/hello/tree/main',
    'https://github.com//octo/hello',
    'git@github.com:octo/hello/extra.git',
    'https://github.com:8443/octo/hello',
    'ssh://git@github.com:2222/octo/hello.git',
    'ftp://github.com/octo/hello',
    'file:///home/me/github.com/octo/hello',
    '/home/me/repo',
    'C:\\repos\\hello',
    'https://github.com/octo/he llo',
    'https://github.com/octo/..'
  ])('거부: %s', (url) => {
    expect(parseGithubRemote(url)).toBeNull()
  })
})

describe('pickGithubRemote', () => {
  const r = (name: string, url: string) => ({ name, url })
  it('origin → upstream → 첫 GitHub remote 순', () => {
    expect(pickGithubRemote([r('fork', 'git@github.com:a/b.git'), r('upstream', 'git@github.com:c/d.git'), r('origin', 'git@github.com:e/f.git')])).toEqual({ owner: 'e', repo: 'f', remote: 'origin' })
    expect(pickGithubRemote([r('fork', 'git@github.com:a/b.git'), r('upstream', 'git@github.com:c/d.git')])).toEqual({ owner: 'c', repo: 'd', remote: 'upstream' })
    expect(pickGithubRemote([r('x', 'git@gitlab.com:a/b.git'), r('fork', 'git@github.com:a/b.git')])).toEqual({ owner: 'a', repo: 'b', remote: 'fork' })
  })
  it('origin 이 GitHub 가 아니면 건너뛴다', () => {
    expect(pickGithubRemote([r('origin', 'git@gitlab.com:a/b.git'), r('upstream', 'git@github.com:c/d.git')])).toEqual({ owner: 'c', repo: 'd', remote: 'upstream' })
  })
  it('없으면 null', () => {
    expect(pickGithubRemote([])).toBeNull()
    expect(pickGithubRemote([r('origin', 'git@gitlab.com:a/b.git')])).toBeNull()
  })
})

describe('git remotes()', () => {
  let root: string
  let repo: string
  const env = () => ({ ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: join(root, '.no-global') })
  const git = (...args: string[]) => execFileSync('git', args, { cwd: repo, env: env() }).toString('utf8').trim()
  const exec = new LocalExecutor()

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-gh-'))
    repo = join(root, 'repo')
    mkdirSync(repo)
    git('init', '-q')
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('remote 가 없으면 빈 목록', async () => {
    expect(await remotes(exec, repo)).toEqual([])
  })

  it('점이 든 이름도 마지막 .url 기준으로 해석한다', async () => {
    git('remote', 'add', 'origin', 'https://github.com/octo/hello.git')
    git('remote', 'add', 'my.fork', 'git@github.com:me/hello.git')
    git('remote', 'add', 'a.url', 'https://example.com/x/y.git')
    const list = await remotes(exec, repo)
    expect(list).toHaveLength(3)
    expect(list).toEqual(
      expect.arrayContaining([
        { name: 'origin', url: 'https://github.com/octo/hello.git' },
        { name: 'my.fork', url: 'git@github.com:me/hello.git' },
        { name: 'a.url', url: 'https://example.com/x/y.git' }
      ])
    )
  })

  it('RepoService.ghRepo 가 origin 을 고른다', async () => {
    const config = new ConfigStore(join(root, 'antler.config.json'))
    const added = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    const svc = new RepoService(config)
    try {
      expect(await svc.ghRepo(added.id)).toEqual({ owner: 'octo', repo: 'hello', remote: 'origin' })
    } finally {
      await svc.dispose()
    }
  })
})
