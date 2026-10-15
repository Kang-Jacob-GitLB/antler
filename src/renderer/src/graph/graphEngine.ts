// renderer 쪽 그래프 데이터 저장소. 레인 계산은 worker 에 맡기고 결과 행만 보관한다.
import type { Commit } from '@shared/types'
import type { LaneRequest, LaneResponse } from '../workers/laneWorker'
import type { GraphRow } from './lanes'

interface Chunk {
  start: number
  rows: GraphRow[]
}

type Listener = () => void

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

  /** 기본 브랜치 tip(굵은 줄기의 시작). 다음 reset 부터 적용된다 */
  private defaultTip: string | null = null
  /** 이번 세대에 worker 로 보낸 행 수(이미 계산이 시작됐는지 판단용) */
  private sent = 0

  /**
   * 기본 브랜치 tip 을 정한다. 이미 그래프를 계산하기 시작했고 tip 이 (새로) 정해졌으면 true —
   * 호출자가 로그를 다시 읽어 줄기를 반영해야 한다. 아직 행을 보내기 전이면 이번 세대에 바로 반영한다.
   */
  setDefaultTip(tip: string | null): boolean {
    if (tip === this.defaultTip) return false
    this.defaultTip = tip
    if (this.sent === 0) {
      this.post({ type: 'reset', gen: this.gen, tip })
      return false
    }
    return tip !== null
  }

  reset(gen: number): void {
    this.gen = gen
    this.chunks = []
    this.count = 0
    this.sent = 0
    this.maxWidth = 1
    this.post({ type: 'reset', gen, tip: this.defaultTip })
    this.emit()
  }

  append(gen: number, start: number, commits: Commit[]): void {
    if (gen !== this.gen || commits.length === 0) return
    this.sent += commits.length
    // worker 로는 레인 계산에 필요한 필드만 보낸다
    const slim = commits.map(({ hash, parents, refs, subject }) => ({ hash, parents, refs, subject }))
    this.post({ type: 'append', gen, start, commits: slim })
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
    if (res.gen !== this.gen || res.start !== this.count) return
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
