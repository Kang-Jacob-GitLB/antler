import { create } from 'zustand'
import type {
  GhAuthState,
  GhComment,
  GhCommitSummary,
  GhDeviceCode,
  GhIssueDetail,
  GhIssueSummary,
  GhLabel,
  GhListQuery,
  GhListResult,
  GhPullDetail,
  GhPullFile,
  GhPullSummary,
  GhRepoRef,
  GhUser
} from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useActionsStore } from './actionsStore'
import { resetAvatarSession } from './avatarStore'
import { resetChecksSession } from './checksStore'
import { useDetailStore } from './detailStore'

/** 중앙에 띄우는 GitHub 화면 */
export type GhView =
  | { kind: 'pulls' }
  | { kind: 'issues' }
  | { kind: 'actions' }
  | { kind: 'pull'; number: number; tab: 'conversation' | 'commits' | 'files' }
  | { kind: 'issue'; number: number }

export interface ListState<T> {
  query: GhListQuery
  items: T[]
  hasMore: boolean
  /** "더 불러오기" 때 요청할 페이지(성공했을 때만 갱신된다) */
  nextPage: number
  loading: boolean
  error: string | null
}

/**
 * 목록 네 개: 중앙 목록(필터 가능)과 사이드바 목록(열린 항목 고정)을 따로 둔다.
 * 중앙에서 '닫힘'이나 라벨을 골라도 사이드바는 바뀌지 않는다.
 */
type ListKey = 'pulls' | 'issues' | 'sidePulls' | 'sideIssues'

const emptyList = <T>(): ListState<T> => ({ query: { state: 'open' }, items: [], hasMore: false, nextPage: 2, loading: false, error: null })

interface PullState {
  number: number
  detail: GhPullDetail | null
  files: GhPullFile[] | null
  commits: GhCommitSummary[] | null
  comments: GhComment[] | null
  error: string | null
  commentsError: string | null
}

interface GithubState {
  repoId: string | null
  /** undefined: 아직 모름, null: GitHub 저장소가 아님 */
  ref: GhRepoRef | null | undefined
  auth: GhAuthState
  device: GhDeviceCode | null
  devicePending: string | null
  authError: string | null
  /** 툴바의 GitHub 로그인 팝오버가 열려 있는지(다른 화면의 안내 버튼도 열 수 있게 스토어에 둔다) */
  authPopoverOpen: boolean
  view: GhView | null
  pulls: ListState<GhPullSummary>
  issues: ListState<GhIssueSummary>
  sidePulls: ListState<GhPullSummary>
  sideIssues: ListState<GhIssueSummary>
  filters: { labels: GhLabel[]; assignees: GhUser[] } | null
  pull: PullState | null
  issue: { number: number; detail: GhIssueDetail | null; comments: GhComment[] | null; error: string | null; commentsError: string | null } | null
  /** PR 파일 탭에서 고른 파일 */
  pullFile: string | null

  open(repoId: string | null): Promise<void>
  /** 로그인·로그아웃 뒤: 보고 있던 화면은 그대로 두고 목록·상세를 다시 받는다 */
  reload(): void
  refreshAuth(): Promise<void>
  loginPat(token: string): Promise<void>
  /** GitHub CLI(gh)에 로그인된 계정으로 로그인 */
  importCli(): Promise<void>
  setAuthPopoverOpen(open: boolean): void
  startDevice(): Promise<void>
  cancelDevice(): void
  logout(): Promise<void>
  show(view: GhView | null): void
  loadList(kind: 'pulls' | 'issues', query?: Partial<GhListQuery>, more?: boolean): Promise<void>
  loadFilters(): Promise<void>
  selectPullFile(path: string | null): void
}

/** 저장소 전환·다시 읽기마다 증가 — 늦게 온 응답을 버린다 */
let gen = 0
/** 목록별 요청 순번 — 같은 목록에 겹친 요청(더 보기 중 필터 변경 등) 중 마지막 것만 반영한다 */
const listSeq: Record<ListKey, number> = { pulls: 0, issues: 0, sidePulls: 0, sideIssues: 0 }
/** 비로그인에서 404(비공개 저장소)였던 저장소 — 로그인/로그아웃 전까지 사이드바 목록을 자동으로 다시 묻지 않는다(비로그인 한도는 시간당 60회) */
const anonNotFound = new Map<string, string>()
/** main 의 GH_NOT_FOUND 메시지에 들어 있는 문구(에러 코드는 IPC 를 건너오지 않는다) */
const NOT_FOUND_HINT = '로그인 필요'

export const useGithubStore = create<GithubState>((set, get) => {
  const current = (g: number, repoId: string | null): boolean => g === gen && get().repoId === repoId

  /** 목록 하나를 받는다. more 면 nextPage 를, 아니면 1페이지를 받는다 */
  const fetchList = async (key: ListKey, query: GhListQuery, more: boolean): Promise<void> => {
    const { repoId, ref } = get()
    if (!repoId || !ref) return
    const g = gen
    const seq = ++listSeq[key]
    const prev = get()[key] as ListState<unknown>
    const page = more ? prev.nextPage : 1
    set({ [key]: { ...prev, query, loading: true, error: null, ...(more ? {} : { items: [], hasMore: false }) } } as Partial<GithubState>)
    try {
      const q = { ...query, page }
      const res: GhListResult<unknown> =
        key === 'pulls' || key === 'sidePulls' ? await api.invoke('gh:pulls', repoId, q) : await api.invoke('gh:issues', repoId, q)
      if (!current(g, repoId) || seq !== listSeq[key]) return
      const items = more ? [...(get()[key] as ListState<unknown>).items, ...res.items] : res.items
      // 빈 페이지를 건너뛴 경우 main 이 다음에 요청할 페이지를 알려 준다
      const nextPage = res.nextPage ?? page + 1
      set({ [key]: { query, items, hasMore: res.hasMore, nextPage, loading: false, error: null } } as Partial<GithubState>)
      // 남은 호출 수 등 인증 상태가 바뀌었을 수 있다(사이드바 목록이 끝날 때 한 번만)
      if (key === 'sideIssues') void get().refreshAuth()
    } catch (err) {
      if (!current(g, repoId) || seq !== listSeq[key]) return
      // 실패해도 페이지 번호는 그대로 — 다시 '더 보기'를 누르면 같은 페이지를 다시 요청한다
      const msg = errorMessage(err)
      if (!get().auth.signedIn && msg.includes(NOT_FOUND_HINT)) anonNotFound.set(repoId, msg)
      set({ [key]: { ...(get()[key] as ListState<unknown>), loading: false, error: msg } } as Partial<GithubState>)
    }
  }

  const loadPull = async (number: number): Promise<void> => {
    const { repoId } = get()
    if (!repoId) return
    const g = gen
    set({ pull: { number, detail: null, files: null, commits: null, comments: null, error: null, commentsError: null }, pullFile: null })
    const mine = (): boolean => current(g, repoId) && get().pull?.number === number
    // 본문과 댓글은 따로 받는다 — 댓글이 실패해도 본문은 보인다
    void api.invoke('gh:comments', repoId, number).then(
      (comments) => mine() && set({ pull: { ...get().pull!, comments } }),
      (err) => mine() && set({ pull: { ...get().pull!, commentsError: errorMessage(err) } })
    )
    try {
      const detail = await api.invoke('gh:pull', repoId, number)
      if (mine()) set({ pull: { ...get().pull!, detail } })
    } catch (err) {
      if (mine()) set({ pull: { ...get().pull!, error: errorMessage(err) } })
    }
  }

  const loadPullTab = async (number: number, tab: 'commits' | 'files'): Promise<void> => {
    const { repoId, pull } = get()
    if (!repoId || !pull || pull.number !== number || pull[tab]) return
    const g = gen
    const mine = (): boolean => current(g, repoId) && get().pull?.number === number
    if (pull.error) set({ pull: { ...pull, error: null } })
    try {
      if (tab === 'files') {
        const files = await api.invoke('gh:pullFiles', repoId, number)
        if (mine()) set({ pull: { ...get().pull!, files }, pullFile: get().pullFile ?? files[0]?.path ?? null })
      } else {
        const commits = await api.invoke('gh:pullCommits', repoId, number)
        if (mine()) set({ pull: { ...get().pull!, commits } })
      }
    } catch (err) {
      if (mine()) set({ pull: { ...get().pull!, error: errorMessage(err) } })
    }
  }

  const loadIssue = async (number: number): Promise<void> => {
    const { repoId } = get()
    if (!repoId) return
    const g = gen
    set({ issue: { number, detail: null, comments: null, error: null, commentsError: null } })
    const mine = (): boolean => current(g, repoId) && get().issue?.number === number
    void api.invoke('gh:comments', repoId, number).then(
      (comments) => mine() && set({ issue: { ...get().issue!, comments } }),
      (err) => mine() && set({ issue: { ...get().issue!, commentsError: errorMessage(err) } })
    )
    try {
      const detail = await api.invoke('gh:issue', repoId, number)
      if (mine()) set({ issue: { ...get().issue!, detail } })
    } catch (err) {
      if (mine()) set({ issue: { ...get().issue!, error: errorMessage(err) } })
    }
  }

  /** 사이드바 목록(열린 항목) 두 개를 받는다 */
  const loadSide = (): void => {
    const known = anonNotFound.get(get().repoId ?? '')
    if (known !== undefined) {
      // 다시 묻지 않고, 전에 받은 이유(로그인 필요)를 그대로 보여 준다
      set({ sidePulls: { ...emptyList(), error: known }, sideIssues: { ...emptyList(), error: known } })
      return
    }
    void fetchList('sidePulls', { state: 'open' }, false)
    void fetchList('sideIssues', { state: 'open' }, false)
  }

  return {
    repoId: null,
    ref: undefined,
    auth: { signedIn: false },
    device: null,
    devicePending: null,
    authError: null,
    authPopoverOpen: false,
    view: null,
    pulls: emptyList(),
    issues: emptyList(),
    sidePulls: emptyList(),
    sideIssues: emptyList(),
    filters: null,
    pull: null,
    issue: null,
    pullFile: null,

    async open(repoId) {
      const g = ++gen
      useActionsStore.getState().reset()
      set({
        repoId,
        ref: undefined,
        view: null,
        pulls: emptyList(),
        issues: emptyList(),
        sidePulls: emptyList(),
        sideIssues: emptyList(),
        filters: null,
        pull: null,
        issue: null,
        pullFile: null
      })
      if (!repoId) return
      try {
        const ref = await api.invoke('gh:repo', repoId)
        if (!current(g, repoId)) return
        set({ ref })
        if (ref) loadSide()
      } catch {
        if (current(g, repoId)) set({ ref: null })
      }
    },

    reload() {
      const { repoId, ref, view } = get()
      anonNotFound.clear()
      // 로그인 상태가 바뀌면 커밋 체크 상태와 Actions 목록도 새 계정 기준으로 다시 받는다
      resetChecksSession()
      useActionsStore.getState().reset()
      if (!repoId) return
      ++gen
      // 비공개 저장소는 로그인 뒤에야 보이므로 GitHub 저장소 여부부터 다시 본다
      if (!ref) return void get().open(repoId)
      set({ filters: null, pull: null, issue: null })
      loadSide()
      if (get().pulls.items.length || view?.kind === 'pulls') void fetchList('pulls', get().pulls.query, false)
      if (get().issues.items.length || view?.kind === 'issues') void fetchList('issues', get().issues.query, false)
      if (view) get().show(view)
    },

    async refreshAuth() {
      try {
        set({ auth: await api.invoke('gh:auth') })
      } catch (err) {
        set({ authError: errorMessage(err) })
      }
    },

    async loginPat(token) {
      set({ authError: null })
      try {
        const auth = await api.invoke('gh:loginPat', token)
        set({ auth })
        resetAvatarSession()
        // 로그인하면 비공개 저장소·리뷰/CI 보강이 가능해지므로 보던 화면 그대로 다시 받는다
        get().reload()
      } catch (err) {
        set({ authError: errorMessage(err) })
        throw err
      }
    },

    async importCli() {
      set({ authError: null })
      try {
        const auth = await api.invoke('gh:importCliToken')
        set({ auth })
        resetAvatarSession()
        get().reload()
      } catch (err) {
        set({ authError: errorMessage(err) })
        throw err
      }
    },

    setAuthPopoverOpen(open) {
      set({ authPopoverOpen: open })
    },

    async startDevice() {
      // 이미 진행 중이면(빠른 두 번 클릭) 새로 시작하지 않는다
      if (get().devicePending) return
      const requestId = newRequestId('gh-device')
      set({ authError: null, device: null, devicePending: requestId })
      try {
        const device = await api.invoke('gh:deviceStart')
        if (get().devicePending !== requestId) return
        set({ device })
        void api.invoke('gh:openExternal', device.verificationUri)
        const auth = await api.invoke('gh:devicePoll', requestId)
        if (get().devicePending !== requestId) return
        set({ auth, device: null, devicePending: null })
        resetAvatarSession()
        get().reload()
      } catch (err) {
        if (get().devicePending !== requestId) return
        set({ device: null, devicePending: null, ...(isAbort(err) ? {} : { authError: errorMessage(err) }) })
      }
    },

    cancelDevice() {
      const id = get().devicePending
      if (id) void api.invoke('req:cancel', id)
      set({ device: null, devicePending: null })
    },

    async logout() {
      set({ auth: await api.invoke('gh:logout') })
      resetAvatarSession()
      get().reload()
    },

    show(view) {
      set({ view })
      if (!view) return
      // 파일 diff 가 열려 있으면 그쪽이 중앙을 차지하므로 닫고 GitHub 화면을 보인다
      useDetailStore.getState().closeFile()
      if (view.kind === 'pull') {
        const p = get().pull
        // 처음 보거나, 앞서 본문을 받지 못했으면 다시 받는다
        if (p?.number !== view.number || (p.error && !p.detail)) void loadPull(view.number)
        if (view.tab !== 'conversation') void loadPullTab(view.number, view.tab)
      } else if (view.kind === 'issue') {
        const i = get().issue
        if (i?.number !== view.number || (i.error && !i.detail)) void loadIssue(view.number)
      } else if (view.kind === 'actions') {
        const { repoId } = get()
        if (repoId) void useActionsStore.getState().load(repoId)
      } else {
        void get().loadFilters()
        const list = get()[view.kind]
        if (!list.items.length && !list.loading) void get().loadList(view.kind)
      }
    },

    async loadList(kind, query, more = false) {
      const q: GhListQuery = { ...get()[kind].query, ...query }
      delete q.page
      await fetchList(kind, q, more)
    },

    async loadFilters() {
      const { repoId, ref, filters } = get()
      if (!repoId || !ref || filters) return
      const g = gen
      try {
        const f = await api.invoke('gh:filters', repoId)
        if (current(g, repoId)) set({ filters: f })
      } catch {
        // 필터 없이도 목록은 볼 수 있다
      }
    },

    selectPullFile(path) {
      set({ pullFile: path })
    }
  }
})

/** 그래프 쪽 동작(찾아가기·ref 필터)을 하면 GitHub 화면을 닫아 그래프가 보이게 한다 */
export function closeGithubView(): void {
  if (useGithubStore.getState().view) useGithubStore.getState().show(null)
}
