import type { FetchStatus } from '@shared/types'
import { mkdirSync } from 'node:fs'
import { AbortError, type GitExecutor } from '../executors/GitExecutor'
import {
  classifyFailure,
  FILE_TRANSPORT_MIN_GIT,
  gitVersion,
  hooksPathFor,
  inspectConfig,
  nextDelayMs,
  runFetch,
  versionAtLeast
} from '../git/fetch'
import type { ConfigStore } from './ConfigStore'
import type { RepoService } from './RepoService'

/** 자동 fetch 기본 주기(분). GitKraken 은 1분이지만 서버·배터리에 덜 부담스럽게 5분으로 둔다 */
export const DEFAULT_AUTO_FETCH_MINUTES = 5
/** 저장소를 선택한 뒤 첫 자동 fetch 까지의 지연(선택 직후의 조회들이 먼저 끝나도록) */
const FIRST_DELAY_MS = 3_000
/** fetch 한 번의 제한 시간 */
const FETCH_TIMEOUT_MS = 60_000
const MAX_ERROR_LEN = 300

export interface FetchOptions {
  /** 사용자가 새로고침을 눌렀다 — 위험 설정 확인 창을 띄울 수 있고, 인증 실패·백오프 상태를 풀고 다시 시도한다 */
  manual: boolean
  /** 위험한 저장소 설정을 이번 세션에서 허용할지 사용자에게 묻는다(manual 일 때만 호출) */
  confirm?: (repoId: string, items: { key: string; value: string }[]) => Promise<boolean>
}

interface State {
  lastOkAt: number | null
  lastAttemptAt: number | null
  error: string | null
  blocked: FetchStatus['blocked']
  authPaused: boolean
  noRemote: boolean
  failures: number
  needsManual: boolean
  fetching: boolean
  /** 이번 세션에서 사용자가 허용한 위험 설정의 지문 */
  accepted: string | null
}

/**
 * 원격 fetch 의 유일한 진입점(이 앱이 저장소에 하는 유일한 쓰기).
 * - 저장소 설정 검사(git/fetch.ts)를 통과해야만 실행한다
 * - 저장소당 하나만 실행(진행 중이면 같은 Promise 를 돌려준다), 환경당 전용 큐 슬롯 하나, 60초 제한, 취소 가능
 * - 활성 저장소만 주기적으로 자동 fetch: 선택 3초 뒤 → N분마다, 창 포커스 시 N분이 지났으면 즉시.
 *   네트워크 실패는 지수 백오프(최대 30분), 인증 실패·설정 차단·remote 없음은 수동 새로고침 전까지 자동 fetch 를 멈춘다.
 */
export class FetchService {
  private readonly states = new Map<string, State>()
  private readonly inflight = new Map<string, Promise<FetchStatus>>()
  private readonly aborts = new Map<string, AbortController>()
  private active: string | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  /** 환경(executor)별 git 버전. 처음 한 번만 읽는다 */
  private readonly versions = new WeakMap<GitExecutor, Promise<[number, number, number] | null>>()

  constructor(
    private readonly config: ConfigStore,
    private readonly repos: RepoService,
    private readonly onStatus: (status: FetchStatus) => void,
    /** fetch 로 ref 가 바뀌었을 때 */
    private readonly onChanged: (repoId: string) => void,
    /**
     * 로컬 git 이 core.hooksPath 로 쓸 앱 소유의 빈 디렉터리(드라이브 문자가 있는 절대 경로). Windows 에서는 필수다.
     * 시작할 때 만들고, fetch 마다 비어 있는지 다시 확인한다
     */
    private readonly noHooksDir: string | null = null
  ) {
    if (noHooksDir) {
      try {
        mkdirSync(noHooksDir, { recursive: true })
      } catch {
        /* 만들지 못하면 fetch 때 검사에서 거부한다 */
      }
    }
  }

  status(repoId: string): FetchStatus {
    const s = this.state(repoId)
    return {
      repoId,
      fetching: s.fetching,
      lastOkAt: s.lastOkAt,
      error: s.error,
      blocked: s.blocked,
      authPaused: s.authPaused,
      needsManual: s.needsManual,
      noRemote: s.noRemote
    }
  }

  /** 자동 fetch 대상(활성 저장소)을 바꾼다 */
  setActive(repoId: string | null): void {
    if (repoId === this.active) return
    this.active = repoId
    this.schedule()
  }

  /** 창이 다시 포커스를 얻었을 때: 마지막 시도가 주기보다 오래됐으면 바로 fetch */
  onFocus(): void {
    const id = this.active
    if (!id || !this.autoEnabled(id) || this.inflight.has(id)) return
    const s = this.state(id)
    // 아직 한 번도 시도하지 않았으면 schedule() 의 첫 지연(FIRST_DELAY_MS)에 맡긴다 — 시작 직후 포커스로 조회와 겹치지 않게
    if (s.lastAttemptAt === null) return
    if (Date.now() - s.lastAttemptAt >= nextDelayMs(this.minutes(), s.failures)) {
      void this.fetch(id, { manual: false })
    }
  }

  /** 자동 fetch 주기 설정이 바뀌었다 */
  onPrefsChanged(): void {
    this.schedule()
  }

  fetch(repoId: string, opts: FetchOptions): Promise<FetchStatus> {
    const existing = this.inflight.get(repoId)
    if (existing) return existing
    const p = this.run(repoId, opts).finally(() => {
      this.inflight.delete(repoId)
      this.aborts.delete(repoId)
      if (repoId === this.active) this.schedule()
    })
    this.inflight.set(repoId, p)
    return p
  }

  cancel(repoId: string): void {
    this.aborts.get(repoId)?.abort()
  }

  /** 저장소를 지웠다 */
  forget(repoId: string): void {
    this.cancel(repoId)
    this.states.delete(repoId)
    if (this.active === repoId) this.setActive(null)
  }

  dispose(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    this.active = null
    for (const ac of this.aborts.values()) ac.abort()
  }

  private version(exec: GitExecutor): Promise<[number, number, number] | null> {
    let v = this.versions.get(exec)
    if (!v) {
      v = gitVersion(exec)
      this.versions.set(exec, v)
    }
    return v
  }

  private minutes(): number {
    return this.config.get().ui.autoFetchMinutes ?? DEFAULT_AUTO_FETCH_MINUTES
  }

  private state(repoId: string): State {
    let s = this.states.get(repoId)
    if (!s) {
      s = {
        lastOkAt: null,
        lastAttemptAt: null,
        error: null,
        blocked: null,
        authPaused: false,
        noRemote: false,
        failures: 0,
        needsManual: false,
        fetching: false,
        accepted: null
      }
      this.states.set(repoId, s)
    }
    return s
  }

  private autoEnabled(repoId: string): boolean {
    const s = this.state(repoId)
    return this.minutes() > 0 && !s.blocked && !s.authPaused && !s.noRemote
  }

  private schedule(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
    const id = this.active
    if (!id || !this.autoEnabled(id)) return
    const s = this.state(id)
    const due = s.lastAttemptAt === null ? 0 : s.lastAttemptAt + nextDelayMs(this.minutes(), s.failures) - Date.now()
    this.timer = setTimeout(() => void this.fetch(id, { manual: false }), Math.max(due, FIRST_DELAY_MS))
    this.timer.unref?.()
  }

  private emit(repoId: string): FetchStatus {
    const status = this.status(repoId)
    this.onStatus(status)
    return status
  }

  private async run(repoId: string, opts: FetchOptions): Promise<FetchStatus> {
    const s = this.state(repoId)
    s.lastAttemptAt = Date.now()
    if (opts.manual) {
      s.authPaused = false
      s.failures = 0
    }
    s.fetching = true
    this.emit(repoId)

    const ac = new AbortController()
    this.aborts.set(repoId, ac)
    let timedOut = false
    const timeout = setTimeout(() => {
      timedOut = true
      ac.abort()
    }, FETCH_TIMEOUT_MS)
    const signal = ac.signal
    let approvedForRun = true
    let hasHttp = false
    s.needsManual = false
    try {
      const { exec, path, queue } = this.repos.fetchContext(repoId)
      const changed = await queue.run(
        async () => {
          const ins = await inspectConfig(exec, path, { signal })
          s.noRemote = !ins.hasRemote
          s.blocked = null
          if (s.noRemote) return false
          if (ins.forbidden.length > 0) {
            // remote URL 은 허용 여부를 묻지 않고 거부한다
            s.blocked = { keys: ins.forbidden.map((f) => `${f.key} (${f.reason})`), confirmable: false }
            return false
          }
          if (ins.dangerous.length > 0 && s.accepted !== ins.fingerprint) {
            if (!opts.manual || !opts.confirm || !(await opts.confirm(repoId, ins.dangerous))) {
              s.blocked = { keys: ins.dangerous.map((d) => d.key), confirmable: true }
              return false
            }
            s.accepted = ins.fingerprint
          }
          // 검사와 실행 사이에 설정이 바뀌었을 수 있다(TOCTOU) — 실행 직전에 다시 검사해 달라졌으면 멈춘다
          const again = await inspectConfig(exec, path, { signal })
          if (again.forbidden.length > 0) {
            s.blocked = { keys: again.forbidden.map((f) => `${f.key} (${f.reason})`), confirmable: false }
            return false
          }
          if (again.dangerous.length > 0 && again.fingerprint !== s.accepted) {
            s.blocked = { keys: again.dangerous.map((d) => d.key), confirmable: true }
            return false
          }
          const hooksPath = hooksPathFor(exec.kind, this.noHooksDir)
          // file 전송은 수동 fetch 에서, 그것도 CVE-2024-32004 가 고쳐진 git 에서만 허용한다
          const allowFile = opts.manual && versionAtLeast(await this.version(exec), FILE_TRANSPORT_MIN_GIT)
          const before = await this.repos.signature(repoId, signal).catch(() => null)
          // 자동 fetch 는 수동 fetch 로 승인된 remote(이름+URL 지문이 같음)에서만 자격 증명 도우미를 쓴다
          const approved = this.config.fetchApproval(repoId) === again.remotesFingerprint
          const credentialHelpers = opts.manual || approved
          approvedForRun = approved
          hasHttp = again.remotes.some((r) => /^https?:/i.test(r.url))
          await runFetch(exec, path, again, { signal, hooksPath, allowFile, credentialHelpers })
          if (opts.manual) this.config.setFetchApproval(repoId, again.remotesFingerprint)
          const after = await this.repos.signature(repoId, signal).catch(() => null)
          s.lastOkAt = Date.now()
          return before !== after
        },
        { signal }
      )
      s.error = null
      s.failures = 0
      s.authPaused = false
      if (changed) this.onChanged(repoId)
    } catch (err) {
      if (err instanceof AbortError && !timedOut) {
        s.error = null // 취소는 오류가 아니다
      } else {
        s.error = (timedOut ? `시간 초과(${FETCH_TIMEOUT_MS / 1000}초)` : errMessage(err)).slice(0, MAX_ERROR_LEN)
        const kind = timedOut ? 'network' : classifyFailure(err)
        if (kind === 'protocol') s.error = `자동 fetch 는 로컬 경로(file) remote 를 쓰지 않습니다. 새로고침으로 직접 fetch 하세요. ${s.error}`.slice(0, MAX_ERROR_LEN)
        if (kind === 'auth' && !opts.manual && !approvedForRun && hasHttp) s.needsManual = true
        if (kind !== 'network') s.authPaused = true
        else s.failures++
      }
    } finally {
      clearTimeout(timeout)
      s.fetching = false
    }
    return this.emit(repoId)
  }
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
