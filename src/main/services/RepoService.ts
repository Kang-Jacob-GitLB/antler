import { basename } from 'node:path'
import type {
  CommitDetail,
  FileDiff,
  FileDiffRequest,
  LogPage,
  LogQuery,
  RefsSnapshot,
  Repo,
  SearchResult,
  Source,
  SourceKind
} from '@shared/types'
import { AbortError, type GitExecutor } from '../executors/GitExecutor'
import { LocalExecutor } from '../executors/LocalExecutor'
import { ProcessQueue } from '../executors/ProcessQueue'
import * as git from '../git/commands'
import { LogCursor } from '../git/logCursor'
import { LOCAL_SOURCE_ID, type ConfigStore } from './ConfigStore'
import { LruCache } from './lru'

/** 환경별 동시 git 프로세스 상한(LogCursor 는 저장소당 하나라 큐 밖에서 돈다) */
const CONCURRENCY: Record<SourceKind, number> = { local: 6, wsl: 4, ssh: 4 }
/**
 * 이 시간 동안 안 쓰인 LogCursor 는 닫는다. 멈춘 git log 는 pack 파일 핸들을 쥐고 있어
 * 사용자의 `git gc`/`repack` 이나 폴더 이동을 막으므로 짧게 둔다.
 */
const CURSOR_IDLE_MS = 5_000
const CURSOR_SWEEP_MS = 2_500
/** 커밋 상세 캐시 항목 수와 총 크기(추정 바이트). 파일이 수천 개인 merge 가 많아도 메모리가 묶이도록 둘 다 건다 */
const COMMIT_CACHE_ENTRIES = 300
const COMMIT_CACHE_BYTES = 24 * 1024 * 1024
/** diff 캐시 총 크기(추정 바이트) */
const DIFF_CACHE_BYTES = 32 * 1024 * 1024
const DIFF_CACHE_ENTRIES = 500

/** 같은 키의 진행 중 요청. 구독자(waiters)가 모두 취소했을 때만 git 을 끊는다 */
interface Inflight<T> {
  ac: AbortController
  waiters: number
  promise: Promise<T>
}

/** diff 가 차지하는 메모리 추정치(바이트). 줄 객체 오버헤드를 고정값으로 어림한다 */
export function estimateDiffBytes(d: FileDiff): number {
  let n = 256 + d.path.length * 2 + (d.oldPath?.length ?? 0) * 2
  for (const m of d.meta) n += m.length * 2 + 16
  for (const h of d.hunks) {
    n += 96 + h.header.length * 2
    for (const l of h.lines) n += 48 + l.text.length * 2
  }
  return n
}

/** 커밋 상세가 차지하는 메모리 추정치(바이트) */
export function estimateCommitBytes(d: CommitDetail): number {
  let n = 512 + (d.subject.length + d.body.length + d.authorName.length + d.committerName.length) * 2
  for (const f of d.files) n += 96 + (f.path.length + (f.oldPath?.length ?? 0)) * 2
  return n
}

/** 이어 읽으려던 위치의 커밋이 바뀌었다 — renderer 는 처음부터 다시 읽어야 한다 */
export class StaleLogError extends Error {
  constructor() {
    super('commit history changed while paging')
    this.name = 'StaleLogError'
  }
}

interface Runtime {
  exec: GitExecutor
  queue: ProcessQueue
}

/**
 * 저장소 단위 git 조회의 진입점. 소스별 executor·큐를 소유하고, 요청 취소(requestId)를 관리한다.
 * 커밋 목록은 ref 이동으로 바뀔 수 있어 캐시하지 않고 LogCursor 로 이어 읽는다
 * (커밋 해시 기반 상세/diff 는 불변이라 LRU 로 캐시한다).
 */
export class RepoService {
  private readonly runtimes = new Map<string, Runtime>()
  private readonly requests = new Map<string, AbortController>()
  private readonly cursors = new Map<string, LogCursor>()
  /** 닫힌 커서의 마지막 위치. 다음 페이지가 이어지는지 검증하는 데 쓴다 */
  private readonly anchors = new Map<string, { position: number; hash: string }>()
  /** 커밋 해시 기준이라 불변이다 — 무효화하지 않는다 */
  private readonly commitCache = new LruCache<string, CommitDetail>(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES)
  private readonly diffCache = new LruCache<string, FileDiff>(DIFF_CACHE_ENTRIES, DIFF_CACHE_BYTES)
  private readonly inflight = new Map<string, Inflight<unknown>>()
  private readonly sweeper = setInterval(() => this.sweepIdleCursors(), CURSOR_SWEEP_MS)

  constructor(private readonly config: ConfigStore) {
    this.sweeper.unref?.()
  }

  async addLocal(path: string): Promise<Repo> {
    const rt = this.runtime(this.config.source(LOCAL_SOURCE_ID)!)
    const top = await git.resolveTopLevel(rt.exec, path)
    // Windows git 은 C:/a/b 형식으로 돌려주므로 사용자가 고른 표기(역슬래시)로 맞춘다
    const normalized = process.platform === 'win32' ? top.replace(/\//g, '\\') : top
    return this.config.addRepo({ sourceId: LOCAL_SOURCE_ID, path: normalized, name: basename(normalized) })
  }

  refs(repoId: string): Promise<RefsSnapshot> {
    const { repo, rt } = this.resolve(repoId)
    return rt.queue.run(() => git.refs(rt.exec, repo.path), { key: `refs:${repoId}` })
  }

  /**
   * 메시지·작성자·해시 검색. 전체 이력을 훑을 수 있어 비싸므로 requestId 로 취소할 수 있게 하고,
   * renderer 는 새 질의를 보낼 때 이전 검색을 취소한다(큐 슬롯을 계속 쥐고 스크롤을 막지 않게).
   */
  search(repoId: string, query: string, ref?: string, requestId?: string): Promise<SearchResult> {
    const { repo, rt } = this.resolve(repoId)
    return this.withSignal(requestId, (signal) =>
      rt.queue.run(() => git.search(rt.exec, repo.path, query, ref, { signal }), { signal })
    )
  }

  /** 커밋 상세(메타데이터 + 첫 부모 대비 변경 파일). 해시 기준 불변 캐시 + 진행 중 요청 공유 */
  commit(repoId: string, hash: string, requestId?: string): Promise<CommitDetail> {
    const { repo, rt } = this.resolve(repoId)
    const key = `${repoId}\0${hash.toLowerCase()}`
    const cached = this.commitCache.get(key)
    if (cached) return Promise.resolve(cached)
    return this.shared(key, requestId, async (signal) => {
      const detail = await rt.queue.run(() => git.commitDetail(rt.exec, repo.path, hash, { signal }), { signal })
      this.commitCache.set(key, detail, estimateCommitBytes(detail))
      return detail
    })
  }

  /** 파일 하나의 diff. 상한을 넘으면 truncated, req.full 이면 전체(별도 캐시 항목) */
  fileDiff(repoId: string, req: FileDiffRequest, requestId?: string): Promise<FileDiff> {
    const { repo, rt } = this.resolve(repoId)
    const key = [repoId, req.hash.toLowerCase(), req.parent?.toLowerCase() ?? '', req.oldPath ?? '', req.path, req.full ? 'full' : 'part'].join('\0')
    const cached = this.diffCache.get(key)
    if (cached) return Promise.resolve(cached)
    return this.shared(key, requestId, async (signal) => {
      const diff = await rt.queue.run(() => git.fileDiff(rt.exec, repo.path, req, { signal }), { signal })
      this.diffCache.set(key, diff, estimateDiffBytes(diff))
      return diff
    })
  }

  /**
   * 키가 같은 진행 중 요청을 공유한다. 공유 요청의 취소는 구독자 전원이 취소했을 때만 git 에 전달되므로
   * 한 호출자의 cancel 이 다른 호출자의 결과를 죽이지 않는다.
   */
  private shared<T>(key: string, requestId: string | undefined, start: (signal: AbortSignal) => Promise<T>): Promise<T> {
    return this.withSignal(requestId, (signal) => {
      let entry = this.inflight.get(key) as Inflight<T> | undefined
      if (!entry) {
        const ac = new AbortController()
        const created: Inflight<T> = { ac, waiters: 0, promise: undefined as unknown as Promise<T> }
        created.promise = start(ac.signal).finally(() => {
          if (this.inflight.get(key) === created) this.inflight.delete(key)
        })
        this.inflight.set(key, created)
        entry = created
      }
      const e = entry
      e.waiters++
      return new Promise<T>((resolve, reject) => {
        let settled = false
        const onAbort = (): void => {
          if (settled) return
          settled = true
          if (--e.waiters === 0) {
            if (this.inflight.get(key) === e) this.inflight.delete(key)
            e.ac.abort()
          }
          reject(new AbortError())
        }
        if (signal?.aborted) return onAbort()
        signal?.addEventListener('abort', onAbort, { once: true })
        e.promise.then(
          (v) => {
            if (settled) return
            settled = true
            signal?.removeEventListener('abort', onAbort)
            resolve(v)
          },
          (err) => {
            if (settled) return
            settled = true
            signal?.removeEventListener('abort', onAbort)
            reject(err)
          }
        )
      })
    })
  }

  /**
   * 커밋 한 페이지. 직전 페이지에 이어지는 요청은 열려 있는 LogCursor 에서 이어 읽는다.
   * 커서는 활성 저장소 하나에만 유지한다(다른 저장소의 커서는 이 시점에 닫아 메모리·프로세스를 돌려준다).
   */
  log(repoId: string, query: LogQuery, requestId?: string): Promise<LogPage> {
    const { repo, rt } = this.resolve(repoId)
    const count = Math.min(Math.max(1, Math.floor(query.count)), 5000)
    const skip = Math.max(0, Math.floor(query.skip))
    const key = `${repoId}\0${query.ref ?? ''}`

    for (const [k, c] of this.cursors) if (k !== key) this.retire(k, c)
    if (skip === 0) this.anchors.delete(key)

    let cursor = this.cursors.get(key)
    // 예약된 다음 위치가 맞고 살아 있는 커서만 이어 쓴다
    let verify: string | null = null
    if (!cursor || !cursor.usable || cursor.nextPosition !== skip) {
      if (cursor) this.retire(key, cursor)
      // 닫힌 커서의 마지막 커밋 위치에서 다시 시작하면, 그 커밋이 그대로인지 한 개 겹쳐 읽어 확인한다
      const anchor = this.anchors.get(key)
      const start = anchor && anchor.position === skip && skip > 0 ? skip - 1 : skip
      if (start !== skip) verify = anchor!.hash
      cursor = new LogCursor(rt.exec, repo.path, (s) => git.logArgs(query.ref, s), start)
      this.cursors.set(key, cursor)
    }
    const active = cursor
    const expected = verify
    return this.withSignal(requestId, async (signal) => {
      const page = (async () => {
        if (expected) {
          const head = await active.read(1)
          if (head.commits[0]?.hash !== expected) {
            // 그 사이 ref 가 움직였다 — 이어 붙이면 중복·누락이 생기므로 renderer 가 처음부터 다시 읽게 한다
            this.retire(key, active)
            this.anchors.delete(key)
            throw new StaleLogError()
          }
        }
        return active.read(count)
      })()
      if (!signal) return page
      return Promise.race([
        page,
        new Promise<never>((_, reject) =>
          signal.addEventListener(
            'abort',
            () => {
              // 읽는 도중 취소되면 커서 위치를 신뢰할 수 없으므로 버린다
              active.dispose()
              if (this.cursors.get(key) === active) this.cursors.delete(key)
              page.catch(() => {})
              reject(new AbortError())
            },
            { once: true }
          )
        )
      ])
    })
  }

  cancel(requestId: string): void {
    this.requests.get(requestId)?.abort()
    this.requests.delete(requestId)
  }

  /** 커서를 닫는다. 이어 읽을 수 있도록 마지막 위치·해시를 앵커로 남긴다 */
  private retire(key: string, cursor: LogCursor): void {
    if (cursor.usable && cursor.lastHash) this.anchors.set(key, { position: cursor.position, hash: cursor.lastHash })
    cursor.dispose()
    if (this.cursors.get(key) === cursor) this.cursors.delete(key)
  }

  private sweepIdleCursors(): void {
    const now = Date.now()
    for (const [k, c] of this.cursors) {
      // 읽는 중인 커서는 건드리지 않는다
      if (!c.reading && now - c.lastUsed > CURSOR_IDLE_MS) this.retire(k, c)
    }
  }

  async dispose(): Promise<void> {
    clearInterval(this.sweeper)
    for (const c of this.cursors.values()) c.dispose()
    this.cursors.clear()
    this.anchors.clear()
    this.commitCache.clear()
    this.diffCache.clear()
    for (const ac of this.requests.values()) ac.abort()
    await Promise.all([...this.runtimes.values()].map((r) => r.exec.dispose()))
    this.runtimes.clear()
  }

  private async withSignal<T>(requestId: string | undefined, fn: (signal?: AbortSignal) => Promise<T>): Promise<T> {
    if (!requestId) return fn()
    const ac = new AbortController()
    this.requests.set(requestId, ac)
    try {
      return await fn(ac.signal)
    } finally {
      if (this.requests.get(requestId) === ac) this.requests.delete(requestId)
    }
  }

  private resolve(repoId: string): { repo: Repo; rt: Runtime } {
    const repo = this.config.repo(repoId)
    if (!repo) throw new Error(`unknown repo: ${repoId}`)
    const source = this.config.source(repo.sourceId)
    if (!source) throw new Error(`unknown source: ${repo.sourceId}`)
    return { repo, rt: this.runtime(source) }
  }

  private runtime(source: Source): Runtime {
    let rt = this.runtimes.get(source.id)
    if (!rt) {
      rt = { exec: createExecutor(source), queue: new ProcessQueue(CONCURRENCY[source.kind]) }
      this.runtimes.set(source.id, rt)
    }
    return rt
  }
}

function createExecutor(source: Source): GitExecutor {
  switch (source.kind) {
    case 'local':
      return new LocalExecutor()
    case 'wsl':
    case 'ssh':
      throw new Error(`${source.kind} executor is not implemented yet (M4)`)
  }
}
