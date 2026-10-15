// 커밋 그래프 레인 계산. DOM 에 의존하지 않는 순수 모듈로, laneWorker 와 테스트가 함께 쓴다.
//
// 입력은 자식이 부모보다 먼저 오는 순서(git log --date-order)다. 위에서 아래로 한 번 훑으며
// "각 레인이 다음에 만날 것으로 기대하는 커밋 해시"를 유지한다.
//
// 한 행은 세 종류의 선분으로 그린다(행 높이를 위·아래 반으로 나눈다).
//   FULL   위 → 아래 그대로 지나가는 레인
//   TOP    행 위쪽 → 노드 (from 레인에서 노드 레인으로 모여드는 선, 자기 레인이면 수직선)
//   BOTTOM 노드 → 행 아래쪽 (첫 부모는 자기 레인, 나머지 부모는 다른 레인으로 갈라지는 선)
import type { RefLabel } from '@shared/types'
import { branchKey, LANE_COUNT, laneIndexFor } from '../lib/laneColor'

export const SEG_FULL = 0
export const SEG_TOP = 1
export const SEG_BOTTOM = 2

/** 레인 계산에 필요한 커밋 필드만 */
export interface GraphCommit {
  hash: string
  parents: string[]
  refs: RefLabel[]
  subject: string
}

export interface GraphRow {
  /** 노드가 놓이는 레인 */
  lane: number
  /** 노드 색 인덱스(0..7, --lane-N) */
  color: number
  merge: boolean
  /** [kind, fromLane, toLane, color] × n */
  segs: number[]
  /** 이 행을 지나는 레인 수(그래프 폭 계산용) */
  width: number
}

const MAIN_COLOR = 0
const MERGE_PATTERNS = [
  /^Merge (?:remote-tracking )?branch '([^']+)'/,
  /^Merge pull request #\d+ from [^/\s]+\/(\S+)/,
  /^Merge branch "([^"]+)"/
]

/** merge 커밋 제목에서 합쳐진 브랜치 이름을 추정한다 */
export function mergedBranchName(subject: string): string | null {
  for (const re of MERGE_PATTERNS) {
    const m = re.exec(subject)
    if (m?.[1]) return m[1].replace(/^origin\//, '')
  }
  return null
}

/** 커밋이 끝(tip)인 브랜치의 색. 로컬 → HEAD → 리모트 순으로 고른다 */
function tipColor(refs: RefLabel[]): number | null {
  const local = refs.find((r) => r.kind === 'local')
  if (local) return laneIndexFor(local.name)
  const head = refs.find((r) => r.kind === 'head' && r.name !== 'HEAD')
  if (head) return laneIndexFor(head.name)
  const remote = refs.find((r) => r.kind === 'remote')
  if (remote) return laneIndexFor(branchKey(remote.name, true))
  return null
}

function hasMainRef(refs: RefLabel[]): boolean {
  return refs.some(
    (r) => (r.kind === 'local' || r.kind === 'head' || r.kind === 'remote') && laneIndexFor(branchKey(r.name, r.kind === 'remote')) === MAIN_COLOR
  )
}

export class LaneBuilder {
  /** 레인별로 다음에 나타날 것으로 기대하는 커밋 해시 */
  private lanes: (string | null)[] = []
  private colors: number[] = []
  private rotate = 0
  maxWidth = 0

  add(c: GraphCommit): GraphRow {
    const segs: number[] = []
    const before = this.lanes.length
    let lane = this.lanes.indexOf(c.hash)
    const continued = lane !== -1

    if (!continued) {
      lane = this.freeLane()
      this.lanes[lane] = c.hash
      this.colors[lane] = tipColor(c.refs) ?? this.pickColor(lane)
    } else if (hasMainRef(c.refs)) {
      // 다른 브랜치가 갈라져 나온 main 커밋: 이 아래로는 main 레인이므로 앰버로 바꾼다
      this.colors[lane] = MAIN_COLOR
    }

    // ── 위 절반: 지나가는 레인과 노드로 모여드는 레인 ──
    for (let j = 0; j < before; j++) {
      const expect = this.lanes[j]
      if (expect == null) continue
      if (expect === c.hash) {
        if (j !== lane || continued) segs.push(SEG_TOP, j, lane, this.colors[j]!)
      } else {
        segs.push(SEG_FULL, j, j, this.colors[j]!)
      }
    }
    for (let j = 0; j < this.lanes.length; j++) {
      if (j !== lane && this.lanes[j] === c.hash) this.lanes[j] = null
    }

    const color = this.colors[lane]!

    // ── 아래 절반: 부모로 이어지는 선 ──
    const [first, ...rest] = c.parents
    if (first === undefined) {
      this.lanes[lane] = null
    } else {
      this.lanes[lane] = first
      segs.push(SEG_BOTTOM, lane, lane, color)
      rest.forEach((p, i) => {
        let k = this.lanes.indexOf(p)
        if (k === -1) {
          k = this.freeLane(lane)
          this.lanes[k] = p
          const name = i === 0 ? mergedBranchName(c.subject) : null
          this.colors[k] = name !== null ? laneIndexFor(name) : this.pickColor(k, color)
        }
        segs.push(SEG_BOTTOM, lane, k, this.colors[k]!)
      })
    }

    const width = Math.max(before, this.lanes.length)
    while (this.lanes.length && this.lanes[this.lanes.length - 1] == null) {
      this.lanes.pop()
      this.colors.pop()
    }
    if (width > this.maxWidth) this.maxWidth = width
    return { lane, color, merge: c.parents.length > 1, segs, width }
  }

  private freeLane(except = -1): number {
    for (let i = 0; i < this.lanes.length; i++) if (i !== except && this.lanes[i] == null) return i
    this.lanes.push(null)
    this.colors.push(0)
    return this.lanes.length - 1
  }

  /** 이름으로 색을 정할 수 없을 때: 1..7 을 돌리되 이웃 레인·avoid 와 겹치지 않게 한다 */
  private pickColor(lane: number, avoid = -1): number {
    const neighbors = new Set([this.colors[lane - 1], this.colors[lane + 1], avoid])
    for (let n = 0; n < LANE_COUNT - 1; n++) {
      const c = 1 + ((this.rotate + n) % (LANE_COUNT - 1))
      if (!neighbors.has(c)) {
        this.rotate = (this.rotate + n + 1) % (LANE_COUNT - 1)
        return c
      }
    }
    return 1 + (this.rotate++ % (LANE_COUNT - 1))
  }
}
