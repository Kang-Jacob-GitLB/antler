import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, rmdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { assertInsideRoot, assertNoLinkEscape, isRemoteLinkTarget, linksStayLocal, MAX_LINK_HOPS } from '../src/main/git/linkGuard'
import { scanLocalRepos } from '../src/main/services/repoScan'

// 원격 UNC 를 가리키는 링크는 만들지 않는다 — 판정은 순수 함수로, 거부 경로는 판정 함수 주입으로 시험한다.

const CASES = JSON.parse(readFileSync(new URL('./fixtures/repos/link-target.cases.json', import.meta.url), 'utf8')) as { remote: string[]; local: string[] }

describe('isRemoteLinkTarget (Rust 와 공유하는 사례표)', () => {
  it.each(CASES.remote)('원격: %j', (t) => expect(isRemoteLinkTarget(t)).toBe(true))
  it.each(CASES.local)('로컬: %j', (t) => expect(isRemoteLinkTarget(t)).toBe(false))
})

const win = process.platform === 'win32'
/** 정션은 관리자 권한이 필요 없다 */
const junction = (link: string, target: string): void => {
  execFileSync('cmd', ['/c', 'mklink', '/J', link, target], { stdio: 'ignore', windowsHide: true })
}

describe.skipIf(!win)('linksStayLocal (정션)', () => {
  let base: string
  const made: string[] = []
  const mk = (name: string, target: string): string => {
    const p = join(base, name)
    junction(p, target)
    made.push(p)
    return p
  }
  /** prefix1 → prefix2 → … → prefix<n> → real 사슬을 만들고 첫 링크를 돌려준다 */
  const chain = (prefix: string, n: number, real: string): string => {
    let target = real
    for (let i = n; i >= 1; i--) target = mk(`${prefix}${i}`, target)
    return target
  }

  beforeAll(() => {
    base = mkdtempSync(join(tmpdir(), 'antler-linkguard-'))
    mkdirSync(join(base, 'real', 'sub'), { recursive: true })
    writeFileSync(join(base, 'real', 'sub', 'f.txt'), 'x')
  })
  afterAll(() => {
    // 정션부터 지운다(rmdir 은 링크만 지운다 — 대상 내용은 건드리지 않는다)
    for (const p of [...made].reverse()) {
      try {
        rmdirSync(p)
      } catch {
        // 이미 없다
      }
    }
    // 링크가 하나라도 남았으면 재귀 삭제가 대상까지 지우지 않도록 통째 삭제를 하지 않는다
    if (base && !made.some((p) => existsSync(p))) rmSync(base, { recursive: true, force: true })
  })

  it('링크가 없는 경로·없는 경로는 통과', () => {
    expect(linksStayLocal(join(base, 'real', 'sub', 'f.txt'))).toBe(true)
    expect(linksStayLocal(join(base, 'nope', 'x'))).toBe(true)
  })

  it('로컬을 가리키는 정션은 통과하고 그 아래 경로도 통과', () => {
    const j = mk('local1', join(base, 'real'))
    expect(linksStayLocal(j)).toBe(true)
    expect(linksStayLocal(join(j, 'sub', 'f.txt'))).toBe(true)
  })

  it('사슬 길이 16 은 통과, 17 은 거부', () => {
    expect(MAX_LINK_HOPS).toBe(16)
    // 정션 생성이 느리다(프로세스 기동) — 17 개 사슬 하나로 둘 다 본다: c1 은 17 단계, c2 는 16 단계
    const first = chain('c', 17, join(base, 'real'))
    expect(linksStayLocal(join(base, 'c2'))).toBe(true)
    expect(linksStayLocal(first)).toBe(false)
  }, 180_000)

  it('판정 함수가 원격이라고 하면 거부한다(마지막·중간 구성 요소 모두)', () => {
    const j = mk('remoteish', join(base, 'real'))
    const remote = (t: string): boolean => t.toLowerCase().endsWith('real')
    expect(linksStayLocal(j, remote)).toBe(false)
    expect(linksStayLocal(join(j, 'sub', 'f.txt'), remote)).toBe(false)
    expect(linksStayLocal(join(base, 'real'), remote)).toBe(true)
  })

  it('중간 링크(b -> a\\sub, a -> 원격)는 a 에서 멈춰 거부한다(구성 요소 단위)', () => {
    const a = mk('hopA', join(base, 'real'))
    const b = mk('hopB', join(a, 'sub'))
    // a 의 대상(real)만 원격이라고 주입한다 — b 의 대상(hopA\sub)은 원격 모양이 아니다
    const remote = (t: string): boolean => t.toLowerCase().endsWith('real')
    expect(linksStayLocal(b)).toBe(true)
    expect(linksStayLocal(b, remote)).toBe(false)
    expect(linksStayLocal(join(b, 'f.txt'), remote)).toBe(false)
  })

  it('assertInsideRoot: 원격 판정 링크는 realpath 전에 invalid path', async () => {
    const root = join(base, 'repoA')
    mkdirSync(join(root, 'plain'), { recursive: true })
    const j = join(root, 'sm')
    junction(j, join(base, 'repoA', 'plain'))
    made.push(j)
    // 대상이 저장소 안이라 기본 판정이면 통과하고, 원격 판정을 주입하면 거부된다 → 거부 이유는 링크 대상 판정이다
    await expect(assertInsideRoot(root, j)).resolves.toBeUndefined()
    await expect(assertInsideRoot(root, j, () => true)).rejects.toThrow('invalid path')
    await expect(assertInsideRoot(root, join(root, 'plain'))).resolves.toBeUndefined()
  })

  it('assertNoLinkEscape: 저장소 경로 자체가 로컬 링크면 기존 규칙대로 동작한다', async () => {
    const real = join(base, 'repoB')
    mkdirSync(join(real, 'd'), { recursive: true })
    const j = mk('repoBlink', real)
    await expect(assertNoLinkEscape(j, 'd')).resolves.toBeUndefined()
  })

  it('scanLocalRepos: 원격 판정 링크는 건너뛰고, 로컬 정션은 따라간다', async () => {
    const root = join(base, 'scan')
    mkdirSync(join(root, 'plain', '.git'), { recursive: true })
    mkdirSync(join(base, 'scanTarget', '.git'), { recursive: true })
    const viaLink = join(root, 'viaLink')
    junction(viaLink, join(base, 'scanTarget'))
    made.push(viaLink)
    expect((await scanLocalRepos(root)).repos.map((r) => r.name)).toEqual(['plain', 'viaLink'])
    const skipped = await scanLocalRepos(root, (t) => t.toLowerCase().endsWith('scantarget'))
    expect(skipped.repos.map((r) => r.name)).toEqual(['plain'])
  })
})
