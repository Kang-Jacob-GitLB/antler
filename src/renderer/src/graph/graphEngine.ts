// renderer 쪽 그래프 데이터 저장소. 레인 계산은 worker 에 맡기고 결과 행만 보관한다.
import type { Commit } from '@shared/types'
import type { LaneRequest, LaneResponse } from '../workers/laneWorker'
import type { GraphCommit, GraphRow } from './lanes'

interface Chunk {
  start: number
  rows: GraphRow[]
}

type Listener = () => void

/** tip 변경으로 다시 계산할 때 worker 로 한 번에 보내는 커밋 수 */
const REPLAY_CHUNK = 5000

/** worker 로는 레인 계산에 필요한 필드만 보낸다 */
const slim = (commits: readonly Commit[]): GraphCommit[] => commits.map(({ hash, parents, refs, subject }) => ({ hash, parents, refs, subject }))

class GraphEngine {
  private worker: Worker | null = null
  private gen = -1
  private chunks: Chunk[] = []
  private listeners = new Set<Listener>()
  /** 계산이 끝난 행 수 */
  count = 0
  maxWidth = 1
  /** 데이터가 바뀔 때마다 증가(구독자 리렌더용) */
  version = 0

  private ensureWorker(): Worker {
    if (!this.worker) {
      this.worker = new Worker(new URL('../workers/laneWorker.ts', import.meta.url), { type: 'module' })
      this.worker.onmessage = (e: MessageEvent<LaneResponse>) => this.receive(e.data)
    }
    return this.worker
  }

  /** 기본 브랜치 tip(굵은 줄기의 시작) */
  private defaultTip: string | null = null
  /** 이번 세대에 worker 로 보낸 행 수(이미 계산이 시작됐는지 판단용) */
  private sent = 0
  /**
   * worker 와 주고받는 세션 번호. 로그 세대(gen)와 별개로, tip 이 바뀌어 처음부터 다시 계산할 때도 증가해
   * 이전 계산의 늦은 응답을 걸러낸다.
   */
  private wgen = 0
  /**
   * tip 변경으로 다시 계산하는 중 — 다 끝날 때까지 이전 행을 그대로 보여 주고, 끝나면 한 번에 바꾼다.
   * (일부만 도착한 상태로 바꾸면 5000행 너머의 줄이 잠깐 사라지고 그래프 폭이 흔들린다)
   */
  private replay: { total: number; chunks: Chunk[]; count: number; maxWidth: number } | null = null

  /**
   * 기본 브랜치 tip 을 정한다. 아직 행을 보내기 전이면 이번 세대에 바로 반영하고, 이미 계산을 시작했다면
   * 로그를 다시 읽지 않고 지금까지 읽은 commits 로 레인을 처음부터 다시 계산한다(목록은 그대로 보인다).
   * 다시 계산하는 것은 같은 세대에서 새 tip 이 정해졌을 때뿐이다: tip 이 없어지는 경우(저장소 전환 중)나
   * commits 가 이번 세대에 보낸 것과 다르면(다른 저장소의 목록) 기록만 하고 재계산하지 않는다.
   */
  setDefaultTip(tip: string | null, commits: readonly Commit[]): void {
    if (tip === this.defaultTip) return
    this.defaultTip = tip
    if (this.sent === 0) {
      this.post({ type: 'reset', gen: ++this.wgen, tip })
      return
    }
    if (tip === null || commits.length !== this.sent) return
    const wgen = ++this.wgen
    this.post({ type: 'reset', gen: wgen, tip })
    this.replay = { total: commits.length, chunks: [], count: 0, maxWidth: 1 }
    for (let start = 0; start < commits.length; start += REPLAY_CHUNK) {
      this.post({ type: 'append', gen: wgen, start, commits: slim(commits.slice(start, start + REPLAY_CHUNK)) })
    }
  }

  /** 저장소가 바뀌었으면 이전 저장소의 tip 을 버린다(새 저장소의 refs 가 도착할 때까지 줄기를 표시하지 않는다) */
  clearDefaultTip(): void {
    this.defaultTip = null
  }

  reset(gen: number): void {
    this.gen = gen
    this.chunks = []
    this.count = 0
    this.sent = 0
    this.maxWidth = 1
    this.replay = null
    this.post({ type: 'reset', gen: ++this.wgen, tip: this.defaultTip })
    this.emit()
  }

  append(gen: number, start: number, commits: Commit[]): void {
    if (gen !== this.gen || commits.length === 0) return
    this.sent += commits.length
    this.post({ type: 'append', gen: this.wgen, start, commits: slim(commits) })
  }

  row(i: number): GraphRow | undefined {
    // 청크는 start 오름차순 — 이진 탐색
    let lo = 0
    let hi = this.chunks.length - 1
    while (lo <= hi) {
      const mid = (lo + hi) >> 1
      const ch = this.chunks[mid]!
      if (i < ch.start) hi = mid - 1
      else if (i >= ch.start + ch.rows.length) lo = mid + 1
      else return ch.rows[i - ch.start]
    }
    return undefined
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private post(msg: LaneRequest): void {
    this.ensureWorker().postMessage(msg)
  }

  private receive(res: LaneResponse): void {
    if (res.gen !== this.wgen) return
    const rp = this.replay
    if (rp) {
      if (res.start !== rp.count) return
      rp.chunks.push({ start: res.start, rows: res.rows })
      rp.count += res.rows.length
      rp.maxWidth = Math.max(1, res.maxWidth)
      if (rp.count < rp.total) return
      // 다시 계산이 끝났다 — 한꺼번에 바꾼다
      this.chunks = rp.chunks
      this.count = rp.count
      this.maxWidth = rp.maxWidth
      this.replay = null
      this.emit()
      return
    }
    if (res.start !== this.count) return
    this.chunks.push({ start: res.start, rows: res.rows })
    this.count += res.rows.length
    this.maxWidth = Math.max(1, res.maxWidth)
    this.emit()
  }

  private emit(): void {
    this.version++
    for (const fn of this.listeners) fn()
  }
}

export const graphEngine = new GraphEngine()
