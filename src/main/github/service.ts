// GitHub 연동의 진입점. 토큰·로그인 상태·API 클라이언트를 소유하고 IPC 계층에는 가공된 결과만 돌려준다.
// 토큰은 이 객체 밖으로 나가지 않는다 — 반환값은 GhAuthState(토큰 없음)와 API 응답을 변환한 공유 타입뿐이다.
// 유일한 예외는 gitToken(): 원격 전용 저장소를 git 이 fetch 할 때 헤더로 넘기는 main 내부 용도다.
import type {
  GhAccessibleReposResult,
  GhAuthState,
  GhAvatarQuery,
  GhAvatarResult,
  GhCheckDetail,
  GhComment,
  GhCommitChecksResult,
  GhCommitSummary,
  GhDeviceCode,
  GhIssueDetail,
  GhIssueSummary,
  GhJob,
  GhLabel,
  GhLaunchpadResult,
  GhListQuery,
  GhListResult,
  GhPullDetail,
  GhPullFile,
  GhPullSummary,
  GhRepoRef,
  GhRun,
  GhUser
} from '@shared/types'
import { AbortError } from '../executors/GitExecutor'
import { accessibleLoginRequired, fetchAccessibleRepos } from './accessibleRepos'
import * as api from './api'
import { AvatarResolver, FALLBACK_SHAS } from './avatars'
import { CheckStateCache, fetchCheckDetails, fetchCommitChecks, listJobs, listRuns } from './checks'
import { GitHubApiError, GitHubClient } from './client'
import { GhCliError, type GhCli } from './ghCli'
import { DeviceFlow, MISSING_CLIENT_ID_MESSAGE, type DeviceSession, type SleepFn } from './deviceFlow'
import { fetchLaunchpad, loginRequired } from './launchpad'
import type { TokenMethod, TokenStore } from './tokenStore'
import { parsePat as parsePatInput } from './validate'

export interface GitHubServiceDeps {
  /** 설정(ui.githubClientId 와 Launchpad 가 훑을 저장소 목록만 읽는다) */
  config: { get(): { ui: { githubClientId?: string | undefined }; repos?: { id: string }[] } }
  repos: {
    ghRepo(repoId: string): Promise<GhRepoRef | null>
    /** 저장소의 모든 GitHub remote(Launchpad). 없으면 ghRepo 의 결과 하나로 대신한다 */
    ghRemotes?(repoId: string, signal?: AbortSignal): Promise<GhRepoRef[]>
    /** 아바타 404 대비: 그 작성자의 push 된 커밋 sha(없으면 빈 배열). 없으면 대체 조회를 하지 않는다 */
    pushedShasByAuthor?(repoId: string, email: string, limit: number, signal?: AbortSignal): Promise<string[]>
  }
  store: TokenStore
  fetchImpl?: typeof fetch
  sleep?: SleepFn
  /** 작성자 아바타 캐시 파일 경로(없으면 메모리에만 둔다) */
  avatarCacheFile?: string
  /** GitHub CLI(gh) 연동. 없으면 "CLI 로그인 가져오기"는 제공하지 않는다 */
  cli?: GhCli
  /** 로그인·로그아웃·토큰 폐기로 토큰이 바뀌었을 때(원격 전용 저장소의 인증 실패 정지를 풀 때 쓴다) */
  onAuthChanged?: () => void
}

/** 접근 가능한 저장소 목록 캐시 유효 시간 */
const ACCESSIBLE_TTL_MS = 5 * 60_000

export class GitHubService {
  private token: string | null
  private method: TokenMethod | undefined
  private user: GhUser | null = null
  /** 시작 시 읽은 토큰의 /user 검증(첫 gh:auth 호출에서 한 번, 동시 호출은 합친다) */
  private validating: Promise<void> | null = null
  private device: DeviceSession | null = null
  private readonly requests = new Map<string, AbortController>()
  /** 토큰이 바뀌거나 로그아웃하면 중단되는 조회 세대. 데이터 호출은 이 신호를 자기 신호와 합친다 */
  private epoch = new AbortController()
  private readonly flow: DeviceFlow
  private readonly client: GitHubClient
  private readonly avatarResolver: AvatarResolver
  /** 커밋 체크 요약 캐시(계정이 바뀌면 보이는 범위가 달라지므로 로그인·로그아웃 때 비운다) */
  private readonly checkCache = new CheckStateCache()
  private cliAvailable: boolean | undefined
  /** 접근 가능한 저장소 목록 캐시(계정이 바뀌면 보이는 범위가 달라지므로 로그인·로그아웃 때 비운다) */
  private accessibleCache: { result: GhAccessibleReposResult; at: number } | null = null

  constructor(private readonly deps: GitHubServiceDeps) {
    const stored = deps.store.load()
    this.token = stored?.token ?? null
    this.method = stored?.method
    this.client = new GitHubClient(deps.fetchImpl, () => this.token)
    this.avatarResolver = new AvatarResolver(this.client, deps.avatarCacheFile)
    this.flow = new DeviceFlow(deps.fetchImpl, deps.sleep)
  }

  // ── 인증 ────────────────────────────────────────────

  /** 현재 로그인 상태. 저장된 토큰이 있으면 첫 호출에서 GET /user 로 검증하고 사용자 정보를 캐시한다 */
  async auth(): Promise<GhAuthState> {
    if (this.deps.cli && this.cliAvailable === undefined) this.cliAvailable = await this.deps.cli.available()
    if (this.token && !this.user) await this.validate()
    return this.state()
  }

  private validate(): Promise<void> {
    if (this.validating) return this.validating
    const tok = this.token
    this.validating = (async () => {
      try {
        const { data } = await this.client.get<{ login?: string; avatar_url?: string }>('/user')
        if (this.token === tok) this.user = api.mapUser(data)
      } catch (err) {
        // 거부된 토큰은 쓸모없으니 버려 로그인 화면으로 돌린다. 네트워크 문제 등은 로그인 상태를 유지하고 다음 호출에서 다시 확인한다
        if (err instanceof GitHubApiError && err.code === 'GH_AUTH' && this.token === tok) this.dropToken()
      } finally {
        this.validating = null
      }
    })()
    return this.validating
  }

  private state(): GhAuthState {
    const rate = this.client.rate
    return {
      signedIn: this.token !== null,
      ...(this.token && this.user ? { user: this.user } : {}),
      ...(this.token && this.method ? { method: this.method } : {}),
      ...(rate ? { rateRemaining: rate.remaining, rateLimit: rate.limit, rateReset: rate.reset } : {}),
      ...(this.deps.store.storageUnavailable ? { storageUnavailable: true } : {}),
      ...(this.cliAvailable !== undefined ? { cliAvailable: this.cliAvailable } : {})
    }
  }

  /** 이 토큰으로 GET /user 가 되는지 확인하고 사용자 정보를 돌려준다(저장 전 검증) */
  private async verify(token: string, signal?: AbortSignal): Promise<GhUser> {
    try {
      const { data } = await this.client.get<{ login?: string; avatar_url?: string }>('/user', { token, ...(signal ? { signal } : {}) })
      return api.mapUser(data)
    } catch (err) {
      if (err instanceof GitHubApiError && err.code === 'GH_AUTH') throw new Error('토큰이 올바르지 않거나 만료되었습니다')
      throw err
    }
  }

  /** 진행 중인 조회(목록 보강 등)를 모두 멈추고 새 세대를 연다 */
  private newEpoch(): void {
    this.epoch.abort()
    this.epoch = new AbortController()
  }

  private signIn(token: string, method: TokenMethod, user: GhUser): GhAuthState {
    this.newEpoch()
    this.deps.store.save(token, method)
    this.token = token
    this.method = method
    this.user = user
    // 한도·"로그인 필요" 표시는 비로그인 기준이었으니 비우고 다시 묻게 한다
    this.avatarResolver.resetSession()
    this.checkCache.clear()
    this.accessibleCache = null
    this.deps.onAuthChanged?.()
    return this.state()
  }

  private dropToken(): void {
    this.newEpoch()
    this.deps.store.clear()
    this.token = null
    this.method = undefined
    this.user = null
    this.client.rate = null
    this.client.clearCache()
    this.avatarResolver.resetSession()
    this.checkCache.clear()
    this.accessibleCache = null
    this.deps.onAuthChanged?.()
  }

  /**
   * 현재 로그인 토큰(없으면 null). 원격 전용 저장소(앱 캐시의 bare 저장소)를 github.com 에서 fetch 할 때
   * git 에 헤더로만 넘기는 용도다. renderer·로그로 내보내지 않는다.
   */
  gitToken(): string | null {
    return this.token
  }

  async loginPat(raw: string): Promise<GhAuthState> {
    const token = parsePatInput(raw)
    return this.signIn(token, 'pat', await this.verify(token))
  }

  /**
   * GitHub CLI(gh)의 로그인 토큰을 가져와 로그인한다. 토큰은 이 함수 안에서만 쓰이고 반환값·에러·로그에 실리지 않는다.
   * 검증과 저장은 PAT 로그인과 같다(GET /user 로 확인 → safeStorage 암호화 저장).
   */
  async importCliToken(): Promise<GhAuthState> {
    const cli = this.deps.cli
    if (!cli) throw new Error('이 환경에서는 GitHub CLI 로그인 가져오기를 쓸 수 없습니다')
    let raw: string
    try {
      raw = await cli.token()
    } catch (err) {
      if (err instanceof GhCliError) throw new Error(err.message)
      throw new Error('GitHub CLI 에서 토큰을 읽지 못했습니다')
    }
    let token: string
    try {
      token = parsePatInput(raw)
    } catch {
      throw new Error('GitHub CLI 가 돌려준 토큰 형식이 올바르지 않습니다')
    }
    try {
      return this.signIn(token, 'gh-cli', await this.verify(token))
    } catch (err) {
      if (err instanceof GitHubApiError || err instanceof AbortError) throw err
      // verify() 의 "토큰이 올바르지 않거나 만료" 메시지는 gh 쪽 안내를 덧붙인다
      throw new Error(`${err instanceof Error ? err.message : '로그인에 실패했습니다'} — 터미널에서 \`gh auth login\` 으로 다시 로그인한 뒤 시도하세요`)
    }
  }

  /** Device Flow 시작. 이전에 진행 중이던 흐름은 버린다 */
  async deviceStart(): Promise<GhDeviceCode> {
    const clientId = this.clientId()
    const { code, session } = await this.flow.start(clientId)
    this.device = session
    return code
  }

  /** 사용자가 승인할 때까지 폴링하고 로그인한다. 취소(AbortError)면 세션을 남겨 다시 폴링할 수 있다 */
  devicePoll(requestId: string): Promise<GhAuthState> {
    const session = this.device
    if (!session) return Promise.reject(new Error('진행 중인 로그인이 없습니다. 다시 시작하세요'))
    const clientId = this.clientId()
    return this.withSignal(requestId, async (signal) => {
      let token: string
      try {
        token = await this.flow.poll(clientId, session, signal)
      } catch (err) {
        // 만료·거부·실패한 세션은 다시 쓸 수 없다
        if (!(err instanceof AbortError) && this.device === session) this.device = null
        throw err
      }
      if (this.device === session) this.device = null
      return this.signIn(token, 'device', await this.verify(token, signal))
    })
  }

  private clientId(): string {
    const id = this.deps.config.get().ui.githubClientId?.trim()
    if (!id) throw new Error(MISSING_CLIENT_ID_MESSAGE)
    return id
  }

  logout(): GhAuthState {
    this.device = null
    for (const ac of this.requests.values()) ac.abort()
    this.requests.clear()
    this.dropToken()
    return this.state()
  }

  // ── 조회 ────────────────────────────────────────────

  private async slug(repoId: string): Promise<api.Slug> {
    const ref = await this.deps.repos.ghRepo(repoId)
    if (!ref) throw new Error('이 저장소의 remote 가 GitHub(github.com) 저장소가 아닙니다')
    return { owner: ref.owner, repo: ref.repo }
  }

  /**
   * 데이터 호출 공통 처리: 현재 세대의 신호를 싣고, 토큰이 있는데 401(GH_AUTH)이면
   * GET /user 로 한 번 다시 확인해 그것도 거부되면 토큰을 버린다(세션 중 폐기된 토큰). 원래 에러는 그대로 던진다.
   */
  private async data<T>(repoId: string, fn: (slug: api.Slug, signal: AbortSignal) => Promise<T>): Promise<T> {
    const tok = this.token
    const signal = this.epoch.signal
    try {
      return await fn(await this.slug(repoId), signal)
    } catch (err) {
      if (tok && this.token === tok && err instanceof GitHubApiError && err.code === 'GH_AUTH') await this.validate()
      throw err
    }
  }

  pulls(repoId: string, query: GhListQuery): Promise<GhListResult<GhPullSummary>> {
    return this.data(repoId, (slug, signal) => api.listPulls(this.client, slug, query, signal))
  }

  /** Jira 이슈 키를 언급한 PR */
  pullsForKey(repoId: string, key: string): Promise<GhPullSummary[]> {
    return this.data(repoId, (slug, signal) => api.searchPullsByKey(this.client, slug, key, signal))
  }

  pull(repoId: string, n: number): Promise<GhPullDetail> {
    return this.data(repoId, (slug, signal) => api.getPull(this.client, slug, n, signal))
  }

  pullFiles(repoId: string, n: number): Promise<GhPullFile[]> {
    return this.data(repoId, (slug, signal) => api.getPullFiles(this.client, slug, n, signal))
  }

  pullCommits(repoId: string, n: number): Promise<GhCommitSummary[]> {
    return this.data(repoId, (slug, signal) => api.getPullCommits(this.client, slug, n, signal))
  }

  issues(repoId: string, query: GhListQuery): Promise<GhListResult<GhIssueSummary>> {
    return this.data(repoId, (slug, signal) => api.listIssues(this.client, slug, query, signal))
  }

  issue(repoId: string, n: number): Promise<GhIssueDetail> {
    return this.data(repoId, (slug, signal) => api.getIssue(this.client, slug, n, signal))
  }

  comments(repoId: string, n: number): Promise<GhComment[]> {
    return this.data(repoId, (slug, signal) => api.getComments(this.client, slug, n, signal))
  }

  filters(repoId: string): Promise<{ labels: GhLabel[]; assignees: GhUser[] }> {
    return this.data(repoId, (slug, signal) => api.getFilters(this.client, slug, signal))
  }

  /**
   * 커밋 목록의 체크 요약(GraphQL 일괄). 로그인하지 않았으면 조회하지 않고 available=false 로 알린다
   * (GraphQL 은 비로그인을 받지 않고, 비로그인 REST 한도(시간당 60)로 커밋마다 묻는 것은 감당할 수 없다).
   */
  commitChecks(repoId: string, shas: string[]): Promise<GhCommitChecksResult> {
    if (!this.token) return Promise.resolve({ available: false, states: {} })
    return this.data(repoId, async (slug, signal) => ({ available: true, states: await fetchCommitChecks(this.client, slug, shas, this.checkCache, signal) }))
  }

  commitCheckRuns(repoId: string, sha: string): Promise<GhCheckDetail[]> {
    return this.data(repoId, (slug, signal) => fetchCheckDetails(this.client, slug, sha, signal))
  }

  runs(repoId: string, page: number): Promise<GhListResult<GhRun>> {
    return this.data(repoId, (slug, signal) => listRuns(this.client, slug, page, signal))
  }

  runJobs(repoId: string, runId: number): Promise<GhJob[]> {
    return this.data(repoId, (slug, signal) => listJobs(this.client, slug, runId, signal))
  }

  /**
   * 작성자 아바타 해석. GitHub 저장소가 아니어도 noreply 이메일은 풀린다(그 경우 github:false).
   * 사진은 보조 기능이라 어떤 실패(한도·네트워크·인증)도 던지지 않고 "지금은 모름"으로 돌려준다.
   */
  avatars(repoId: string, requestId: string, authors: GhAvatarQuery[]): Promise<GhAvatarResult> {
    const epoch = this.epoch.signal
    return this.withSignal(requestId, async (own) => {
      const ref = await this.deps.repos.ghRepo(repoId).catch(() => null)
      const slug = ref ? { owner: ref.owner, repo: ref.repo } : null
      const signal = AbortSignal.any([epoch, own])
      // 고른 커밋이 push 안 된 것이라 404 면, 같은 작성자의 push 된 커밋으로 다시 묻는다
      const pushed = this.deps.repos.pushedShasByAuthor?.bind(this.deps.repos)
      const fallback = pushed
        ? (email: string): Promise<string[]> => pushed(repoId, email, FALLBACK_SHAS + 1, signal).catch(() => [])
        : undefined
      const avatars = await this.avatarResolver.resolve(slug, authors, signal, fallback)
      // 풀리지 않은 작성자가 남아 있을 때만 막힌 이유를 알린다(전부 풀렸으면 알릴 것이 없다)
      const unresolved = authors.some((a) => !(a.email in avatars))
      const blocked = unresolved && slug ? this.avatarResolver.blockedFor(slug) : null
      return { github: slug !== null, avatars, ...(blocked ? { blocked } : {}) }
    })
  }

  /**
   * Launchpad: 내 PR·리뷰 요청·할당 이슈. 로그인하지 않았으면 조회하지 않고 needsLogin 으로 알린다.
   * 검색은 사용자 기준으로 보내고, 등록된 모든 저장소(config)의 GitHub remote 와 맞춰 거른다.
   * remote 를 읽지 못한 저장소는 건너뛰고(unresolved), 검색 일부가 실패하면 이유를 errors 에 담아 나머지를 돌려준다.
   */
  launchpad(requestId: string): Promise<GhLaunchpadResult> {
    const epoch = this.epoch.signal
    return this.withSignal(requestId, async (own) => {
      const tok = this.token
      if (!tok) return loginRequired()
      const signal = AbortSignal.any([epoch, own])
      const repos = this.deps.repos
      const ids = (this.deps.config.get().repos ?? []).map((r) => r.id)
      try {
        return await fetchLaunchpad(
          this.client,
          ids,
          async (id, sig) => {
            const refs = repos.ghRemotes ? await repos.ghRemotes(id, sig) : [await repos.ghRepo(id)].filter((r) => r !== null)
            return refs.map((r) => ({ owner: r.owner, repo: r.repo }))
          },
          signal
        )
      } catch (err) {
        if (this.token === tok && err instanceof GitHubApiError && err.code === 'GH_AUTH') await this.validate()
        throw err
      }
    })
  }

  /**
   * 로그인한 사용자가 접근할 수 있는 저장소(본인·협업·소속 org). 로그인하지 않았으면 조회하지 않고 needsLogin 으로 알린다.
   * 결과는 5분 메모리 캐시에 둔다(fresh 면 무시하고 다시 읽는다).
   */
  accessibleRepos(requestId: string, fresh = false): Promise<GhAccessibleReposResult> {
    const epoch = this.epoch.signal
    return this.withSignal(requestId, async (own) => {
      const tok = this.token
      if (!tok) return accessibleLoginRequired()
      const hit = this.accessibleCache
      if (!fresh && hit && Date.now() - hit.at < ACCESSIBLE_TTL_MS) return hit.result
      const signal = AbortSignal.any([epoch, own])
      try {
        const result = await fetchAccessibleRepos(this.client, signal)
        // 읽는 동안 계정이 바뀌었으면 캐시에 넣지 않는다
        if (this.token === tok) this.accessibleCache = { result, at: Date.now() }
        return result
      } catch (err) {
        if (this.token === tok && err instanceof GitHubApiError && err.code === 'GH_AUTH') await this.validate()
        throw err
      }
    })
  }

  // ── 요청 취소 ───────────────────────────────────────

  /** req:cancel 에서 호출 — RepoService 와 requestId 공간을 공유하므로 없는 id 는 무시한다 */
  cancel(requestId: string): void {
    this.requests.get(requestId)?.abort()
    this.requests.delete(requestId)
  }

  dispose(): void {
    this.avatarResolver.flush()
    this.epoch.abort()
    for (const ac of this.requests.values()) ac.abort()
    this.requests.clear()
  }

  private async withSignal<T>(requestId: string, fn: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const ac = new AbortController()
    this.requests.set(requestId, ac)
    try {
      return await fn(ac.signal)
    } finally {
      if (this.requests.get(requestId) === ac) this.requests.delete(requestId)
    }
  }
}
