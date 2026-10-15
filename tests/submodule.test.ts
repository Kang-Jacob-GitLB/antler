import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { refs, submodules } from '../src/main/git/commands'
import { parseGitlinks, parseGitmodules, parseProbe, submoduleStatus } from '../src/main/git/parsers/submodule'

const SHA_A = 'a'.repeat(40)
const SHA_B = 'b'.repeat(40)

describe('parseGitlinks', () => {
  it('gitlink(160000)만 뽑고 한글·공백 경로를 보존한다', () => {
    const buf = [
      `100644 ${SHA_B} 0\tREADME.md`,
      `160000 ${SHA_A} 0\tlibs/한글 모듈`,
      `160000 ${SHA_B} 0\tvendor/x`,
      `100755 ${SHA_B} 0\tbuild.sh`
    ].join('\0')
    expect(parseGitlinks(buf)).toEqual([
      { path: 'libs/한글 모듈', sha: SHA_A },
      { path: 'vendor/x', sha: SHA_B }
    ])
  })
  it('충돌 단계(1·2·3)는 경로당 하나만 남긴다', () => {
    const buf = [`160000 ${SHA_A} 1\tm`, `160000 ${SHA_B} 2\tm`, `160000 ${SHA_B} 3\tm`].join('\0')
    expect(parseGitlinks(buf)).toEqual([{ path: 'm', sha: SHA_A }])
  })
  it('빈 출력이면 빈 배열', () => expect(parseGitlinks('')).toEqual([]))
})

describe('parseGitmodules', () => {
  it('이름에 점·한글이 있어도 path/url/branch 를 경로에 대응시킨다', () => {
    const buf = [
      'submodule.libs/foo.bar.path\nlibs/foo.bar',
      'submodule.libs/foo.bar.url\nhttps://example.com/a.git',
      'submodule.libs/foo.bar.branch\ndevelop',
      'submodule.한글 이름.path\nlibs/한글 모듈',
      'submodule.한글 이름.url\n../rel/x.git',
      'submodule.nopath.url\nhttps://example.com/n.git',
      'submodule.active\n'
    ].join('\0')
    const m = parseGitmodules(buf)
    expect(m.get('libs/foo.bar')).toEqual({ name: 'libs/foo.bar', url: 'https://example.com/a.git', branch: 'develop' })
    expect(m.get('libs/한글 모듈')).toEqual({ name: '한글 이름', url: '../rel/x.git' })
    expect(m.size).toBe(2)
  })
})

describe('parseProbe / submoduleStatus', () => {
  it('git-dir 과 HEAD 를 읽는다', () => {
    expect(parseProbe(`/r/.git/modules/x\n${SHA_A}\n`)).toEqual({ gitDir: '/r/.git/modules/x', head: SHA_A })
    // HEAD 가 없는 저장소는 rev-parse 가 "HEAD" 를 그대로 찍는다
    expect(parseProbe('/r/.git\nHEAD\n')).toEqual({ gitDir: '/r/.git' })
  })
  it('상태 판정', () => {
    expect(submoduleStatus(SHA_A, SHA_A)).toBe('clean')
    expect(submoduleStatus(SHA_A, SHA_B)).toBe('modified')
    expect(submoduleStatus(SHA_A, undefined)).toBe('unknown')
  })
})

// ── 실제 저장소 ─────────────────────────────────────────

const exec = new LocalExecutor()
let root: string
let parent: string
let clone: string
let marker: string
const ident = ['-c', 'user.name=t', '-c', 'user.email=t@t']
const git = (cwd: string, ...a: string[]): string =>
  execFileSync('git', ['-C', cwd, ...ident, '-c', 'protocol.file.allow=always', ...a], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-sub-'))
  marker = join(root, 'PWNED')
  const lib = join(root, 'lib')
  execFileSync('git', ['init', '-q', '-b', 'main', lib])
  git(lib, 'commit', '-q', '--allow-empty', '-m', 'lib 1')
  parent = join(root, 'parent')
  execFileSync('git', ['init', '-q', '-b', 'main', parent])
  git(parent, 'commit', '-q', '--allow-empty', '-m', 'init')
  // 픽스처 구성에만 git submodule 을 쓴다(앱 코드는 쓰지 않는다)
  git(parent, 'submodule', 'add', '-q', lib, 'libs/한글 모듈')
  git(parent, 'submodule', 'add', '-q', '-b', 'main', lib, 'vendor/lib')
  git(parent, 'commit', '-q', '-m', 'add submodules')
  // 두 번째 clone — --recurse 없이 만들어 서브모듈이 비어 있다
  clone = join(root, 'clone')
  execFileSync('git', ['clone', '-q', parent, clone])
}, 60_000)
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('submodules (실제 저장소)', () => {
  it('초기화된 서브모듈은 up-to-date, URL·브랜치를 .gitmodules 에서 읽는다', async () => {
    const subs = await submodules(exec, parent)
    expect(subs.map((s) => s.path)).toEqual(['libs/한글 모듈', 'vendor/lib'])
    expect(subs.every((s) => s.status === 'clean' && s.head === s.recorded)).toBe(true)
    expect(subs[1]).toMatchObject({ name: 'vendor/lib', branch: 'main' })
    expect(subs[0]!.url).toContain('lib')
  })

  it('서브모듈 안에서 커밋하면 modified', async () => {
    const sub = join(parent, 'vendor', 'lib')
    git(sub, 'commit', '-q', '--allow-empty', '-m', 'lib 2')
    const subs = await submodules(exec, parent)
    const m = subs.find((s) => s.path === 'vendor/lib')!
    expect(m.status).toBe('modified')
    expect(m.head).toBe(git(sub, 'rev-parse', 'HEAD').trim())
    expect(m.head).not.toBe(m.recorded)
    expect(subs.find((s) => s.path !== 'vendor/lib')!.status).toBe('clean')
  })

  it('--recurse 없이 clone 한 저장소는 uninitialized (상위 저장소로 올라가 오판하지 않는다)', async () => {
    const subs = await submodules(exec, clone)
    expect(subs).toHaveLength(2)
    expect(subs.every((s) => s.status === 'uninitialized' && !s.head)).toBe(true)
  })

  it('refs() 스냅샷에 포함되고, 서브모듈이 없으면 빈 배열', async () => {
    expect((await refs(exec, parent)).submodules).toHaveLength(2)
    expect((await refs(exec, join(root, 'lib'))).submodules).toEqual([])
  })
})

describe('악성 서브모듈', () => {
  it('서브모듈의 .git/config 에 심은 fsmonitor·filter·hook·sshCommand 가 상태 조회 중 실행되지 않는다', async () => {
    const sub = join(parent, 'libs', '한글 모듈')
    const touch = `node -e "require('fs').writeFileSync(process.argv[1],'x')" "${marker.replace(/\\/g, '/')}"`
    const gitDir = git(sub, 'rev-parse', '--absolute-git-dir').trim()
    for (const [k, v] of [
      ['core.fsmonitor', touch],
      ['core.sshCommand', touch],
      ['core.pager', touch],
      ['core.editor', touch],
      ['filter.x.clean', touch],
      ['filter.x.smudge', touch],
      ['remote.origin.uploadpack', touch]
    ] as const) {
      git(sub, 'config', k, v)
    }
    // 훅(실행 가능 표시는 Windows 에서 의미 없지만 POSIX 에서는 필요하다)
    mkdirSync(join(gitDir, 'hooks'), { recursive: true })
    for (const h of ['post-checkout', 'post-commit', 'reference-transaction', 'pre-command']) {
      writeFileSync(join(gitDir, 'hooks', h), `#!/bin/sh\n${touch}\n`, { mode: 0o755 })
    }
    // 서브모듈 쪽 .gitattributes 로 필터 유도
    writeFileSync(join(sub, '.gitattributes'), '* filter=x\n')
    // 대조군: 하드닝 없이 index 를 읽는 명령을 돌리면 fsmonitor 페이로드가 실제로 실행된다(테스트가 의미 있음을 확인)
    rmSync(marker, { force: true })
    try {
      execFileSync('git', ['-C', sub, 'status', '--porcelain'], { stdio: 'ignore' })
    } catch {
      /* 마커만 본다 */
    }
    expect(existsSync(marker)).toBe(true)
    rmSync(marker, { force: true })

    const subs = await submodules(exec, parent)
    await refs(exec, parent)
    expect(subs.find((s) => s.path === 'libs/한글 모듈')?.status).toBe('clean')
    expect(existsSync(marker)).toBe(false)
  }, 30_000)
})
