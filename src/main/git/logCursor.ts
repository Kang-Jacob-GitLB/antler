import type { Commit, LogPage } from '@shared/types'
import { GitError, type GitExecutor } from '../executors/GitExecutor'
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
 */
export class LogCursor {
  private iter: AsyncIterator<Buffer> | null
  private readonly splitter = new NulRecordSplitter()
  private buffered: Commit[] = []
  private done = false
  private readonly ac = new AbortController()
  private busy: Promise<unknown> = Promise.resolve()
  /** 지금까지 페이지로 내보낸 커밋 수 = 다음에 이어 읽을 위치 */
  position: number
  lastUsed = Date.now()

  constructor(
    exec: GitExecutor,
    repoPath: string,
    /** skip 위치를 받아 git log 인자를 만든다 */
    buildArgs: (skip: number) => string[],
    startAt = 0
  ) {
    this.position = startAt
    this.iter = exec.stream(repoPath, buildArgs(startAt), { signal: this.ac.signal })[Symbol.asyncIterator]()
  }

  /** count 개를 읽는다. 동시 호출은 순서대로 처리한다. */
  read(count: number): Promise<LogPage> {
    const run = this.busy.then(() => this.readNow(count))
    this.busy = run.catch(() => {})
    return run
  }

  private async readNow(count: number): Promise<LogPage> {
    if (!this.iter) throw new Error('cursor disposed')
    this.lastUsed = Date.now()
    // 다음 페이지 존재 여부를 알기 위해 하나 더 확보한다
    while (this.buffered.length <= count && !this.done) {
      let next: IteratorResult<Buffer>
      try {
        next = await this.iter.next()
      } catch (err) {
        // 커밋이 하나도 없는 새 저장소는 빈 목록으로 취급한다
        if (err instanceof GitError && isEmptyRepoError(err.stderr)) next = { done: true, value: undefined }
        else throw err
      }
      if (next.done) {
        this.done = true
        this.pushRecords(this.splitter.end())
      } else {
        this.pushRecords(this.splitter.push(next.value))
      }
    }
    const commits = this.buffered.splice(0, count)
    this.position += commits.length
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
