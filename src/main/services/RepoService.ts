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
/** 이 시간 동안 안 쓰인 LogCursor 는 닫는다 */
const CURSOR_IDLE_MS = 60_000

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
  private readonly sweeper = setInterval(() => this.sweepIdleCursors(), 30_000)

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

    for (const [k, c] of this.cursors) {
      if (k !== key) {
        c.dispose()
        this.cursors.delete(k)
      }
    }
    let cursor = this.cursors.get(key)
    if (!cursor || cursor.position !== skip) {
      cursor?.dispose()
      cursor = new LogCursor(rt.exec, repo.path, (s) => git.logArgs(query.ref, s), skip)
      this.cursors.set(key, cursor)
    }
    const active = cursor
    return this.withSignal(requestId, async (signal) => {
      const page = active.read(count)
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

  private sweepIdleCursors(): void {
    const now = Date.now()
    for (const [k, c] of this.cursors) {
      if (now - c.lastUsed > CURSOR_IDLE_MS) {
        c.dispose()
        this.cursors.delete(k)
      }
    }
  }

  async dispose(): Promise<void> {
    clearInterval(this.sweeper)
    for (const c of this.cursors.values()) c.dispose()
    this.cursors.clear()
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
