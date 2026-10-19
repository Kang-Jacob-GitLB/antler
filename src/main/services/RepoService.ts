import type {
  AddRepoInput,
  CommitActivity,
  RemoteDirListing,
  CommitDetail,
  CompareResult,
  RefsDetails,
  FileDiff,
  FileDiffRequest,
  FileHistoryPage,
  FileHistoryRequest,
  BlameRequest,
  BlameResult,
  GhRepoRef,
  LogPage,
  LogQuery,
  LfsPathsRequest,
  RepoExtras,
  RefsSnapshot,
  Repo,
  SearchResult,
  Source,
  SourceKind,
  WipDiffRequest,
  WipStatus
} from '@shared/types'
import { AbortError, type GitExecutor } from '../executors/GitExecutor'
import { LocalExecutor } from '../executors/LocalExecutor'
import { ProcessQueue } from '../executors/ProcessQueue'
import { SshExecutor } from '../executors/SshExecutor'
import { WslExecutor } from '../executors/WslExecutor'
import { wslFromUnc } from '../executors/wslPaths'
import { commitActivity } from '../git/activity'
import * as git from '../git/commands'
import * as fileInsight from '../git/fileInsight'
import { LogCursor } from '../git/logCursor'
import * as extras from '../git/repoExtras'
import * as wip from '../git/wip'
import { pickGithubRemote } from '../github/remoteUrl'
import { LOCAL_SOURCE_ID, sourceFor, type ConfigStore } from './ConfigStore'
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
/** 연결 테스트 제한 시간(SSH 접속·WSL 배포판 기동 포함) */
const ENV_TEST_TIMEOUT_MS = 25_000
/** 커밋 활동 캐시 항목 수 */
const ACTIVITY_CACHE_ENTRIES = 8
/**
 * 저장소를 활성화한 뒤 첫 로그 페이지가 전달되기를 백그라운드 조회가 기다리는 최대 시간.
 * 첫 화면 전에 git 프로세스(Windows 에서는 spawn 한 번이 메인 스레드를 8~15ms 막는다)가 몰리지 않게 하는 문이며,
 * 로그가 오지 않는 경우(오류·빈 저장소)에도 조회가 영영 막히지 않도록 시간 제한을 둔다.
 */
const FIRST_PAGE_GRACE_MS = 1_500
/** 공유 시그니처 계산(감시 기준·활동 조회)의 제한 시간. 멈춘 원격이 큐 슬롯과 감시를 붙잡지 않게 한다 */
const SIGNATURE_TIMEOUT_MS = 20_000
/** GitHub remote 추론 결과 캐시 시간 */
const GH_REPO_TTL_MS = 60_000

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

/** 비교 결과가 차지하는 메모리 추정치(바이트) */
export function estimateCompareBytes(d: CompareResult): number {
  let n = 256
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
  /** fetch 전용 슬롯 하나. 최대 60초 걸리는 네트워크 작업이 조회용 큐를 점유하지 않고, 환경당 동시 fetch 도 하나로 제한한다 */
  fetchQueue: ProcessQueue
}

/**
 * 저장소 단위 git 조회의 진입점. 소스별 executor·큐를 소유하고, 요청 취소(requestId)를 관리한다.
 * 커밋 목록은 ref 이동으로 바뀔 수 있어 캐시하지 않고 LogCursor 로 이어 읽는다
 * (커밋 해시 기반 상세/diff 는 불변이라 LRU 로 캐시한다).
 */
export class RepoService {
  private readonly runtimes = new Map<string, Runtime>()
  /** 아직 저장하지 않은 원격 환경 런타임의 참조 횟수(holdRuntime/dropRuntime) */
  private readonly transient = new Map<string, number>()
  private readonly requests = new Map<string, AbortController>()
  private readonly cursors = new Map<string, LogCursor>()
  /** 닫힌 커서의 마지막 위치. 다음 페이지가 이어지는지 검증하는 데 쓴다 */
  private readonly anchors = new Map<string, { position: number; hash: string }>()
  /** 커밋 해시 기준이라 불변이다 — 무효화하지 않는다 */
  private readonly commitCache = new LruCache<string, CommitDetail>(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES)
  private readonly compareCache = new LruCache<string, CompareResult>(COMMIT_CACHE_ENTRIES, COMMIT_CACHE_BYTES)
  private readonly diffCache = new LruCache<string, FileDiff>(DIFF_CACHE_ENTRIES, DIFF_CACHE_BYTES)
  private readonly inflight = new Map<string, Inflight<unknown>>()
  /** 커밋 활동 집계 캐시: 저장소+ref 별로 ref 시그니처와 함께 보관한다(시그니처가 같으면 이력이 같다) */
  private readonly activityCache = new Map<string, { sig: string; value: CommitActivity }>()
  private readonly ghRepoCache = new Map<string, { at: number; value: GhRepoRef | null }>()
  /** 활성화됐지만 첫 로그 페이지가 아직 안 나간 저장소의 문(열리면 목록에서 빠진다) */
  private readonly firstPageGates = new Map<string, { promise: Promise<void>; open: () => void }>()
  /** 가장 최근에 계산한 ref 시그니처(활성 저장소의 변경 감시·활동 캐시가 같은 값을 쓴다) */
  private readonly knownSigs = new Map<string, string>()
  /** 공유 시그니처 계산의 진행 중 요청(감시의 기준 계산과 활동 조회가 합쳐진다. 자체 제한 시간이 있다) */
  private readonly sigInflight = new Map<string, Promise<string>>()
  /** 시그니처 계산 시작 순번. 늦게 끝난 오래된 계산이 더 새로운 값을 덮어쓰지 못하게 한다 */
  private sigCounter = 0
  private readonly sigApplied = new Map<string, number>()
  private readonly sweeper = setInterval(() => this.sweepIdleCursors(), CURSOR_SWEEP_MS)

  constructor(private readonly config: ConfigStore) {
    this.sweeper.unref?.()
  }

  /**
   * 환경과 경로로 저장소를 추가한다. 그 환경 안에서 git 최상위 경로를 확인해 저장하므로 하위 폴더를 골라도 된다.
   * 로컬로 고른 경로가 \\wsl.localhost\… 같은 WSL 공유 경로면 WSL 저장소로 바꿔 등록한다(WSL 안에서 git 을 돌려야
   * 9P 파일 공유를 거치지 않아 빠르고 줄바꿈·권한이 정확하다).
   */
  async addRepo(input: AddRepoInput): Promise<Repo> {
    let { kind, path } = input
    let target = kind === 'wsl' ? input.distro : kind === 'ssh' ? input.sshHost?.trim() : undefined
    path = path.trim()
    if (kind === 'local' || kind === 'wsl') {
      const unc = wslFromUnc(path)
      if (unc) {
        kind = 'wsl'
        target = unc.distro
        path = unc.path
      }
    }
    if (!path) throw new Error('경로를 입력하세요')
    const source = sourceFor(kind, target)
    // 처음 써 보는 원격 환경이 실패하면 연결을 남기지 않는다(성공하면 소스가 저장돼 계속 쓴다)
    const rt = this.holdRuntime(source)
    try {
      const top = await rt.queue.run(() => git.resolveTopLevel(rt.exec, remoteRelative(kind, path)))
      // Windows git 은 C:/a/b 형식으로 돌려주므로 사용자가 고른 표기(역슬래시)로 맞춘다
      const normalized = kind === 'local' && process.platform === 'win32' ? top.replace(/\//g, '\\') : top
      const name = normalized.split(/[\\/]/).filter(Boolean).pop() ?? normalized
      const group = input.group?.trim()
      const repo = this.config.addRepo({ sourceId: source.id, path: normalized, name, ...(group ? { group } : {}) }, source)
      // 이미 등록된 저장소를 다시 추가하면서 그룹을 골랐으면 그 그룹으로 옮긴다(조용히 버리지 않는다)
      if (group && repo.group !== group) this.config.updateRepo(repo.id, { group })
      return this.config.repo(repo.id) ?? repo
    } finally {
      this.dropRuntime(source.id)
    }
  }

  /** 연결 테스트: 그 환경에서 git 버전을 돌려준다(SSH 인증·호스트 키·WSL 배포판 문제를 저장 전에 드러낸다) */
  async testEnv(input: Omit<AddRepoInput, 'path' | 'group'>): Promise<string> {
    const target = input.kind === 'wsl' ? input.distro : input.kind === 'ssh' ? input.sshHost?.trim() : undefined
    const source = sourceFor(input.kind, target)
    const rt = this.holdRuntime(source)
    const timeout = AbortSignal.timeout(ENV_TEST_TIMEOUT_MS)
    try {
      const r = await rt.queue.run(() => rt.exec.run('.', ['--version'], { signal: timeout }))
      if (r.code !== 0) throw new Error(r.stderr.trim() || `git 실행 실패(${r.code})`)
      return r.stdout.toString('utf8').trim()
    } catch (err) {
      // 시간 초과는 "취소"가 아니라 응답이 없었던 것이다 — 그대로 보여 준다
      if (timeout.aborted && err instanceof AbortError) {
        throw new Error(`${ENV_TEST_TIMEOUT_MS / 1000}초 안에 응답이 없습니다(네트워크·방화벽·WSL 상태를 확인하세요)`)
      }
      throw err
    } finally {
      this.dropRuntime(source.id)
    }
  }

  /** SSH 원격 폴더 목록(저장소 추가 창의 폴더 탐색). 연결 테스트처럼 저장 전 환경의 런타임을 잠시 쓴다 */
  async listRemoteDir(sshHost: string, path: string): Promise<RemoteDirListing> {
    const source = sourceFor('ssh', sshHost)
    const rt = this.holdRuntime(source)
    const timeout = AbortSignal.timeout(ENV_TEST_TIMEOUT_MS)
    try {
      if (!(rt.exec instanceof SshExecutor)) throw new Error('SSH 환경이 아닙니다')
      return await rt.exec.listDirs(path, timeout)
    } catch (err) {
      if (timeout.aborted && err instanceof AbortError) {
        throw new Error(`${ENV_TEST_TIMEOUT_MS / 1000}초 안에 응답이 없습니다(네트워크·방화벽 상태를 확인하세요)`)
      }
      throw err
    } finally {
      this.dropRuntime(source.id)
    }
  }

  /**
   * 아직 저장하지 않은 원격 환경(연결 테스트·추가 중)의 런타임을 잡아 둔다. 여러 요청이 동시에 같은 새 환경을 쓰면
   * 먼저 끝난 쪽이 연결을 닫아 다른 쪽을 끊지 않도록 참조 횟수로 관리한다.
   */
  private holdRuntime(source: Source): Runtime {
    this.transient.set(source.id, (this.transient.get(source.id) ?? 0) + 1)
    return this.runtime(source)
  }

  /** 잡아 둔 런타임을 놓는다. 마지막 참조이고 그사이 저장되지도 않았으면 연결을 닫는다 */
  private dropRuntime(sourceId: string): void {
    const n = (this.transient.get(sourceId) ?? 1) - 1
    if (n > 0) {
      this.transient.set(sourceId, n)
      return
    }
    this.transient.delete(sourceId)
    if (sourceId !== LOCAL_SOURCE_ID && !this.config.source(sourceId)) void this.releaseRuntime(sourceId)
  }

  /**
   * 저장소가 활성화됐다 — 첫 로그 페이지가 전달될 때(또는 FIRST_PAGE_GRACE_MS 뒤)까지 afterFirstPage 가 기다리게 한다.
   * 다시 활성화된 저장소의 이전 시그니처는 낡았을 수 있어 버린다.
   */
  armFirstPage(repoId: string): void {
    this.invalidateSignature(repoId)
    if (this.firstPageGates.has(repoId)) return
    let open!: () => void
    const promise = new Promise<void>((resolve) => (open = resolve))
    const timer = setTimeout(() => this.openFirstPage(repoId), FIRST_PAGE_GRACE_MS)
    timer.unref?.()
    this.firstPageGates.set(repoId, {
      promise,
      open: () => {
        clearTimeout(timer)
        open()
      }
    })
  }

  /** 첫 화면에 필요 없는 백그라운드 조회(활동·WIP·GitHub·감시 기준 등)는 이것을 기다린 뒤 git 을 띄운다 */
  afterFirstPage(repoId: string): Promise<void> {
    return this.firstPageGates.get(repoId)?.promise ?? Promise.resolve()
  }

  private openFirstPage(repoId: string): void {
    const gate = this.firstPageGates.get(repoId)
    if (!gate) return
    this.firstPageGates.delete(repoId)
    gate.open()
  }

  /**
   * 변경 감지용 시그니처: HEAD 와 모든 ref 가 가리키는 해시를 요약한 값.
   * 계산한 값은 기억해 두고(activity 가 재사용), signal 이 없는 요청은 진행 중인 계산과 합친다(계산 자체에 제한 시간이 있다).
   * 시간 제한(signal)을 거는 호출(감시 확인·fetch 전후 비교)은 합치지 않고 따로 계산한다 — 진행 중이던 계산이 ref 변경 전
   * 값일 수 있어 합치면 변화를 놓친다.
   */
  signature(repoId: string, signal?: AbortSignal): Promise<string> {
    return signal ? this.computeSignature(repoId, signal) : this.sharedSignature(repoId)
  }

  private computeSignature(repoId: string, signal: AbortSignal): Promise<string> {
    const { repo, rt } = this.resolve(repoId)
    const seq = ++this.sigCounter
    const opts = { signal }
    return rt.queue.run(() => git.refsSignature(rt.exec, repo.path, opts), opts).then((sig) => {
      // 더 나중에 시작한 계산(또는 무효화)이 이미 반영됐으면 이 값은 낡았다
      if (seq > (this.sigApplied.get(repoId) ?? 0)) {
        this.sigApplied.set(repoId, seq)
        this.knownSigs.set(repoId, sig)
      }
      return sig
    })
  }

  /** 진행 중인 계산에 합류한다. 호출자의 signal 은 기다림만 끊는다(계산은 자체 제한 시간까지 간다) */
  private sharedSignature(repoId: string, signal?: AbortSignal): Promise<string> {
    let p = this.sigInflight.get(repoId)
    if (!p) {
      const created: Promise<string> = this.computeSignature(repoId, AbortSignal.timeout(SIGNATURE_TIMEOUT_MS)).finally(() => {
        if (this.sigInflight.get(repoId) === created) this.sigInflight.delete(repoId)
      })
      this.sigInflight.set(repoId, created)
      p = created
    }
    if (!signal) return p
    const shared = p
    return new Promise<string>((resolve, reject) => {
      if (signal.aborted) return reject(new AbortError())
      const onAbort = (): void => reject(new AbortError())
      signal.addEventListener('abort', onAbort, { once: true })
      shared.then(
        (v) => {
          signal.removeEventListener('abort', onAbort)
          resolve(v)
        },
        (err) => {
          signal.removeEventListener('abort', onAbort)
          reject(err)
        }
      )
    })
  }

  /** 기억한 시그니처를 버리고, 그 전에 시작한 계산이 다시 기록하지 못하게 한다(이력이 바뀐 것을 알았을 때) */
  private invalidateSignature(repoId: string): void {
    this.knownSigs.delete(repoId)
    this.sigInflight.delete(repoId)
    this.sigApplied.set(repoId, ++this.sigCounter)
  }

  /** 로컬 저장소의 .git 디렉터리들(워크트리는 git-dir 과 공통 디렉터리가 다르다). 원격 환경이면 null */
  async localGitDirs(repoId: string): Promise<string[] | null> {
    const { repo, rt } = this.resolve(repoId)
    if (rt.exec.kind !== 'local') return null
    return rt.queue.run(() => git.gitDirs(rt.exec, repo.path))
  }

  /** FetchService 용: 저장소 경로와 실행기, fetch 전용 큐 */
  fetchContext(repoId: string): { exec: GitExecutor; path: string; queue: ProcessQueue } {
    const { repo, rt } = this.resolve(repoId)
    return { exec: rt.exec, path: repo.path, queue: rt.fetchQueue }
  }

  repoKind(repoId: string): SourceKind | null {
    const repo = this.config.repo(repoId)
    return repo ? (this.config.source(repo.sourceId)?.kind ?? null) : null
  }

  /** 저장소를 지운 뒤 더 이상 쓰지 않는 환경의 연결(SSH 세션 등)을 닫는다 */
  async releaseUnused(): Promise<void> {
    const used = new Set(this.config.get().repos.map((r) => r.sourceId))
    await Promise.all([...this.runtimes.keys()].filter((id) => id !== LOCAL_SOURCE_ID && !used.has(id)).map((id) => this.releaseRuntime(id)))
  }

  private async releaseRuntime(sourceId: string): Promise<void> {
    const rt = this.runtimes.get(sourceId)
    if (!rt) return
    this.runtimes.delete(sourceId)
    await rt.exec.dispose().catch(() => {})
  }

  /** 사이드바 ref 스냅샷(첫 화면용: 업스트림 ahead/behind·서브모듈은 refsDetails 로 따로 받는다) */
  refs(repoId: string): Promise<RefsSnapshot> {
    const { repo, rt } = this.resolve(repoId)
    return rt.queue.run(() => git.refs(rt.exec, repo.path, undefined, true), { key: `refs:${repoId}` })
  }

  /** refs 에서 미룬 부가 정보. 비싼 조회이므로 첫 로그 페이지가 나간 뒤에 시작한다 */
  async refsDetails(repoId: string): Promise<RefsDetails> {
    await this.afterFirstPage(repoId)
    const { repo, rt } = this.resolve(repoId)
    return rt.queue.run(() => git.refsDetails(rt.exec, repo.path), { key: `refsDetails:${repoId}` })
  }

  /**
   * remote URL 에서 추론한 GitHub 저장소(GitHub 저장소가 아니면 null). remote 는 거의 안 바뀌므로
   * 저장소별로 짧게(60초) 캐시한다 — 목록·상세 조회마다 git 을 띄우지 않기 위해서다.
   */
  async ghRepo(repoId: string): Promise<GhRepoRef | null> {
    const hit = this.ghRepoCache.get(repoId)
    if (hit && Date.now() - hit.at < GH_REPO_TTL_MS) return hit.value
    await this.afterFirstPage(repoId)
    const { repo, rt } = this.resolve(repoId)
    const list = await rt.queue.run(() => git.remotes(rt.exec, repo.path), { key: `remotes:${repoId}` })
    const value = pickGithubRemote(list)
    this.ghRepoCache.set(repoId, { at: Date.now(), value })
    return value
  }

  /**
   * 아바타 조회용: 그 작성자의 push 된(원격 추적 ref 에서 닿는) 커밋 sha 몇 개. 로컬 브랜치에만 있는 커밋은
   * GitHub 가 몰라 404 이므로, 같은 작성자의 다른 커밋으로 다시 묻는 데 쓴다. 이메일은 로컬 git 인자로만 쓴다(-F: 정규식 아님).
   */
  async pushedShasByAuthor(repoId: string, email: string, limit: number, signal?: AbortSignal): Promise<string[]> {
    const { repo, rt } = this.resolve(repoId)
    const r = await rt.queue.run(
      () => rt.exec.run(repo.path, ['log', '--remotes', `--max-count=${limit}`, '--format=%H', '-F', '-i', `--author=<${email}>`], { signal }),
      { signal }
    )
    if (r.code !== 0) return []
    return r.stdout
      .toString('utf8')
      .split('\n')
      .map((s) => s.trim().toLowerCase())
      .filter((s) => /^[0-9a-f]{40}([0-9a-f]{24})?$/.test(s))
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

  /**
   * 커밋 활동 미니맵 데이터. 전체 이력을 한 번 훑으므로 ref 시그니처(HEAD·모든 ref 의 해시 요약)가 같으면
   * 캐시를 돌려주고, 같은 키의 진행 중 요청은 공유한다. 취소 가능(큐 슬롯을 오래 쥐지 않게 renderer 가 취소한다).
   */
  activity(repoId: string, ref: string | undefined, requestId?: string): Promise<CommitActivity> {
    if (ref !== undefined && typeof ref !== 'string') throw new Error('잘못된 ref 입니다')
    const { repo, rt } = this.resolve(repoId)
    const key = `${repoId}\0${ref ?? ''}`
    return this.shared(`activity\0${key}`, requestId, async (signal) => {
      const opts = { signal }
      await this.afterFirstPage(repoId)
      // 감시가 이미 계산한 시그니처를 쓴다(for-each-ref 전체를 한 번 더 돌리지 않는다). 없으면 진행 중인 계산에 합류한다
      const sig = this.knownSigs.get(repoId) ?? (await this.sharedSignature(repoId, signal))
      const hit = this.activityCache.get(key)
      if (hit && hit.sig === sig) return hit.value
      const value = await rt.queue.run(() => commitActivity(rt.exec, repo.path, ref, opts), opts)
      this.activityCache.delete(key)
      this.activityCache.set(key, { sig, value })
      // 저장소·필터를 오가도 메모리가 쌓이지 않게 오래된 것부터 버린다
      if (this.activityCache.size > ACTIVITY_CACHE_ENTRIES) this.activityCache.delete(this.activityCache.keys().next().value!)
      return value
    })
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

  /** 두 커밋 사이의 변경 파일 목록. 해시 쌍 기준이라 불변이므로 캐시하고 진행 중 요청은 공유한다 */
  compare(repoId: string, from: string, to: string, requestId?: string): Promise<CompareResult> {
    const { repo, rt } = this.resolve(repoId)
    const key = `compare\0${repoId}\0${String(from).toLowerCase()}\0${String(to).toLowerCase()}`
    const cached = this.compareCache.get(key)
    if (cached) return Promise.resolve(cached)
    return this.shared(key, requestId, async (signal) => {
      const result = await rt.queue.run(() => git.compareCommits(rt.exec, repo.path, from, to, { signal }), { signal })
      this.compareCache.set(key, result, estimateCompareBytes(result))
      return result
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

  /** 파일 하나를 바꾼 커밋 목록(페이지 단위). HEAD 가 움직일 수 있어 캐시하지 않는다 */
  fileHistory(repoId: string, req: FileHistoryRequest, requestId?: string): Promise<FileHistoryPage> {
    const { repo, rt } = this.resolve(repoId)
    return this.withSignal(requestId, (signal) =>
      rt.queue.run(() => fileInsight.fileHistory(rt.exec, repo.path, req, signal ? { signal } : {}), signal ? { signal } : {})
    )
  }

  /** 지정 커밋 시점 파일의 blame */
  blame(repoId: string, req: BlameRequest, requestId?: string): Promise<BlameResult> {
    const { repo, rt } = this.resolve(repoId)
    return this.withSignal(requestId, (signal) =>
      rt.queue.run(() => fileInsight.fileBlame(rt.exec, repo.path, req, signal ? { signal } : {}), signal ? { signal } : {})
    )
  }

  /** LFS 사용 여부·gitflow 설정(표시 전용). 설정이 바뀔 수 있어 캐시하지 않는다 */
  repoExtras(repoId: string): Promise<RepoExtras> {
    const { repo, rt } = this.resolve(repoId)
    return rt.queue.run(() => extras.repoExtras(rt.exec, repo.path))
  }

  /** 경로 중 filter=lfs 속성인 것(git check-attr — 필터·LFS 는 실행하지 않는다) */
  lfsPaths(repoId: string, req: LfsPathsRequest, requestId?: string): Promise<string[]> {
    const { repo, rt } = this.resolve(repoId)
    return this.withSignal(requestId, (signal) =>
      rt.queue.run(() => extras.lfsPaths(rt.exec, repo.path, req, signal ? { signal } : {}), signal ? { signal } : {})
    )
  }

  /**
   * 작업 트리 상태(WIP). 파일 저장만으로 바뀌므로 캐시하지 않고, 같은 저장소의 진행 중 요청은 하나로 합친다
   * (shared 는 구독자가 모두 취소했을 때만 git 을 끊는다 — ProcessQueue 의 key 합치기는 signal 을 버려 쓰지 않는다).
   */
  wipStatus(repoId: string, requestId?: string): Promise<WipStatus> {
    const { repo, rt } = this.resolve(repoId)
    return this.shared(`wip\0${repoId}`, requestId, async (signal) => {
      await this.afterFirstPage(repoId)
      return rt.queue.run(() => wip.wipStatus(rt.exec, repo.path, { signal }), { signal })
    })
  }

  /** 작업 트리 파일 하나의 diff. 파일이 계속 바뀔 수 있어 캐시하지 않는다 */
  wipDiff(repoId: string, req: WipDiffRequest, requestId?: string): Promise<FileDiff> {
    const { repo, rt } = this.resolve(repoId)
    return this.withSignal(requestId, (signal) =>
      rt.queue.run(() => wip.wipFileDiff(rt.exec, repo.path, req, signal ? { signal } : {}), signal ? { signal } : {})
    )
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
            // 기억해 둔 시그니처도 낡았다 — 감시가 알아채기 전에 활동 조회가 낡은 캐시를 쓰지 않게 한다
            this.invalidateSignature(repoId)
            throw new StaleLogError()
          }
        }
        return active.read(count)
      })()
      // 첫 페이지가 나가면 백그라운드 조회를 풀어 준다(실패해도 풀어 준다 — 오류 화면에서 다른 조회가 막히면 안 된다)
      if (skip === 0) page.then(() => this.openFirstPage(repoId), () => this.openFirstPage(repoId))
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
    this.activityCache.clear()
    for (const g of this.firstPageGates.values()) g.open()
    this.firstPageGates.clear()
    this.knownSigs.clear()
    this.sigInflight.clear()
    this.sigApplied.clear()
    this.commitCache.clear()
    this.compareCache.clear()
    this.diffCache.clear()
    this.ghRepoCache.clear()
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
      rt = { exec: createExecutor(source), queue: new ProcessQueue(CONCURRENCY[source.kind]), fetchQueue: new ProcessQueue(1) }
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
      if (process.platform !== 'win32') throw new Error('WSL 저장소는 Windows 에서만 열 수 있습니다')
      return new WslExecutor(source.distro!)
    case 'ssh':
      return new SshExecutor(source.sshHost!)
  }
}

/**
 * WSL·SSH 에서 '~/' 로 시작하는 경로는 셸 없이 실행하므로 펼쳐지지 않는다.
 * 두 실행기 모두 홈 디렉터리에서 git 을 실행하므로(WSL --cd ~, SSH exec 기본 위치) 홈 기준 상대 경로로 바꾼다.
 */
function remoteRelative(kind: SourceKind, path: string): string {
  if (kind === 'local') return path
  if (path === '~') return '.'
  return path.startsWith('~/') ? path.slice(2) || '.' : path
}
