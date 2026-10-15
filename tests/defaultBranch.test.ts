import { describe, expect, it } from 'vitest'
import type { Branch, RemoteGroup } from '../src/shared/types'
import { pickDefaultBranch } from '../src/main/git/defaultBranch'
import { LaneBuilder, SEG_BOLD, SEG_BOTTOM, SEG_FULL, SEG_TOP, type GraphRow } from '../src/renderer/src/graph/lanes'

const br = (name: string, hash: string, remote = false): Branch => ({
  name,
  fullName: remote ? `refs/remotes/${name}` : `refs/heads/${name}`,
  hash,
  isHead: false
})
const group = (...branches: Branch[]): RemoteGroup => ({ name: 'origin', branches })

describe('pickDefaultBranch', () => {
  it('origin/HEAD 가 가리키는 원격 브랜치를 우선한다(로컬 main 이 앞서 있어도 원격 tip)', () => {
    const r = pickDefaultBranch('refs/remotes/origin/trunk\n', [br('main', 'L')], [group(br('origin/trunk', 'T', true), br('origin/main', 'M', true))])
    expect(r).toEqual({ name: 'origin/trunk', hash: 'T' })
  })
  it('origin/HEAD 가 없으면 로컬 main → master 순', () => {
    expect(pickDefaultBranch(null, [br('master', 'A'), br('main', 'B')], [])).toEqual({ name: 'main', hash: 'B' })
    expect(pickDefaultBranch(null, [br('dev', 'D'), br('master', 'A')], [])).toEqual({ name: 'master', hash: 'A' })
  })
  it('origin/HEAD 가 가리키는 브랜치를 찾을 수 없으면 대체 규칙으로 간다', () => {
    expect(pickDefaultBranch('refs/remotes/origin/gone', [br('main', 'B')], [group(br('origin/x', 'X', true))])).toEqual({ name: 'main', hash: 'B' })
    expect(pickDefaultBranch('garbage', [], [])).toBeUndefined()
  })
  it('아무것도 없으면 undefined', () => {
    expect(pickDefaultBranch(null, [br('dev', 'D')], [])).toBeUndefined()
  })
})

const c = (hash: string, parents: string[]) => ({ hash, parents, refs: [], subject: hash })
const boldSegs = (row: GraphRow): number[][] => {
  const out: number[][] = []
  for (let k = 0; k < row.segs.length; k += 4) if (row.segs[k + 3]! >= SEG_BOLD) out.push([row.segs[k]!, row.segs[k + 1]!, row.segs[k + 2]!])
  return out
}
const build = (tip: string | null, commits: ReturnType<typeof c>[]): GraphRow[] => {
  const b = new LaneBuilder(tip)
  return commits.map((x) => b.add(x))
}

describe('기본 브랜치 줄기 표시(LaneBuilder)', () => {
  it('tip 이 없으면 아무것도 굵지 않다', () => {
    const rows = build(null, [c('a', ['b']), c('b', [])])
    expect(rows.flatMap(boldSegs)).toEqual([])
  })

  it('선형 이력: tip 에서 루트까지 이어지는 선이 굵다(마지막 노드 아래는 선 없음)', () => {
    const rows = build('a', [c('a', ['b']), c('b', ['c']), c('c', [])])
    expect(boldSegs(rows[0]!)).toEqual([[SEG_BOTTOM, 0, 0]])
    expect(boldSegs(rows[1]!)).toEqual([[SEG_TOP, 0, 0], [SEG_BOTTOM, 0, 0]])
    expect(boldSegs(rows[2]!)).toEqual([[SEG_TOP, 0, 0]])
  })

  it('merge: first parent 로만 이어지고 두 번째 부모로 가는 선·가지 쪽 레인은 가늘다', () => {
    //  m(main tip) = merge(a, f)   a: main 의 이전, f: feature
    const rows = build('m', [c('m', ['a', 'f']), c('f', ['a']), c('a', ['r']), c('r', [])])
    // m: first parent 선만 굵다(두 번째 부모로 갈라지는 선은 가늘다)
    expect(boldSegs(rows[0]!)).toEqual([[SEG_BOTTOM, 0, 0]])
    // f 행: 레인 0 은 줄기가 a 를 기다리며 지나가므로 굵고, f 자신의 레인(1)은 가늘다
    expect(boldSegs(rows[1]!)).toEqual([[SEG_FULL, 0, 0]])
    // a 행: 줄기 레인(0)에서 들어오는 선은 굵고, f 에서 모여드는 선(1→0)은 가늘다
    expect(boldSegs(rows[2]!)).toEqual([[SEG_TOP, 0, 0], [SEG_BOTTOM, 0, 0]])
    expect(boldSegs(rows[3]!)).toEqual([[SEG_TOP, 0, 0]])
  })

  it('tip 이 아직 로드되지 않았으면 만날 때까지 표시하지 않고, 만난 뒤부터 이어 간다', () => {
    const rows = build('t', [c('x', ['y']), c('y', []), c('t', ['u']), c('u', [])])
    expect(rows[0]!.segs.every((v, i) => i % 4 !== 3 || v < SEG_BOLD)).toBe(true)
    expect(boldSegs(rows[1]!)).toEqual([])
    expect(boldSegs(rows[2]!)).toEqual([[SEG_BOTTOM, rows[2]!.lane, rows[2]!.lane]])
    expect(boldSegs(rows[3]!)).toEqual([[SEG_TOP, rows[3]!.lane, rows[3]!.lane]])
  })

  it('tip 이 로드되지 않은 채 끝나도 색 인덱스는 SEG_BOLD 에 오염되지 않는다', () => {
    const rows = build('zzz', [c('a', ['b']), c('b', [])])
    for (const r of rows) for (let k = 3; k < r.segs.length; k += 4) expect(r.segs[k]!).toBeLessThan(8)
  })
})

describe('늦게 정해진 기본 브랜치 tip(graphEngine)', () => {
  // laneWorker 를 같은 프로세스에서 돌리는 가짜 Worker — 메시지 순서·세대 규칙은 실제 worker 코드를 그대로 쓴다
  type Handler = (e: { data: unknown }) => void
  let toEngine: Handler | null = null
  const toWorker: { type: string; start?: number }[] = []
  const workerSelf: { onmessage: Handler | null; postMessage: (m: unknown) => void } = {
    onmessage: null,
    postMessage: (m) => queueMicrotask(() => toEngine?.({ data: m }))
  }
  class FakeWorker {
    set onmessage(h: Handler) {
      toEngine = h
    }
    postMessage(m: unknown): void {
      toWorker.push(m as { type: string; start?: number })
      queueMicrotask(() => workerSelf.onmessage?.({ data: m }))
    }
  }
  const settle = (): Promise<void> => new Promise((r) => setTimeout(r, 10))
  const mk = (list: ReturnType<typeof c>[]) => list.map((x) => ({ ...x, authorName: '', authorEmail: '' })) as never[]
  const load = async () => {
    Object.assign(globalThis, { self: workerSelf, Worker: FakeWorker })
    await import('../src/renderer/src/workers/laneWorker')
    return (await import('../src/renderer/src/graph/graphEngine')).graphEngine
  }
  const rowsOf = (e: Awaited<ReturnType<typeof load>>, n: number): GraphRow[] => Array.from({ length: n }, (_, i) => e.row(i)!)
  const commits = mk([c('m', ['a', 'f']), c('f', ['a']), c('a', ['r']), c('r', [])])

  it('로그를 다시 읽지 않고 읽어 둔 커밋으로 다시 계산해, 처음부터 tip 을 알았을 때와 같은 줄기가 된다', async () => {
    const graphEngine = await load()
    // 기준: tip 을 먼저 알고 시작
    graphEngine.setDefaultTip('m', [])
    graphEngine.reset(1)
    graphEngine.append(1, 0, commits)
    await settle()
    const early = rowsOf(graphEngine, 4).map(boldSegs)
    expect(early.flat().length).toBeGreaterThan(0)

    // 늦은 tip: 첫 페이지를 tip 없이 계산한 뒤 tip 이 도착
    graphEngine.clearDefaultTip()
    graphEngine.reset(2)
    graphEngine.append(2, 0, commits)
    await settle()
    expect(rowsOf(graphEngine, 4).flatMap(boldSegs)).toEqual([])
    graphEngine.setDefaultTip('m', commits)
    await settle()
    expect(rowsOf(graphEngine, 4).map(boldSegs)).toEqual(early)
    expect(graphEngine.count).toBe(commits.length)
  })

  it('저장소 전환 중(tip 이 없어지거나 다른 목록)에는 이전 저장소의 커밋을 다시 보내지 않는다', async () => {
    const graphEngine = await load()
    graphEngine.clearDefaultTip()
    graphEngine.reset(3)
    graphEngine.setDefaultTip('m', [])
    graphEngine.append(3, 0, commits)
    await settle()
    const before = toWorker.length
    // refs 가 비워져 tip 이 null 이 된다
    graphEngine.setDefaultTip(null, commits)
    // 새 저장소의 tip 이 왔지만 목록은 아직 이전 것(이번 세대에 보낸 수와 다르다)
    graphEngine.setDefaultTip('x', commits.slice(0, 2))
    await settle()
    expect(toWorker.length).toBe(before)
    expect(graphEngine.count).toBe(commits.length)
  })

  it('다시 계산하는 동안 이전 행을 유지하고, 5000행을 넘는 목록도 다 끝난 뒤 한 번에 바꾼다', async () => {
    const graphEngine = await load()
    const long = mk(Array.from({ length: 6000 }, (_, i) => c(`h${i}`, i < 5999 ? [`h${i + 1}`] : [])))
    graphEngine.clearDefaultTip()
    graphEngine.reset(4)
    graphEngine.append(4, 0, long)
    await settle()
    expect(graphEngine.count).toBe(6000)
    const seen: number[] = []
    const off = graphEngine.subscribe(() => seen.push(graphEngine.count))
    graphEngine.setDefaultTip('h0', long)
    await settle()
    off()
    // 중간(5000행)만 보이는 상태 없이 곧바로 6000 으로 바뀐다
    expect(seen).toEqual([6000])
    expect(graphEngine.row(5999)).toBeDefined()
  })
})
