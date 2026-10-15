import { describe, expect, it } from 'vitest'
import type { Branch, RefsSnapshot } from '../src/shared/types'
import { carryDetails, withDetails } from '../src/renderer/src/lib/refsDetails'

const br = (name: string, extra: Partial<Branch> = {}): Branch => ({
  name,
  fullName: `refs/heads/${name}`,
  hash: name,
  isHead: false,
  upstream: `origin/${name}`,
  ...extra
})
const snap = (...local: Branch[]): RefsSnapshot => ({
  head: { branch: null, hash: null },
  local,
  remotes: [],
  tags: [],
  worktrees: [],
  stashes: [],
  submodules: []
})
const by = (s: RefsSnapshot, n: string): Branch => s.local.find((b) => b.name === n)!

describe('withDetails / carryDetails', () => {
  it('push 로 동기화된 브랜치는 낡은 ↑ 를 지운다(tracking 에서 빠진 브랜치 = 동기화됨)', () => {
    const prev = withDetails(snap(br('main')), { tracking: { 'refs/heads/main': { ahead: 2 } }, submodules: [] })
    expect(by(prev, 'main').ahead).toBe(2)
    // 새 스냅샷에 이전 값을 먼저 보여 주고(carry), 이어 도착한 상세로 바꾼다
    const fresh = snap(br('main'))
    const carried = carryDetails(fresh, prev)
    expect(by(carried, 'main').ahead).toBe(2)
    const merged = withDetails(fresh, { tracking: {}, submodules: [] })
    expect(by(merged, 'main').ahead).toBeUndefined()
  })

  it('ahead 2 → behind 3 으로 바뀌면 ↑2 가 남지 않고, 사라졌던 업스트림 표시도 풀린다', () => {
    const prev = withDetails(snap(br('a'), br('b')), {
      tracking: { 'refs/heads/a': { ahead: 2 }, 'refs/heads/b': { upstreamGone: true } },
      submodules: []
    })
    const merged = withDetails(snap(br('a'), br('b')), { tracking: { 'refs/heads/a': { behind: 3 } }, submodules: [] })
    expect(by(merged, 'a')).toMatchObject({ behind: 3 })
    expect(by(merged, 'a').ahead).toBeUndefined()
    expect(by(merged, 'b').upstreamGone).toBeUndefined()
    expect(by(prev, 'b').upstreamGone).toBe(true)
  })

  it('업스트림이 없는 브랜치는 건드리지 않는다', () => {
    const merged = withDetails(snap(br('x', { upstream: undefined })), { tracking: {}, submodules: [] })
    expect(by(merged, 'x').upstream).toBeUndefined()
  })
})
