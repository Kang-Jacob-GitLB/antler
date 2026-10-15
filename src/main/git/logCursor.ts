import type { Commit, LogPage } from '@shared/types'
import { AbortError, GitError, type GitExecutor } from '../executors/GitExecutor'
import { isEmptyRepoError } from './commands'
import { parseLogRecord } from './parsers/log'
import { NulRecordSplitter } from './parsers/nulRecords'

/**
 * 하나의 `git log` 프로세스를 열어 두고 페이지 단위로 이어 읽는 커서.
 *
 * git log 는 시작할 때마다 모든 ref 를 읽는다. loose ref 가 많은 저장소(Windows 에서 특히)는
 * 이 고정 비용이 수백 ms 라서, 페이지마다 `--skip` 으로 새 프로세스를 띄우면 스크롤을 따라가지 못한다.
 * 커서는 다음 페이지가 직전 페이지 바로 뒤일 때 같은 프로세스에서 이어 읽고, stdout 을 소비하지 않는 동안에는
 * 스트림 backpressure 로 git 이 자연히 멈춘다. 순서를 벗어난 요청은 호출자가 `--skip` 위치로 새 커서를 만든다.
 *
 * 멈춰 있는 git 프로세스는 pack 파일 핸들을 쥐고 있어 사용자의 `git gc` 를 막을 수 있으므로,
 * 호출자는 짧은 유휴 시간 뒤 커서를 닫아야 한다(RepoService 참고).
 */
export class LogCursor {
  private iter: AsyncIterator<Buffer> | null
  private readonly splitter = new NulRecordSplitter()
  private buffered: Commit[] = []
  private done = false
  private failed = false
  private readonly ac = new AbortController()
  private busy: Promise<unknown> = Promise.resolve()
  private pending = 0
  /** 지금까지 페이지로 내보낸 커밋 수 */
  position: number
  /** 대기 중인 read 까지 반영한 다음 시작 위치. 재사용 판정은 이 값으로 한다 */
  nextPosition: number
  /** 마지막으로 내보낸 커밋 해시. 커서를 다시 만들 때 이어지는지 검증하는 데 쓴다 */
  lastHash: string | null = null
  lastUsed = Date.now()

  constructor(
    exec: GitExecutor,
    repoPath: string,
    /** skip 위치를 받아 git log 인자를 만든다 */
    buildArgs: (skip: number) => string[],
    startAt = 0
  ) {
    this.position = startAt
    this.nextPosition = startAt
    this.iter = exec.stream(repoPath, buildArgs(startAt), { signal: this.ac.signal })[Symbol.asyncIterator]()
  }

  /** 닫혔거나 실패한 커서는 재사용하면 안 된다 */
  get usable(): boolean {
    return this.iter !== null && !this.failed
  }

  /** 읽는 중이거나 대기 중인 요청이 있는지 */
  get reading(): boolean {
    return this.pending > 0
  }

  /** count 개를 읽는다. 동시 호출은 순서대로 처리하며, 각 호출의 시작 위치는 호출 시점에 예약된다. */
  read(count: number): Promise<LogPage> {
    this.nextPosition += count
    this.pending++
    const run = this.busy.then(() => this.readNow(count))
    this.busy = run.catch(() => {})
    void run.finally(() => this.pending--).catch(() => {})
    return run
  }

  private async readNow(count: number): Promise<LogPage> {
    this.lastUsed = Date.now()
    try {
      // 다음 페이지 존재 여부를 알기 위해 하나 더 확보한다
      while (this.buffered.length <= count && !this.done) {
        if (!this.iter) throw new AbortError()
        let next: IteratorResult<Buffer>
        try {
          next = await this.iter.next()
        } catch (err) {
          // 커밋이 하나도 없는 새 저장소는 빈 목록으로 취급한다
          if (err instanceof GitError && isEmptyRepoError(err.stderr)) next = { done: true, value: undefined }
          else throw err
        }
        // 기다리는 사이 dispose 됐으면 남은 버퍼를 믿지 않는다
        if (!this.iter) throw new AbortError()
        if (next.done) {
          this.done = true
          this.pushRecords(this.splitter.end())
        } else {
          this.pushRecords(this.splitter.push(next.value))
        }
      }
    } catch (err) {
      this.failed = true
      this.dispose()
      throw err
    }
    const commits = this.buffered.splice(0, count)
    this.position += commits.length
    // 끝에 닿아 덜 받았으면 예약했던 위치를 되돌린다
    this.nextPosition -= count - commits.length
    if (commits.length) this.lastHash = commits[commits.length - 1]!.hash
    this.lastUsed = Date.now()
    return { commits, hasMore: this.buffered.length > 0 || !this.done }
  }

  private pushRecords(records: string[]): void {
    for (const r of records) {
      const c = parseLogRecord(r)
      if (c) this.buffered.push(c)
    }
  }

  dispose(): void {
    this.ac.abort()
    void this.iter?.return?.()?.catch(() => {})
    this.iter = null
    this.buffered = []
  }
}
