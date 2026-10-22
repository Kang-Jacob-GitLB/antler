// Jira 연동의 진입점(조회 + 이슈 상태 전환). 자격 증명·연결 상태·API 클라이언트를 소유하고 IPC 계층에는 가공된 결과만 돌려준다.
// 토큰은 이 객체 밖으로 나가지 않는다 — 반환값은 JiraAuthState(토큰 없음)와 API 응답을 변환한 공유 타입뿐이다.
import type { JiraAuthState, JiraComment, JiraConnectInput, JiraDevInfo, JiraIssueDetail, JiraSearchResult, JiraTransition, JiraUser, JiraWorklogResult } from '@shared/types'
import type { TokenStore } from '../github/tokenStore'
import * as api from './api'
import { JiraApiError, JiraClient, type JiraCredential, type SleepFn } from './client'
import { allowedJiraUrl, parseConnectInput, parseIssueKey, parseSearchQuery, parseTransitionId, parseUserSearch, parseWorklogQuery, type SiteOptions } from './validate'

/** 자격 증명을 담는 파일 이름(GitHub 토큰 파일과 같은 safeStorage 방식, 파일만 따로) */
export const JIRA_TOKEN_FILE = 'jira-credential.bin'
export const JIRA_META_FILE = 'jira-credential.json'

/** 날짜 필드 목록 조회가 실패했을 때 다시 시도하기까지의 간격 */
const DATE_FIELDS_COOLDOWN_MS = 5 * 60_000

export interface JiraServiceDeps extends SiteOptions {
  store: TokenStore
  fetchImpl?: typeof fetch
  sleep?: SleepFn
}

interface Connection extends JiraCredential {
  displayName: string
}

export class JiraService {
  private conn: Connection | null = null
  private readonly requests = new Map<string, AbortController>()
  /** 연결이 바뀌거나 해제되면 중단되는 조회 세대 */
  private epoch = new AbortController()
  private readonly client: JiraClient
  /** /myself 로 알아낸 내 id. 연결이 바뀌거나 해제되면(epoch 교체) 비운다 */
  private me: JiraUser | null = null
  /** /field 로 찾은 시작·종료 날짜 필드 id(성공만 캐시). 연결이 바뀌거나 해제되면 비운다 */
  private dateFields: Promise<api.DateFieldIds> | null = null
  /** /field 가 실패한 뒤 이 시각까지는 다시 묻지 않는다 */
  private dateFieldsRetryAt = 0

  constructor(private readonly deps: JiraServiceDeps) {
    this.conn = this.load()
    this.client = new JiraClient(deps.fetchImpl, () => this.conn, deps.sleep)
  }

  private load(): Connection | null {
    const stored = this.deps.store.load()
    if (!stored) return null
    try {
      const v = JSON.parse(stored.token) as Partial<Connection>
      if ((v.kind !== 'cloud' && v.kind !== 'server') || typeof v.siteUrl !== 'string' || typeof v.token !== 'string') return null
      // 저장된 값도 다시 검증한다(파일이 바뀌었거나 옛 버전이 쓴 값일 수 있다)
      const p = parseConnectInput({ kind: v.kind, siteUrl: v.siteUrl, email: v.email, token: v.token }, this.deps)
      return { ...p, displayName: typeof v.displayName === 'string' ? v.displayName : '' }
    } catch {
      return null
    }
  }

  auth(): JiraAuthState {
    const c = this.conn
    return {
      connected: c !== null,
      ...(c ? { kind: c.kind, siteUrl: c.siteUrl, ...(c.email ? { email: c.email } : {}), ...(c.displayName ? { displayName: c.displayName } : {}) } : {}),
      ...(this.deps.store.storageUnavailable ? { storageUnavailable: true } : {})
    }
  }

  /** 입력한 자격 증명으로 /myself 가 되는지 확인한 뒤 암호화 저장하고 연결한다 */
  async connect(input: JiraConnectInput): Promise<JiraAuthState> {
    const p = parseConnectInput(input, this.deps)
    const cred: JiraCredential = { kind: p.kind, siteUrl: p.siteUrl, email: p.email, token: p.token }
    let displayName: string
    try {
      displayName = await api.myself(this.client, cred)
    } catch (err) {
      if (err instanceof JiraApiError && err.code === 'JIRA_AUTH') throw new Error('이메일·토큰이 올바르지 않거나 만료되었습니다')
      throw err
    }
    this.newEpoch()
    this.deps.store.save(JSON.stringify({ ...cred, displayName }), 'pat')
    this.conn = { ...cred, displayName }
    return this.auth()
  }

  disconnect(): JiraAuthState {
    for (const ac of this.requests.values()) ac.abort()
    this.requests.clear()
    this.newEpoch()
    this.deps.store.clear()
    this.conn = null
    return this.auth()
  }

  private newEpoch(): void {
    this.epoch.abort()
    this.epoch = new AbortController()
    this.me = null
    this.dateFields = null
    this.dateFieldsRetryAt = 0
  }

  // ── 조회 ────────────────────────────────────────────

  private async data<T>(requestId: string | undefined, fn: (c: JiraCredential, signal: AbortSignal) => Promise<T>): Promise<T> {
    const c = this.conn
    if (!c) throw new JiraApiError('Jira 에 연결되어 있지 않습니다', 'JIRA_AUTH')
    const epoch = this.epoch.signal
    if (!requestId) return fn(c, epoch)
    const ac = new AbortController()
    this.requests.set(requestId, ac)
    try {
      return await fn(c, AbortSignal.any([epoch, ac.signal]))
    } finally {
      if (this.requests.get(requestId) === ac) this.requests.delete(requestId)
    }
  }

  async issue(key: unknown, requestId?: string): Promise<JiraIssueDetail> {
    const k = parseIssueKey(key)
    return this.data(requestId, async (c, signal) => api.getIssue(this.client, c, k, await this.dateFieldIds(c), signal))
  }

  /**
   * 시작·종료 날짜 필드 id. 연결 단위로 캐시하고 동시 호출은 같은 Promise 를 쓴다.
   * 실패해도 던지지 않는다(그 날짜만 생략) — 캐시하지 않고 쿨다운 동안은 다시 묻지 않는다.
   * 개별 조회가 아니라 연결 세대(epoch)의 신호로 호출한다: 한 조회의 취소가 다른 조회에 번지지 않게
   */
  private dateFieldIds(c: JiraCredential): Promise<api.DateFieldIds> {
    if (this.dateFields) return this.dateFields
    if (Date.now() < this.dateFieldsRetryAt) return Promise.resolve(api.NO_DATE_FIELDS)
    const epoch = this.epoch
    const p: Promise<api.DateFieldIds> = api.getDateFieldIds(this.client, c, epoch.signal).catch(() => {
      if (this.epoch === epoch) {
        if (this.dateFields === p) this.dateFields = null
        this.dateFieldsRetryAt = Date.now() + DATE_FIELDS_COOLDOWN_MS
      }
      return api.NO_DATE_FIELDS
    })
    this.dateFields = p
    return p
  }

  /** 이슈에서 실행할 수 있는 상태 전환 목록 */
  async transitions(key: unknown, requestId?: string): Promise<JiraTransition[]> {
    const k = parseIssueKey(key)
    return this.data(requestId, (c, signal) => api.getTransitions(this.client, c, k, signal))
  }

  /** 상태 전환 실행(쓰기). requestId 로 취소하지 않는다 — 도중에 끊으면 적용 여부를 알 수 없게 되므로. 연결 해제 때만 중단된다 */
  async transition(key: unknown, transitionId: unknown): Promise<void> {
    const k = parseIssueKey(key)
    const id = parseTransitionId(transitionId)
    return this.data(undefined, (c) => api.doTransition(this.client, c, k, id))
  }

  async comments(key: unknown, requestId?: string): Promise<JiraComment[]> {
    const k = parseIssueKey(key)
    return this.data(requestId, (c, signal) => api.getComments(this.client, c, k, signal))
  }

  /** 개발 정보(Jira 의 GitHub 연동이 모은 PR·커밋). 연동이 없으면 available:false */
  async devInfo(key: unknown, requestId?: string): Promise<JiraDevInfo> {
    const k = parseIssueKey(key)
    return this.data(requestId, (c, signal) => api.getDevInfo(this.client, c, k, signal))
  }

  async search(query: unknown, requestId?: string): Promise<JiraSearchResult> {
    const q = parseSearchQuery(query)
    return this.data(requestId, (c, signal) => api.search(this.client, c, q, signal))
  }

  /** 범위 안의 워크로그. users 가 비면 나만 */
  async worklogs(query: unknown, requestId?: string): Promise<JiraWorklogResult> {
    const q = parseWorklogQuery(query)
    return this.data(requestId, async (c, signal) => {
      const epoch = this.epoch
      let me = this.me
      if (!me) {
        me = await api.getMyId(this.client, c, signal)
        if (this.epoch === epoch) this.me = me
      }
      return api.getWorklogs(this.client, c, q, me, signal)
    })
  }

  /** 팀원 추가용 사용자 검색 */
  async userSearch(text: unknown, requestId?: string): Promise<JiraUser[]> {
    const t = parseUserSearch(text)
    return this.data(requestId, (c, signal) => api.searchUsers(this.client, c, t, signal))
  }

  /** jira:openExternal 이 쓸 검증된 주소(연결한 사이트의 것만). 아니면 null */
  externalUrl(raw: unknown): string | null {
    return allowedJiraUrl(raw, this.conn?.siteUrl, this.deps)
  }

  // ── 요청 취소 ───────────────────────────────────────

  /** req:cancel 에서 호출 — 다른 서비스와 requestId 공간을 공유하므로 없는 id 는 무시한다 */
  cancel(requestId: string): void {
    this.requests.get(requestId)?.abort()
    this.requests.delete(requestId)
  }

  dispose(): void {
    this.epoch.abort()
    for (const ac of this.requests.values()) ac.abort()
    this.requests.clear()
  }
}
