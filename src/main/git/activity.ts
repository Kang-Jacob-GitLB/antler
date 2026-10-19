// 커밋 활동(미니맵) 집계. 커밋 시각만 스트림으로 읽어 시간 구간별 개수로 줄인다.
import type { CommitActivity, LogScope } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { isEmptyRepoError, scopeArgs } from './commands'

/** 목표 구간 수. 실제로는 아래 단위 중 구간 수가 MAX_BUCKETS 이하가 되는 가장 작은 단위를 고른다 */
export const TARGET_BUCKETS = 300
export const MAX_BUCKETS = 400
const HOUR = 3600
const DAY = 24 * HOUR
/** 구간 단위(초): 1시간 · 6시간 · 1일 · 1주 · 30일 · 1년 */
const STEPS = [HOUR, 6 * HOUR, DAY, 7 * DAY, 30 * DAY, 365 * DAY]
/** 시계가 어긋난 미래 커밋 허용 폭 — 이보다 먼 미래 값은 차트 범위에서 제외한다 */
const FUTURE_SLACK = 2 * DAY

export function emptyActivity(): CommitActivity {
  return { buckets: [], first: 0, last: 0, total: 0, bucketSec: DAY }
}

/**
 * 커밋 시각(epoch 초) 목록을 구간별 개수로 집계한다.
 * 0 이하이거나 `now` 보다 훨씬 미래인 값은 차트 범위를 왜곡하므로 구간에서 빼지만 total 에는 센다.
 * 구간은 비어 있어도 연속으로 채워(개수 0) 시간축이 끊기지 않게 한다.
 */
export function bucketize(times: ArrayLike<number>, n = times.length, now = Date.now() / 1000): CommitActivity {
  const limit = now + FUTURE_SLACK
  let first = Infinity
  let last = -Infinity
  for (let i = 0; i < n; i++) {
    const t = times[i]!
    if (t <= 0 || t > limit) continue
    if (t < first) first = t
    if (t > last) last = t
  }
  if (first === Infinity) return { ...emptyActivity(), total: n }

  const span = last - first
  const bucketSec = STEPS.find((s) => span / s + 1 <= MAX_BUCKETS) ?? STEPS[STEPS.length - 1]!
  const origin = Math.floor(first / bucketSec) * bucketSec
  const count = Math.floor((last - origin) / bucketSec) + 1
  const counts = new Uint32Array(count)
  for (let i = 0; i < n; i++) {
    const t = times[i]!
    if (t <= 0 || t > limit) continue
    counts[Math.floor((t - origin) / bucketSec)]!++
  }
  const buckets = new Array<{ t: number; count: number }>(count)
  for (let i = 0; i < count; i++) buckets[i] = { t: origin + i * bucketSec, count: counts[i]! }
  return { buckets, first, last, total: n, bucketSec }
}

/** 줄바꿈으로 구분된 십진수 스트림 파서. 청크 경계에 걸친 숫자를 이어 붙인다 */
export class TimeStreamParser {
  private buf = new Uint32Array(1 << 14)
  n = 0
  private cur = 0
  private inNum = false

  push(chunk: Uint8Array): void {
    for (let i = 0; i < chunk.length; i++) {
      const c = chunk[i]!
      if (c >= 48 && c <= 57) {
        this.cur = this.cur * 10 + (c - 48)
        this.inNum = true
      } else if (this.inNum) this.flush()
    }
  }

  end(): void {
    if (this.inNum) this.flush()
  }

  private flush(): void {
    if (this.n === this.buf.length) {
      const bigger = new Uint32Array(this.buf.length * 2)
      bigger.set(this.buf)
      this.buf = bigger
    }
    this.buf[this.n++] = this.cur
    this.cur = 0
    this.inNum = false
  }

  get values(): Uint32Array {
    return this.buf
  }
}

/** 보기 범위(scope)의 모든 커밋 작성 시각을 스트림으로 읽어 집계한다. 목록과 같은 scopeArgs 를 쓴다 */
export async function commitActivity(
  exec: GitExecutor,
  repoPath: string,
  scope: LogScope | undefined,
  opts?: RunOpts
): Promise<CommitActivity> {
  // %at: 목록의 authorDate 와 같은 필드 — 뷰포트 표시·점프가 어긋나지 않게 한다
  const args = ['log', '--format=%at', ...scopeArgs(scope), '--']
  const parser = new TimeStreamParser()
  try {
    for await (const chunk of exec.stream(repoPath, args, opts)) parser.push(chunk)
  } catch (err) {
    // 커밋이 하나도 없는 새 저장소는 빈 차트로 취급한다
    if (!(err instanceof GitError && isEmptyRepoError(err.stderr))) throw err
  }
  parser.end()
  return bucketize(parser.values, parser.n)
}
