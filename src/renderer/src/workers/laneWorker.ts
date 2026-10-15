/// <reference lib="webworker" />
// 레인 계산 worker. 커밋 페이지를 순서대로 받아 행별 그래프 데이터를 돌려준다.
import { LaneBuilder, type GraphCommit, type GraphRow } from '../graph/lanes'

export type LaneRequest =
  /** tip: 기본 브랜치 끝 커밋(굵게 그릴 줄기의 시작). 없으면 줄기를 표시하지 않는다 */
  | { type: 'reset'; gen: number; tip?: string | null }
  | { type: 'append'; gen: number; start: number; commits: GraphCommit[] }

export type LaneResponse = { type: 'rows'; gen: number; start: number; rows: GraphRow[]; maxWidth: number }

let builder = new LaneBuilder()
let gen = -1
let next = 0

self.onmessage = (e: MessageEvent<LaneRequest>) => {
  const m = e.data
  if (m.type === 'reset') {
    builder = new LaneBuilder(m.tip ?? null)
    gen = m.gen
    next = 0
    return
  }
  // 다른 세대이거나 순서가 어긋난 페이지는 버린다(곧 reset 이 뒤따른다)
  if (m.gen !== gen || m.start !== next) return
  const rows = m.commits.map((c) => builder.add(c))
  next += rows.length
  const res: LaneResponse = { type: 'rows', gen, start: m.start, rows, maxWidth: builder.maxWidth }
  self.postMessage(res)
}
