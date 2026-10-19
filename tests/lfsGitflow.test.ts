// Git LFS · Gitflow 표시: 포인터 파서·속성 감지·gitflow 설정 파싱·기본 접두사 감지와 악성 설정값 처리.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { classifyBranch, GITFLOW_VALUE_MAX, parseGitflowConfig, resolveGitflow } from '../src/shared/gitflow'
import { gitattributesUsesLfs, parseCheckAttrLfs, parseLfsPointer, pointerFromLines } from '../src/shared/lfs'
import { lfsFromDiff } from '../src/renderer/src/diff/lfsDiff'
import { formatBytes } from '../src/renderer/src/lib/format'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { lfsPaths, repoExtras } from '../src/main/git/repoExtras'
import type { FileDiff } from '../src/shared/types'

const OID = 'a'.repeat(64)
const OID2 = 'b'.repeat(64)
const pointer = (oid: string, size: number): string => `version https://git-lfs.github.com/spec/v1\noid sha256:${oid}\nsize ${size}\n`

describe('parseLfsPointer', () => {
  it('정상 포인터를 읽는다', () => {
    expect(parseLfsPointer(pointer(OID, 12345))).toEqual({ algo: 'sha256', oid: OID, size: 12345 })
  })
  it('CRLF·끝 개행 없음도 읽는다', () => {
    expect(parseLfsPointer(pointer(OID, 7).replace(/\n/g, '\r\n').trimEnd())).toMatchObject({ size: 7 })
  })
  it('확장 키(ext-*)가 있어도 필수 키만 본다', () => {
    expect(parseLfsPointer(`version https://git-lfs.github.com/spec/v1\next-0-foo sha256:${OID2}\noid sha256:${OID}\nsize 3\n`)).toMatchObject({ oid: OID })
  })
  it('형식이 어긋나면 null', () => {
    expect(parseLfsPointer('hello')).toBeNull()
    expect(parseLfsPointer('')).toBeNull()
    expect(parseLfsPointer(`version https://example.com/v2\noid sha256:${OID}\nsize 1\n`)).toBeNull()
    expect(parseLfsPointer(`version https://git-lfs.github.com/spec/v1\noid sha256:${OID}\n`)).toBeNull()
    expect(parseLfsPointer(`version https://git-lfs.github.com/spec/v1\nsize 1\n`)).toBeNull()
    expect(parseLfsPointer(pointer('xyz', 1))).toBeNull()
    expect(parseLfsPointer(pointer(OID.toUpperCase(), 1))).toBeNull()
    expect(parseLfsPointer(`version https://git-lfs.github.com/spec/v1\noid md5:${OID}\nsize 1\n`)).toBeNull()
  })
  it('크기가 정수가 아니거나 비정상이면 null', () => {
    for (const bad of ['-1', '1.5', '1e3', 'abc', '', '9'.repeat(40)]) {
      expect(parseLfsPointer(`version https://git-lfs.github.com/spec/v1\noid sha256:${OID}\nsize ${bad}\n`)).toBeNull()
    }
  })
  it('중복 키·너무 긴 텍스트·키 없는 줄은 null', () => {
    expect(parseLfsPointer(pointer(OID, 1) + `oid sha256:${OID2}\n`)).toBeNull()
    expect(parseLfsPointer(pointer(OID, 1) + 'x'.repeat(2000))).toBeNull()
    expect(parseLfsPointer(pointer(OID, 1) + 'nospace\n')).toBeNull()
  })
  it('pointerFromLines 는 줄 수가 터무니없으면 보지 않는다', () => {
    expect(pointerFromLines(pointer(OID, 5).trimEnd().split('\n'))).toMatchObject({ size: 5 })
    expect(pointerFromLines(['a', 'b'])).toBeNull()
    expect(pointerFromLines(Array(100).fill('x'))).toBeNull()
  })
})

describe('속성 판단', () => {
  it('check-attr -z 출력에서 filter=lfs 경로만 모은다(개행·공백 경로 포함)', () => {
    const out = ['a.bin', 'filter', 'lfs', 'b.txt', 'filter', 'unspecified', 'we ird\nname.psd', 'filter', 'lfs', 'c', 'filter', 'other', ''].join('\0')
    expect(parseCheckAttrLfs(out)).toEqual(['a.bin', 'we ird\nname.psd'])
    expect(parseCheckAttrLfs('')).toEqual([])
    expect(parseCheckAttrLfs('a\0filter')).toEqual([])
  })
  it('.gitattributes 에서 filter=lfs 규칙을 찾는다', () => {
    expect(gitattributesUsesLfs('*.psd filter=lfs diff=lfs merge=lfs -text\n')).toBe(true)
    expect(gitattributesUsesLfs('*.txt text\r\n*.bin filter=lfs\r\n')).toBe(true)
    expect(gitattributesUsesLfs('# *.psd filter=lfs\n*.txt text\n')).toBe(false)
    expect(gitattributesUsesLfs('*.psd filter=lfsx\n*.a myfilter=lfs\n')).toBe(false)
    expect(gitattributesUsesLfs('')).toBe(false)
  })
})

const diffOf = (lines: { t: ' ' | '+' | '-'; text: string }[], extra: Partial<FileDiff> = {}): FileDiff => ({
  path: 'a.bin',
  binary: false,
  hunks: [{ oldStart: 1, oldLines: 3, newStart: 1, newLines: 3, header: '', lines }],
  truncated: false,
  totalLines: lines.length,
  meta: [],
  ...extra
})
const side = (t: '+' | '-', text: string) => text.trimEnd().split('\n').map((l) => ({ t, text: l }))

describe('lfsFromDiff', () => {
  it('수정된 포인터는 양쪽을 돌려준다', () => {
    const d = diffOf([
      { t: ' ', text: 'version https://git-lfs.github.com/spec/v1' },
      { t: '-', text: `oid sha256:${OID}` },
      { t: '-', text: 'size 100' },
      { t: '+', text: `oid sha256:${OID2}` },
      { t: '+', text: 'size 2097152' }
    ])
    const r = lfsFromDiff(d)
    expect(r?.before).toMatchObject({ oid: OID, size: 100 })
    expect(r?.after).toMatchObject({ oid: OID2, size: 2097152 })
  })
  it('새 파일·삭제된 파일', () => {
    expect(lfsFromDiff(diffOf(side('+', pointer(OID, 9))))).toMatchObject({ before: null, after: { size: 9 } })
    expect(lfsFromDiff(diffOf(side('-', pointer(OID, 9))))).toMatchObject({ before: { size: 9 }, after: null })
  })
  it('일반 텍스트·바이너리·잘린 diff 는 null', () => {
    expect(lfsFromDiff(diffOf([{ t: '+', text: 'hello' }, { t: '+', text: 'a' }, { t: '+', text: 'b' }]))).toBeNull()
    expect(lfsFromDiff(diffOf(side('+', pointer(OID, 1)), { binary: true }))).toBeNull()
    expect(lfsFromDiff(diffOf(side('+', pointer(OID, 1)), { truncated: true }))).toBeNull()
    expect(lfsFromDiff(null)).toBeNull()
  })
})

describe('formatBytes', () => {
  it('단위를 고른다', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
    expect(formatBytes(3 * 1024 ** 3)).toBe('3.0 GB')
    expect(formatBytes(-1)).toBe('?')
    expect(formatBytes(NaN)).toBe('?')
  })
})

const rec = (k: string, v: string): string => `${k}\n${v}\0`

describe('parseGitflowConfig', () => {
  it('branch·prefix 값을 읽는다', () => {
    const raw = rec('gitflow.branch.master', 'main') + rec('gitflow.branch.develop', 'dev') + rec('gitflow.prefix.feature', 'feat/') + rec('gitflow.prefix.versiontag', 'v')
    expect(parseGitflowConfig(raw)).toEqual({ master: 'main', develop: 'dev', prefixes: { feature: 'feat/' } })
  })
  it('설정이 없으면 null', () => {
    expect(parseGitflowConfig('')).toBeNull()
    expect(parseGitflowConfig(rec('gitflow.prefix.versiontag', 'v') + rec('gitflow.unknown', 'x'))).toBeNull()
  })
  it('같은 키는 마지막 값이 이긴다', () => {
    expect(parseGitflowConfig(rec('gitflow.branch.develop', 'a') + rec('gitflow.branch.develop', 'b'))?.develop).toBe('b')
  })
  it('줄바꿈·제어문자·너무 긴 값은 버린다', () => {
    const raw =
      rec('gitflow.branch.master', 'ma\nin') +
      rec('gitflow.branch.develop', 'de\u001b[31mv') +
      rec('gitflow.prefix.feature', 'x'.repeat(GITFLOW_VALUE_MAX + 1)) +
      rec('gitflow.prefix.release', 'rel\u0007/') +
      rec('gitflow.prefix.hotfix', '   ') +
      rec('gitflow.prefix.bugfix', 'fix/')
    expect(parseGitflowConfig(raw)).toEqual({ prefixes: { bugfix: 'fix/' } })
    expect(parseGitflowConfig(rec('gitflow.branch.master', 'm\u{2028}x'))).toBeNull()
    expect(parseGitflowConfig(rec('gitflow.branch.master', '$(touch pwned)'))).toEqual({ master: '$(touch pwned)', prefixes: {} })
  })
  it('깨진 레코드와 아주 큰 입력도 던지지 않는다', () => {
    expect(parseGitflowConfig('no-newline\0\n\0')).toBeNull()
    expect(parseGitflowConfig(rec('gitflow.branch.master', 'm').repeat(50_000))).toMatchObject({ master: 'm' })
  })
})

describe('resolveGitflow · classifyBranch', () => {
  it('설정이 있으면 빠진 접두사는 기본값으로 채운다', () => {
    const f = resolveGitflow({ develop: 'dev', prefixes: { feature: 'feat/' } }, [])!
    expect(f).toMatchObject({ source: 'config', develop: 'dev', master: null })
    expect(f.prefixes).toMatchObject({ feature: 'feat/', release: 'release/' })
    expect(classifyBranch('feat/x', f)).toBe('feature')
    expect(classifyBranch('feature/x', f)).toBeNull()
    expect(classifyBranch('dev', f)).toBe('develop')
    expect(classifyBranch('main', f)).toBe('master')
    expect(classifyBranch('master', f)).toBe('master')
  })
  it('master 가 설정되면 그 이름만 master 다', () => {
    const f = resolveGitflow({ master: 'prod', prefixes: {} }, [])!
    expect(classifyBranch('prod', f)).toBe('master')
    expect(classifyBranch('main', f)).toBeNull()
  })
  it('설정이 없으면 기본 접두사 브랜치가 있을 때만 감지한다', () => {
    expect(resolveGitflow(null, ['main', 'develop', 'topic/x'])).toBeNull()
    expect(resolveGitflow(null, ['main', 'feature/'])).toBeNull()
    const f = resolveGitflow(null, ['main', 'hotfix/1'])!
    expect(f.source).toBe('detected')
    expect(classifyBranch('hotfix/1', f)).toBe('hotfix')
    expect(classifyBranch('release/1.0', f)).toBe('release')
    expect(classifyBranch('bugfix/a', f)).toBe('bugfix')
    expect(classifyBranch('support/1.x', f)).toBe('support')
    expect(classifyBranch('feature', f)).toBeNull()
  })
  it('접두사가 겹치면 더 긴 쪽이 이긴다', () => {
    const f = resolveGitflow({ prefixes: { feature: 'f/', bugfix: 'f/bug/' } }, [])!
    expect(classifyBranch('f/bug/1', f)).toBe('bugfix')
    expect(classifyBranch('f/x', f)).toBe('feature')
  })
})

// ── 실제 git 으로: 필터 명령이 실행되지 않는지, LFS 감지가 동작하는지 ──
let root: string
let repo: string
let marker: string
let head: string
const exec = new LocalExecutor()

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-lfs-'))
  repo = join(root, 'repo')
  marker = join(root, 'PWNED')
  const g = (...a: string[]): string =>
    execFileSync('git', ['-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t', ...a], { encoding: 'utf8' })
  execFileSync('git', ['init', '-q', '-b', 'main', repo])
  writeFileSync(join(repo, '.gitattributes'), '*.psd filter=lfs diff=lfs merge=lfs -text\n')
  writeFileSync(join(repo, 'a.psd'), pointer(OID, 4096))
  writeFileSync(join(repo, 'b.txt'), 'plain\n')
  writeFileSync(join(repo, '-weird.psd'), pointer(OID2, 1))
  g('add', '-A')
  g('commit', '-q', '-m', 'lfs')
  head = g('rev-parse', 'HEAD').trim()
  // 페이로드: 실행되면 마커 파일을 만든다(저장소가 정한 필터 명령)
  const touch = `node -e "require('fs').writeFileSync(process.argv[1],'x')" "${marker.replace(/\\/g, '/')}"`
  for (const k of ['clean', 'smudge', 'process']) g('config', `filter.lfs.${k}`, touch)
  g('config', 'gitflow.branch.master', 'main')
  g('config', 'gitflow.prefix.feature', 'feat/')
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('repoExtras · lfsPaths (실제 git)', () => {
  it('LFS 사용과 gitflow 설정을 읽는다', async () => {
    const x = await repoExtras(exec, repo)
    expect(x.lfs).toBe(true)
    expect(x.gitflow).toEqual({ master: 'main', prefixes: { feature: 'feat/' } })
  })
  it('커밋 기준·작업 트리 기준 모두 filter=lfs 경로만 돌려주고 필터 명령은 실행하지 않는다', async () => {
    const paths = ['a.psd', 'b.txt', '-weird.psd', 'missing.psd']
    expect((await lfsPaths(exec, repo, { rev: head, paths })).sort()).toEqual(['-weird.psd', 'a.psd', 'missing.psd'])
    expect((await lfsPaths(exec, repo, { paths })).sort()).toEqual(['-weird.psd', 'a.psd', 'missing.psd'])
    expect(existsSync(marker)).toBe(false)
  })
  it('rev 는 해시만, 경로는 NUL 이 없어야 한다', async () => {
    await expect(lfsPaths(exec, repo, { rev: '--help', paths: ['a'] })).rejects.toThrow()
    await expect(lfsPaths(exec, repo, { paths: ['a\0b'] })).rejects.toThrow()
    await expect(lfsPaths(exec, repo, { paths: [''] })).rejects.toThrow()
    expect(await lfsPaths(exec, repo, { paths: [] })).toEqual([])
  })
  it('경로가 아주 많아도 묶어서 처리한다', async () => {
    const many = Array.from({ length: 1500 }, (_, i) => `dir/file-${i}-${'x'.repeat(30)}.psd`)
    const r = await lfsPaths(exec, repo, { rev: head, paths: many })
    expect(r.length).toBe(1000)
  })
})
