import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import type { Commit } from '../src/shared/types'
import { parseLog } from '../src/main/git/parsers/log'
import { LaneBuilder, mergedBranchName, SEG_BOTTOM, SEG_FULL, SEG_TOP, type GraphRow } from '../src/renderer/src/graph/lanes'

const c = (hash: string, parents: string[], extra: Partial<Commit> = {}) => ({ hash, parents, refs: [], subject: hash, ...extra })

function build(commits: ReturnType<typeof c>[]): GraphRow[] {
  const b = new LaneBuilder()
  return commits.map((x) => b.add(x))
}

/** 행 i 의 아래쪽에서 끝나는 레인 집합과 행 i+1 의 위쪽에서 시작하는 레인 집합이 같아야 선이 끊기지 않는다 */
function assertContinuous(rows: GraphRow[]): void {
  for (let i = 0; i + 1 < rows.length; i++) {
    const bottom = new Set<number>()
    const top = new Set<number>()
    const a = rows[i]!.segs
    for (let s = 0; s < a.length; s += 4) {
      if (a[s] === SEG_FULL || a[s] === SEG_BOTTOM) bottom.add(a[s + 2]!)
    }
    const b = rows[i + 1]!.segs
    for (let s = 0; s < b.length; s += 4) {
      if (b[s] === SEG_FULL || b[s] === SEG_TOP) top.add(b[s + 1]!)
    }
    expect([...top].sort(), `row ${i + 1}`).toEqual([...bottom].sort())
  }
}

describe('LaneBuilder', () => {
  it('직선 이력은 한 레인에 머문다', () => {
    const rows = build([c('c', ['b']), c('b', ['a']), c('a', [])])
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0])
    expect(rows[0]!.segs).toEqual([SEG_BOTTOM, 0, 0, rows[0]!.color])
    // 루트 커밋은 아래로 선이 없다
    expect(rows[2]!.segs.filter((_, i) => i % 4 === 0)).toEqual([SEG_TOP])
    assertContinuous(rows)
  })

  it('merge 는 두 번째 부모로 레인을 가르고 공통 조상에서 다시 모인다', () => {
    //   m  (merge of b, f)
    //   |\
    //   | f
    //   b |
    //   |/
    //   a
    const rows = build([c('m', ['b', 'f']), c('f', ['a']), c('b', ['a']), c('a', [])])
    expect(rows.map((r) => r.lane)).toEqual([0, 1, 0, 0])
    expect(rows[0]!.merge).toBe(true)
    // a 에서 레인 1 이 레인 0 으로 모여든다
    const a = rows[3]!.segs
    expect(a).toContain(SEG_TOP)
    const tops = []
    for (let s = 0; s < a.length; s += 4) if (a[s] === SEG_TOP) tops.push([a[s + 1], a[s + 2]])
    expect(tops).toEqual([
      [0, 0],
      [1, 0]
    ])
    assertContinuous(rows)
  })

  it('main 은 앰버(0) 고정, 같은 브랜치 이름은 항상 같은 색', () => {
    const main = [{ kind: 'local' as const, name: 'main' }]
    const feat = [{ kind: 'local' as const, name: 'feature/x' }]
    const r1 = build([c('f', ['a'], { refs: feat }), c('a', [], { refs: main })])
    const r2 = build([c('z', ['y'], { refs: feat }), c('y', [])])
    expect(r1[1]!.color).toBe(0) // feature 아래 main 커밋은 앰버로 바뀐다
    expect(r1[0]!.color).toBe(r2[0]!.color)
    expect(r1[0]!.color).not.toBe(0)
  })

  it('merge 제목에서 브랜치 이름을 읽어 두 번째 부모 레인 색을 정한다', () => {
    expect(mergedBranchName("Merge branch 'feature/이름-변경'")).toBe('feature/이름-변경')
    expect(mergedBranchName("Merge remote-tracking branch 'origin/dev' into main")).toBe('dev')
    expect(mergedBranchName('Merge pull request #12 from someone/fix/crash')).toBe('fix/crash')
    expect(mergedBranchName('feat: 일반 커밋')).toBeNull()
    const named = build([c('m', ['b', 'f'], { subject: "Merge branch 'dev'" }), c('f', ['b']), c('b', [])])
    const again = build([c('n', ['q', 'g'], { subject: "Merge branch 'dev'" }), c('g', ['q']), c('q', [])])
    expect(named[1]!.color).toBe(again[1]!.color)
  })

  it('끝난 레인은 비우고 다음 새 레인이 그 자리를 쓴다', () => {
    // 두 개의 독립 루트 이력
    const rows = build([c('b2', ['b1']), c('a2', ['a1']), c('b1', []), c('a1', []), c('x', [])])
    expect(rows.map((r) => r.lane)).toEqual([0, 1, 0, 1, 0])
    assertContinuous(rows)
  })
})

describe('LaneBuilder (실제 이력)', () => {
  let root: string
  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'antler-lanes-'))
  })
  afterAll(() => rmSync(root, { recursive: true, force: true }))

  it('fixture log 에서 선이 끊기지 않는다', () => {
    const commits = parseLog(readFileSync(join(__dirname, 'fixtures', 'log.bin')))
    assertContinuous(build(commits))
  })

  it('브랜치·merge 가 많은 3,000 커밋 이력에서 선이 끊기지 않고 폭이 제한된다', () => {
    const repo = join(root, 'big')
    execFileSync('node', [resolve(__dirname, '../scripts/make-big-repo.mjs'), repo, '3000'], { stdio: 'ignore' })
    const out = execFileSync(
      'git',
      ['-C', repo, 'log', '-z', '--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%D%x1f%s', '--decorate=full', '--date-order', '--branches', '--tags', 'HEAD'],
      { maxBuffer: 1 << 28 }
    )
    const commits = parseLog(out)
    expect(commits).toHaveLength(3000)
    const b = new LaneBuilder()
    const rows = commits.map((x) => b.add(x))
    assertContinuous(rows)
    // 노드 레인은 항상 그 행 폭 안에 있다
    rows.forEach((r) => expect(r.lane).toBeLessThan(r.width))
    expect(b.maxWidth).toBeLessThan(40)
  }, 60_000)
})
