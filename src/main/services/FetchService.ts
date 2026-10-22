import type { FetchStatus, GithubRepoLink } from '@shared/types'
import { mkdirSync } from 'node:fs'
import { AbortError, type GitExecutor } from '../executors/GitExecutor'
import {
  classifyFailure,
  FILE_TRANSPORT_MIN_GIT,
  fetchConfigArgs,
  gitVersion,
  hooksPathFor,
  inspectConfig,
  inspectEntries,
  nextDelayMs,
  runFetch,
  versionAtLeast
} from '../git/fetch'
import { ensureGithubHead, inspectGithubRepo } from '../git/githubCache'
import type { ConfigStore } from './ConfigStore'
import type { RepoService } from './RepoService'

/** 자동 fetch 기본 주기(분). GitKraken 은 1분이지만 서버·배터리에 덜 부담스럽게 5분으로 둔다 */
export const DEFAULT_AUTO_FETCH_MINUTES = 5
/** 저장소를 선택한 뒤 첫 자동 fetch 까지의 지연(선택 직후의 조회들이 먼저 끝나도록) */
const FIRST_DELAY_MS = 3_000
/** fetch 한 번의 제한 시간 */
const FETCH_TIMEOUT_MS = 60_000
/** GitHub 원격 전용 저장소의 첫 fetch(아직 ref 가 없다)는 이력 전체를 받으므로 넉넉히 둔다. 큐 슬롯을 얻은 뒤부터 잰다 */
const GITHUB_FIRST_FETCH_TIMEOUT_MS = 60 * 60_000
/** 저장소를 지우기 전에 진행 중인 fetch 가 끝나기를 기다리는 최대 시간(그 뒤에는 고아 캐시 정리가 안전망이다) */
const FORGET_WAIT_MS = 5_000
const MAX_ERROR_LEN = 300

export interface FetchOptions {
  /** 사용자가 새로고침을 눌렀다 — 위험 설정 확인 창을 띄울 수 있고, 인증 실패·백오프 상태를 풀고 다시 시도한다 */
  manual: boolean
  /**
   * 위험한 저장소 설정을 허용할지 사용자에게 묻는다(manual 일 때만 호출).
   * true 는 이번 실행 동안, 'always' 는 같은 설정이 유지되는 한 이 저장소에서 계속(설정 파일에 지문을 남긴다)
   */
  confirm?: (repoId: string, items: { key: string; value: string }[]) => Promise<boolean | 'always'>
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
  /** 이번 실행에서 사용자가 허용한 위험 설정의 지문("항상 허용"은 ConfigStore.trustedConfig 에 따로 남는다) */
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
    if (repoId) void this.fetchIfEmpty(repoId)
  }

  /**
   * 아직 한 번도 받지 못한 GitHub 원격 전용 저장소(ref 가 없다)는 보여 줄 것이 없으므로 선택 즉시 fetch 한다
   * (일반 저장소의 첫 자동 fetch 지연 FIRST_DELAY_MS 는 선택 직후의 조회들이 먼저 끝나게 하려는 것이다)
   */
  private async fetchIfEmpty(repoId: string): Promise<void> {
    try {
      if (!this.repos.githubTarget(repoId)) return
      const s = this.state(repoId)
      if (s.blocked || s.authPaused || this.inflight.has(repoId) || !(await this.repos.hasNoRefs(repoId))) return
      if (this.active === repoId) await this.fetch(repoId, { manual: false })
    } catch {
      /* 저장소가 사라졌거나 읽지 못하면 다음 자동 fetch 에 맡긴다 */
    }
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

  /**
   * 저장소를 지우기 전: 진행 중인 fetch 를 끊고 끝날 때까지 기다린다(캐시 폴더를 지우기 전에 git 이 손을 떼야 한다).
   * 끊어도 끝나지 않는 fetch 가 삭제를 붙잡지 않도록 FORGET_WAIT_MS 까지만 기다린다.
   */
  async forgetAndWait(repoId: string): Promise<void> {
    const running = this.inflight.get(repoId)
    this.forget(repoId)
    if (!running) return
    let timer: ReturnType<typeof setTimeout> | undefined
    await Promise.race([running.then(() => {}, () => {}), new Promise<void>((resolve) => (timer = setTimeout(resolve, FORGET_WAIT_MS)))])
    clearTimeout(timer)
  }

  /**
   * GitHub 로그인 상태가 바뀌었다(로그인·로그아웃·토큰 만료): 인증 문제로 멈춰 있던 원격 전용 저장소의 자동 fetch 를 다시 연다.
   * 저장소를 열고 있다면 아직 받은 것이 없는 경우에 바로 다시 시도한다.
   */
  resumeGithubAuth(): void {
    for (const [repoId, s] of this.states) {
      if (!s.authPaused || !this.repos.isGithub(repoId)) continue
      s.authPaused = false
      s.failures = 0
      s.needsManual = false
      this.emit(repoId)
    }
    this.schedule()
    if (this.active) void this.fetchIfEmpty(this.active)
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

  /** 위험 설정 지문을 사용자가 허용했는가(이번 실행 동안, 또는 이 저장소에서 항상) */
  private accepted(repoId: string, fingerprint: string): boolean {
    return this.state(repoId).accepted === fingerprint || this.config.trustedConfig(repoId) === fingerprint
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
    // 소스가 github 면 레코드가 어긋나 있어도(github === null) 일반 저장소 흐름으로 가지 않는다 — runGithub 가 막는다
    const isGithub = this.repos.isGithub(repoId)
    const github = isGithub ? this.repos.githubTarget(repoId) : null
    const timeoutMs = github && (await this.repos.hasNoRefs(repoId).catch(() => false)) ? GITHUB_FIRST_FETCH_TIMEOUT_MS : FETCH_TIMEOUT_MS
    let timedOut = false
    let timeout: ReturnType<typeof setTimeout> | undefined
    const arm = (): void => {
      timeout = setTimeout(() => {
        timedOut = true
        ac.abort()
      }, timeoutMs)
    }
    // 일반 저장소는 요청 때부터 잰다. GitHub 원격 전용 저장소의 첫 fetch 는 이력 전체를 받는 긴 작업이라, 다른 fetch 를 기다린
    // 시간이 제한을 갉아먹지 않도록 큐 슬롯을 얻은 뒤부터 잰다
    if (!isGithub) arm()
    const signal = ac.signal
    let approvedForRun = true
    let hasHttp = false
    s.needsManual = false
    try {
      const { exec, path, queue } = this.repos.fetchContext(repoId)
      const changed = await queue.run(
        async () => {
          if (isGithub) {
            arm()
            return this.runGithub(repoId, github, exec, path, s, signal)
          }
          const ins = await inspectConfig(exec, path, { signal })
          s.noRemote = !ins.hasRemote
          s.blocked = null
          if (s.noRemote) return false
          if (ins.forbidden.length > 0) {
            // remote URL 은 허용 여부를 묻지 않고 거부한다
            s.blocked = { keys: ins.forbidden.map((f) => `${f.key} (${f.reason})`), confirmable: false }
            return false
          }
          if (ins.dangerous.length > 0 && !this.accepted(repoId, ins.fingerprint)) {
            const answer = opts.manual && opts.confirm ? await opts.confirm(repoId, ins.dangerous) : false
            if (!answer) {
              s.blocked = { keys: ins.dangerous.map((d) => d.key), confirmable: true }
              return false
            }
            s.accepted = ins.fingerprint
            // 확인 창이 떠 있는 동안 저장소를 지웠으면 남기지 않는다. 저장에 실패해도 이번 실행 동안의 허용으로 계속한다
            if (answer === 'always' && this.config.repo(repoId)) {
              try {
                this.config.setTrustedConfig(repoId, ins.fingerprint)
              } catch {
                /* 다음 실행에서 다시 묻는다 */
              }
            }
          }
          // 검사와 실행 사이에 설정이 바뀌었을 수 있다(TOCTOU) — 실행 직전에 다시 검사해 달라졌으면 멈춘다
          const again = await inspectConfig(exec, path, { signal })
          if (again.forbidden.length > 0) {
            s.blocked = { keys: again.forbidden.map((f) => `${f.key} (${f.reason})`), confirmable: false }
            return false
          }
          if (again.dangerous.length > 0 && !this.accepted(repoId, again.fingerprint)) {
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
        s.error = this.repos.redactSecrets(timedOut ? `시간 초과(${timeoutMs / 1000}초)` : errMessage(err)).slice(0, MAX_ERROR_LEN)
        const kind = timedOut ? 'network' : classifyFailure(err, { github: isGithub })
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

  /**
   * GitHub 원격 전용 저장소의 fetch. 일반 저장소와 달리 허용 목록·확인 창을 거치지 않고, 설정이 앱이 만든 것과 정확히 같을 때만
   * 자동으로 받는다(다르면 확인 없이 차단). 자격 증명은 githubEnv 의 Authorization 헤더로만 가고 자격 증명 도우미(GCM)는 쓰지 않는다.
   * 검사와 fetch 사이에 확인 창 같은 대기가 없으므로 검사 직후 바로 실행한다.
   */
  private async runGithub(
    repoId: string,
    github: { link: GithubRepoLink; path: string } | null,
    exec: GitExecutor,
    path: string,
    s: State,
    signal: AbortSignal
  ): Promise<boolean> {
    s.noRemote = false
    s.blocked = null
    if (!github) {
      s.blocked = { keys: ['github (앱 캐시 경로가 저장소와 맞지 않음)'], confirmable: false }
      return false
    }
    const problems = await inspectGithubRepo(exec, path, github.link, { signal })
    if (problems.length > 0) {
      s.blocked = { keys: problems, confirmable: false }
      return false
    }
    await this.repos.githubReady()
    const env = this.repos.githubFetchEnv(path)
    const hooksPath = hooksPathFor(exec.kind, this.noHooksDir)
    const before = await this.repos.signature(repoId, signal).catch(() => null)
    await runFetch(exec, path, { ...inspectEntries([]), remoteNames: ['origin'] }, { signal, hooksPath, allowFile: false, credentialHelpers: false, env, filter: 'blob:none', followRedirects: 'false' })
    // HEAD 를 받아 온 기본 브랜치에 맞춘다. 실패해도 fetch 자체는 성공이다(취소만 그대로 올린다)
    try {
      const branch = await ensureGithubHead(exec, path, github.link, { signal, env }, fetchConfigArgs(hooksPath, { followRedirects: 'false' }))
      if (branch && branch !== github.link.defaultBranch) this.config.setGithubDefaultBranch(repoId, branch)
    } catch (err) {
      if (err instanceof AbortError) throw err
    }
    const after = await this.repos.signature(repoId, signal).catch(() => null)
    s.lastOkAt = Date.now()
    return before !== after
  }
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
