import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { CODE_TEXT_MAX_BYTES, CODE_TREE_MAX_ENTRIES } from '../src/main/git/codeBrowse'
import { allowedLinkUrl } from '../src/main/external/link'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RepoService } from '../src/main/services/RepoService'

const LFS_POINTER = `version https://git-lfs.github.com/spec/v1\noid sha256:${'a'.repeat(64)}\nsize 12345\n`
// 1x1 PNG 가 아니어도 된다 — 확장자로만 판정한다
const PNG_BYTES = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 3])

describe('코드 탐색기 (실제 저장소)', { timeout: 60_000 }, () => {
  let root: string
  let repo: string
  let head = ''
  const env = () => ({ ...process.env, GIT_AUTHOR_NAME: '홍길동', GIT_AUTHOR_EMAIL: 'a@x.io', GIT_COMMITTER_NAME: '홍길동', GIT_COMMITTER_EMAIL: 'a@x.io' })
  const git = (...args: string[]) => execFileSync('git', ['-c', 'core.autocrlf=false', ...args], { cwd: repo, env: env() }).toString('utf8').trim()
  const put = (rel: string, data: string | Buffer) => {
    const full = join(repo, rel)
    mkdirSync(join(full, '..'), { recursive: true })
    writeFileSync(full, data)
  }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-cb-'))
    repo = join(root, 'repo')
    mkdirSync(repo)
    git('init', '-q', '-b', 'main')
    put('README.md', '# hi\n')
    put('src/b.ts', 'export const b = 1\n')
    put('src/A.ts', 'export const a = 1\n')
    put('src/lib/deep.txt', 'deep\n')
    put('Zeta.txt', 'z\n')
    put('alpha.txt', 'a\n')
    put('-x', 'dash\n')
    put('latin1.txt', Buffer.from([0x61, 0xe9, 0x62, 0x0a]))
    put('bom.txt', Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from('hello\n')]))
    put('bin.dat', Buffer.from([1, 2, 0, 3]))
    put('pic.PNG', PNG_BYTES)
    put('vec.svg', '<svg xmlns="http://www.w3.org/2000/svg"/>\n')
    put('model.bin', LFS_POINTER)
    put('logo.png', LFS_POINTER)
    put('big.txt', Buffer.alloc(CODE_TEXT_MAX_BYTES + 1, 0x78))
    git('add', '-A')
    // 심볼릭 링크·서브모듈은 윈도우에서 체크아웃 없이 인덱스로 직접 만든다
    const target = execFileSyncStdin(['hash-object', '-w', '--stdin'], 'alpha.txt')
    git('update-index', '--add', '--cacheinfo', `120000,${target},link`)
    git('update-index', '--add', '--cacheinfo', `160000,${'c'.repeat(40)},sub`)
    git('commit', '-q', '-m', '초기')
    head = git('rev-parse', 'HEAD')
    git('tag', 'v1')
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  function execFileSyncStdin(args: string[], input: string): string {
    return execFileSync('git', args, { cwd: repo, env: env(), input }).toString('utf8').trim()
  }

  let svc: RepoService | null = null
  afterEach(async () => {
    await svc?.dispose()
    svc = null
  })
  function setup(): { svc: RepoService; repoId: string } {
    const config = new ConfigStore(join(mkdtempSync(join(root, 'cfg-')), 'antler.config.json'))
    const r = config.addRepo({ sourceId: 'local', path: repo, name: 'repo' })
    svc = new RepoService(config)
    return { svc, repoId: r.id }
  }

  describe('resolveRev', () => {
    it('HEAD·전체 ref 이름·해시는 커밋 해시로 푼다', async () => {
      const { svc, repoId } = setup()
      expect(await svc.resolveRev(repoId, 'HEAD')).toBe(head)
      expect(await svc.resolveRev(repoId, 'refs/heads/main')).toBe(head)
      expect(await svc.resolveRev(repoId, 'refs/tags/v1')).toBe(head)
      expect(await svc.resolveRev(repoId, head)).toBe(head)
      expect(await svc.resolveRev(repoId, head.slice(0, 8))).toBe(head)
    })

    it('옵션·단축 이름·리비전 식·없는 ref 는 거부한다', async () => {
      const { svc, repoId } = setup()
      for (const bad of ['--output=x', 'main', 'HEAD~1', 'HEAD^', 'refs/heads/*', 'refs/remotes/origin/*', '', 'refs/heads/main\0x']) {
        await expect(svc.resolveRev(repoId, bad)).rejects.toThrow()
      }
      await expect(svc.resolveRev(repoId, 'refs/heads/nope')).rejects.toThrow(/찾을 수 없습니다/)
      expect(existsSync(join(repo, 'x'))).toBe(false)
    })
  })

  describe('onRemote (줄 링크 push 여부)', () => {
    it('원격 추적 ref 에 든 커밋만 true, 로컬 전용 커밋은 false', async () => {
      const child = git('commit-tree', `${head}^{tree}`, '-p', head, '-m', 'local only')
      try {
        const { svc, repoId } = setup()
        expect(await svc.codeOnRemote(repoId, head)).toBe(false)
        git('update-ref', 'refs/remotes/origin/main', head)
        expect(await svc.codeOnRemote(repoId, head)).toBe(true)
        expect(await svc.codeOnRemote(repoId, head.toUpperCase())).toBe(true)
        // 원격 tip 의 자식(아직 push 안 한 커밋)은 false
        expect(await svc.codeOnRemote(repoId, child)).toBe(false)
        // 존재하지 않는 커밋도 false(오류 아님)
        expect(await svc.codeOnRemote(repoId, 'd'.repeat(40))).toBe(false)
      } finally {
        git('update-ref', '-d', 'refs/remotes/origin/main')
      }
    })

    it('짧은 해시·옵션 모양·비문자열은 거부한다', async () => {
      const { svc, repoId } = setup()
      for (const bad of [head.slice(0, 12), '--all', '--contains=x', `${head}x`, '', 'HEAD', 'refs/heads/main', 5 as unknown as string]) {
        await expect(svc.codeOnRemote(repoId, bad)).rejects.toThrow()
      }
    })
  })

  describe('트리', () => {
    it('최상위: 디렉터리 먼저, 이름순(대소문자 무시), 종류·모드를 구분한다', async () => {
      const { svc, repoId } = setup()
      const t = await svc.codeTree(repoId, { rev: head, dir: '' })
      expect(t).toMatchObject({ rev: head, dir: '', missing: false })
      expect(t.entries.map((e) => `${e.kind}:${e.name}`)).toEqual([
        'dir:src',
        'file:-x',
        'file:alpha.txt',
        'file:big.txt',
        'file:bin.dat',
        'file:bom.txt',
        'file:latin1.txt',
        'symlink:link',
        'file:logo.png',
        'file:model.bin',
        'file:pic.PNG',
        'file:README.md',
        'submodule:sub',
        'file:vec.svg',
        'file:Zeta.txt'
      ])
      const byName = Object.fromEntries(t.entries.map((e) => [e.name, e]))
      expect(byName['src']).toMatchObject({ kind: 'dir', mode: '040000', path: 'src' })
      expect(byName['link']).toMatchObject({ kind: 'symlink', mode: '120000' })
      expect(byName['sub']).toMatchObject({ kind: 'submodule', mode: '160000', oid: 'c'.repeat(40) })
      expect(byName['README.md']).toMatchObject({ kind: 'file', mode: '100644' })
    })

    it('하위 디렉터리: 경로를 이어 붙이고 앞뒤 / 를 정리한다', async () => {
      const { svc, repoId } = setup()
      const t = await svc.codeTree(repoId, { rev: head, dir: '/src/' })
      expect(t.dir).toBe('src')
      expect(t.entries.map((e) => [e.kind, e.name, e.path])).toEqual([
        ['dir', 'lib', 'src/lib'],
        ['file', 'A.ts', 'src/A.ts'],
        ['file', 'b.ts', 'src/b.ts']
      ])
      const deep = await svc.codeTree(repoId, { rev: head, dir: 'src/lib' })
      expect(deep.entries.map((e) => e.path)).toEqual(['src/lib/deep.txt'])
    })

    it('없는 경로·파일 경로·서브모듈 경로·없는 커밋은 missing 이다', async () => {
      const { svc, repoId } = setup()
      for (const dir of ['nope', 'README.md', 'sub', 'src/nope']) {
        expect(await svc.codeTree(repoId, { rev: head, dir })).toMatchObject({ missing: true, entries: [] })
      }
      expect(await svc.codeTree(repoId, { rev: 'f'.repeat(40), dir: '' })).toMatchObject({ missing: true, entries: [] })
    })

    it('rev·경로를 검증한다', async () => {
      const { svc, repoId } = setup()
      await expect(svc.codeTree(repoId, { rev: 'HEAD', dir: '' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: '--all', dir: '' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head.slice(0, 8), dir: '' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head, dir: '../x' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head, dir: 'src/../..' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head, dir: 'src/./lib' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head, dir: 'src\0' })).rejects.toThrow(/invalid/)
      await expect(svc.codeTree(repoId, { rev: head, dir: 5 as unknown as string })).rejects.toThrow(/invalid/)
    })

    it('항목이 5000개를 넘으면 정렬한 앞부분만 담고 truncated 다', async () => {
      const blob = execFileSyncStdin(['hash-object', '-w', '--stdin'], 'x\n')
      const lines = Array.from({ length: CODE_TREE_MAX_ENTRIES + 1 }, (_, i) => `100644 ${blob}\tmany/f${String(i).padStart(5, '0')}.txt\n`).join('')
      execFileSyncStdin(['update-index', '--add', '--index-info'], lines)
      const tree = execFileSyncStdin(['write-tree'], '')
      const commit = execFileSyncStdin(['commit-tree', tree, '-m', 'many'], '')
      const { svc, repoId } = setup()
      const t = await svc.codeTree(repoId, { rev: commit, dir: 'many' })
      expect(t.truncated).toBe(true)
      expect(t.entries).toHaveLength(CODE_TREE_MAX_ENTRIES)
      expect(t.entries[0]!.name).toBe('f00000.txt')
      expect(t.entries[CODE_TREE_MAX_ENTRIES - 1]!.name).toBe(`f${String(CODE_TREE_MAX_ENTRIES - 1).padStart(5, '0')}.txt`)
      const small = await svc.codeTree(repoId, { rev: commit, dir: 'src' })
      expect(small.truncated).toBeUndefined()
    })

    it('같은 요청은 캐시에서 돌려준다', async () => {
      const { svc, repoId } = setup()
      const a = await svc.codeTree(repoId, { rev: head, dir: 'src' })
      const b = await svc.codeTree(repoId, { rev: head.toUpperCase(), dir: 'src' })
      expect(b).toBe(a)
    })
  })

  describe('파일 내용', () => {
    const read = async (path: string) => {
      const { svc, repoId } = setup()
      return svc.codeBlob(repoId, { rev: head, path })
    }

    it('텍스트', async () => {
      const b = await read('src/b.ts')
      expect(b).toMatchObject({ kind: 'text', text: 'export const b = 1\n', path: 'src/b.ts', rev: head })
      expect(b.size).toBe(19)
      expect(b.invalidUtf8).toBeUndefined()
    })

    it('UTF-8 이 아니면 invalidUtf8 이고 깨진 바이트는 U+FFFD 다', async () => {
      const b = await read('latin1.txt')
      expect(b).toMatchObject({ kind: 'text', invalidUtf8: true, text: 'a�b\n' })
    })

    it('UTF-8 BOM 은 제거한다', async () => {
      const b = await read('bom.txt')
      expect(b).toMatchObject({ kind: 'text', text: 'hello\n' })
      expect(b.invalidUtf8).toBeUndefined()
      expect(b.size).toBe(9)
    })

    it('NUL 이 있으면 binary', async () => {
      expect(await read('bin.dat')).toMatchObject({ kind: 'binary', size: 4 })
    })

    it('이미지는 확장자(대소문자 무관)로 판정해 dataUrl 을 담는다', async () => {
      const png = await read('pic.PNG')
      expect(png).toMatchObject({ kind: 'image', size: PNG_BYTES.length, dataUrl: `data:image/png;base64,${PNG_BYTES.toString('base64')}` })
      const svg = await read('vec.svg')
      expect(svg.kind).toBe('image')
      expect(svg.dataUrl?.startsWith('data:image/svg+xml;base64,')).toBe(true)
    })

    it('LFS 포인터', async () => {
      expect(await read('model.bin')).toMatchObject({ kind: 'lfs', lfs: { oid: 'a'.repeat(64), size: 12345 } })
    })

    it('LFS 로 추적되는 이미지 확장자 파일도 포인터면 lfs 다', async () => {
      expect(await read('logo.png')).toMatchObject({ kind: 'lfs', lfs: { oid: 'a'.repeat(64), size: 12345 } })
    })

    it('상한(2MB)을 넘으면 내용을 읽지 않고 tooLarge', async () => {
      const b = await read('big.txt')
      expect(b).toMatchObject({ kind: 'tooLarge', size: CODE_TEXT_MAX_BYTES + 1 })
      expect(b.text).toBeUndefined()
    })

    it('디렉터리·서브모듈·없는 경로는 missing', async () => {
      for (const p of ['src', 'sub', 'nope.txt', 'src/nope.ts']) {
        expect(await read(p)).toMatchObject({ kind: 'missing', size: 0 })
      }
    })

    it('심볼릭 링크는 링크 대상 문자열을 텍스트로 돌려준다', async () => {
      expect(await read('link')).toMatchObject({ kind: 'text', text: 'alpha.txt' })
    })

    it('옵션처럼 보이는 파일명도 파일로 읽는다', async () => {
      expect(await read('-x')).toMatchObject({ kind: 'text', text: 'dash\n' })
    })

    it('rev·경로를 검증한다', async () => {
      const { svc, repoId } = setup()
      await expect(svc.codeBlob(repoId, { rev: 'HEAD', path: 'README.md' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: '--all', path: 'README.md' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head.slice(0, 8), path: 'README.md' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head, path: '' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head, path: '/' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head, path: '../README.md' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head, path: 'src/../README.md' })).rejects.toThrow(/invalid/)
      await expect(svc.codeBlob(repoId, { rev: head, path: 'README.md\0' })).rejects.toThrow(/invalid/)
    })

    it('요청을 취소할 수 있다', async () => {
      const { svc, repoId } = setup()
      const p = svc.codeBlob(repoId, { rev: head, path: 'big.txt' }, 'req-cb')
      svc.cancel('req-cb')
      await expect(p).rejects.toThrow()
    })
  })

  it('악성 저장소: textconv·filter 명령을 실행하지 않는다', async () => {
    const marker = join(root, 'cb-evil-ran').replace(/\\/g, '/')
    put('evil.txt', 'plain\n')
    git('add', 'evil.txt')
    git('commit', '-q', '-m', '악성')
    const evilHead = git('rev-parse', 'HEAD')
    writeFileSync(join(repo, '.git', 'info', 'attributes'), '*.txt diff=evil filter=evil\n')
    git('config', 'diff.evil.textconv', `sh -c "echo x > '${marker}'; cat" --`)
    git('config', 'filter.evil.smudge', `sh -c "echo x > '${marker}'; cat" --`)
    git('config', 'filter.evil.clean', `sh -c "echo x > '${marker}'; cat" --`)
    try {
      const { svc, repoId } = setup()
      const b = await svc.codeBlob(repoId, { rev: evilHead, path: 'evil.txt' })
      expect(b).toMatchObject({ kind: 'text', text: 'plain\n' })
      await svc.codeTree(repoId, { rev: evilHead, dir: '' })
      expect(existsSync(marker)).toBe(false)
    } finally {
      git('config', '--unset', 'diff.evil.textconv')
      git('config', '--unset', 'filter.evil.smudge')
      git('config', '--unset', 'filter.evil.clean')
      rmSync(join(repo, '.git', 'info', 'attributes'), { force: true })
    }
  })
})

describe('allowedLinkUrl', () => {
  it('자격 증명 없는 https 만 허용한다', () => {
    expect(allowedLinkUrl('https://example.com/a?b=1#c')).toBe('https://example.com/a?b=1#c')
    expect(allowedLinkUrl('http://example.com')).toBeNull()
    expect(allowedLinkUrl('file:///etc/passwd')).toBeNull()
    expect(allowedLinkUrl('javascript:alert(1)')).toBeNull()
    expect(allowedLinkUrl('https://user@example.com')).toBeNull()
    expect(allowedLinkUrl('https://user:pw@example.com')).toBeNull()
    expect(allowedLinkUrl('not a url')).toBeNull()
    expect(allowedLinkUrl(5)).toBeNull()
  })
})
