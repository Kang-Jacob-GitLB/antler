import { basename } from 'node:path'
import type { HeadInfo, LogPage, LogQuery, Repo, Source, SourceKind } from '@shared/types'
import { AbortError, type GitExecutor } from '../executors/GitExecutor'
import { LocalExecutor } from '../executors/LocalExecutor'
import { ProcessQueue } from '../executors/ProcessQueue'
import * as git from '../git/commands'
import { LogCursor } from '../git/logCursor'
import { LOCAL_SOURCE_ID, type ConfigStore } from './ConfigStore'

/** 환경별 동시 git 프로세스 상한(LogCursor 는 저장소당 하나라 큐 밖에서 돈다) */
const CONCURRENCY: Record<SourceKind, number> = { local: 6, wsl: 4, ssh: 4 }
/**
 * 이 시간 동안 안 쓰인 LogCursor 는 닫는다. 멈춘 git log 는 pack 파일 핸들을 쥐고 있어
 * 사용자의 `git gc`/`repack` 이나 폴더 이동을 막으므로 짧게 둔다.
 */
const CURSOR_IDLE_MS = 5_000
const CURSOR_SWEEP_MS = 2_500

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
 * (커밋 해시 기반 불변 캐시는 M3 상세/diff 에서 쓴다).
 */
export class RepoService {
  private readonly runtimes = new Map<string, Runtime>()
  private readonly requests = new Map<string, AbortController>()
  private readonly cursors = new Map<string, LogCursor>()
  /** 닫힌 커서의 마지막 위치. 다음 페이지가 이어지는지 검증하는 데 쓴다 */
  private readonly anchors = new Map<string, { position: number; hash: string }>()
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

  head(repoId: string): Promise<HeadInfo> {
    const { repo, rt } = this.resolve(repoId)
    return rt.queue.run(() => git.head(rt.exec, repo.path), { key: `head:${repoId}` })
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
