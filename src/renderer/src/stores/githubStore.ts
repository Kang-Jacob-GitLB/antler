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
  GhPullDetail,
  GhPullFile,
  GhPullSummary,
  GhRepoRef,
  GhUser
} from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useDetailStore } from './detailStore'

/** 중앙에 띄우는 GitHub 화면 */
export type GhView =
  | { kind: 'pulls' }
  | { kind: 'issues' }
  | { kind: 'pull'; number: number; tab: 'conversation' | 'commits' | 'files' }
  | { kind: 'issue'; number: number }

interface ListState<T> {
  query: GhListQuery
  items: T[]
  hasMore: boolean
  loading: boolean
  error: string | null
}

const emptyList = <T>(): ListState<T> => ({ query: { state: 'open' }, items: [], hasMore: false, loading: false, error: null })

interface GithubState {
  repoId: string | null
  /** undefined: 아직 모름, null: GitHub 저장소가 아님 */
  ref: GhRepoRef | null | undefined
  auth: GhAuthState
  device: GhDeviceCode | null
  devicePending: string | null
  authError: string | null
  view: GhView | null
  pulls: ListState<GhPullSummary>
  issues: ListState<GhIssueSummary>
  filters: { labels: GhLabel[]; assignees: GhUser[] } | null
  pull: { number: number; detail: GhPullDetail | null; files: GhPullFile[] | null; commits: GhCommitSummary[] | null; comments: GhComment[] | null; error: string | null } | null
  issue: { number: number; detail: GhIssueDetail | null; comments: GhComment[] | null; error: string | null } | null
  /** PR 파일 탭에서 고른 파일 */
  pullFile: string | null

  open(repoId: string | null): Promise<void>
  refreshAuth(): Promise<void>
  loginPat(token: string): Promise<void>
  startDevice(): Promise<void>
  cancelDevice(): void
  logout(): Promise<void>
  show(view: GhView | null): void
  loadList(kind: 'pulls' | 'issues', query?: Partial<GhListQuery>, more?: boolean): Promise<void>
  loadFilters(): Promise<void>
  selectPullFile(path: string | null): void
}

/** 저장소 전환·새로고침마다 증가 — 늦게 온 응답을 버린다 */
let gen = 0

export const useGithubStore = create<GithubState>((set, get) => {
  const current = (g: number, repoId: string | null): boolean => g === gen && get().repoId === repoId

  const loadPull = async (number: number): Promise<void> => {
    const { repoId } = get()
    if (!repoId) return
    const g = gen
    set({ pull: { number, detail: null, files: null, commits: null, comments: null, error: null }, pullFile: null })
    try {
      // 대화 탭에 필요한 것부터 받고, 커밋·파일은 탭을 열 때 받는다
      const [detail, comments] = await Promise.all([api.invoke('gh:pull', repoId, number), api.invoke('gh:comments', repoId, number)])
      if (!current(g, repoId) || get().pull?.number !== number) return
      set({ pull: { ...get().pull!, detail, comments } })
    } catch (err) {
      if (current(g, repoId) && get().pull?.number === number) set({ pull: { ...get().pull!, error: errorMessage(err) } })
    }
  }

  const loadPullTab = async (number: number, tab: 'commits' | 'files'): Promise<void> => {
    const { repoId, pull } = get()
    if (!repoId || !pull || pull.number !== number || pull[tab]) return
    const g = gen
    try {
      if (tab === 'files') {
        const files = await api.invoke('gh:pullFiles', repoId, number)
        if (current(g, repoId) && get().pull?.number === number) {
          set({ pull: { ...get().pull!, files }, pullFile: get().pullFile ?? files[0]?.path ?? null })
        }
      } else {
        const commits = await api.invoke('gh:pullCommits', repoId, number)
        if (current(g, repoId) && get().pull?.number === number) set({ pull: { ...get().pull!, commits } })
      }
    } catch (err) {
      if (current(g, repoId) && get().pull?.number === number) set({ pull: { ...get().pull!, error: errorMessage(err) } })
    }
  }

  const loadIssue = async (number: number): Promise<void> => {
    const { repoId } = get()
    if (!repoId) return
    const g = gen
    set({ issue: { number, detail: null, comments: null, error: null } })
    try {
      const [detail, comments] = await Promise.all([api.invoke('gh:issue', repoId, number), api.invoke('gh:comments', repoId, number)])
      if (current(g, repoId) && get().issue?.number === number) set({ issue: { number, detail, comments, error: null } })
    } catch (err) {
      if (current(g, repoId) && get().issue?.number === number) set({ issue: { ...get().issue!, error: errorMessage(err) } })
    }
  }

  return {
    repoId: null,
    ref: undefined,
    auth: { signedIn: false },
    device: null,
    devicePending: null,
    authError: null,
    view: null,
    pulls: emptyList(),
    issues: emptyList(),
    filters: null,
    pull: null,
    issue: null,
    pullFile: null,

    async open(repoId) {
      const g = ++gen
      set({ repoId, ref: undefined, view: null, pulls: emptyList(), issues: emptyList(), filters: null, pull: null, issue: null, pullFile: null })
      if (!repoId) return
      try {
        const ref = await api.invoke('gh:repo', repoId)
        if (!current(g, repoId)) return
        set({ ref })
        // 사이드바용으로 열린 PR·이슈를 미리 받아 둔다
        if (ref) {
          void get().loadList('pulls')
          void get().loadList('issues')
        }
      } catch {
        if (current(g, repoId)) set({ ref: null })
      }
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
        // 로그인하면 비공개 저장소·리뷰/CI 보강이 가능해지므로 목록을 다시 받는다
        void get().open(get().repoId)
      } catch (err) {
        set({ authError: errorMessage(err) })
        throw err
      }
    },

    async startDevice() {
      set({ authError: null, device: null })
      try {
        const device = await api.invoke('gh:deviceStart')
        const requestId = newRequestId('gh-device')
        set({ device, devicePending: requestId })
        void api.invoke('gh:openExternal', device.verificationUri)
        const auth = await api.invoke('gh:devicePoll', requestId)
        if (get().devicePending !== requestId) return
        set({ auth, device: null, devicePending: null })
        void get().open(get().repoId)
      } catch (err) {
        if (isAbort(err)) return
        set({ authError: errorMessage(err), device: null, devicePending: null })
      }
    },

    cancelDevice() {
      const id = get().devicePending
      if (id) void api.invoke('req:cancel', id)
      set({ device: null, devicePending: null })
    },

    async logout() {
      set({ auth: await api.invoke('gh:logout') })
      void get().open(get().repoId)
    },

    show(view) {
      set({ view })
      if (!view) return
      // 파일 diff 가 열려 있으면 그쪽이 중앙을 차지하므로 닫고 GitHub 화면을 보인다
      useDetailStore.getState().closeFile()
      if (view.kind === 'pull') {
        if (get().pull?.number !== view.number) void loadPull(view.number)
        if (view.tab !== 'conversation') void loadPullTab(view.number, view.tab)
      } else if (view.kind === 'issue') {
        if (get().issue?.number !== view.number) void loadIssue(view.number)
      } else {
        void get().loadFilters()
        const list = get()[view.kind]
        if (!list.items.length && !list.loading) void get().loadList(view.kind)
      }
    },

    async loadList(kind, query, more = false) {
      const { repoId, ref } = get()
      if (!repoId || !ref) return
      const g = gen
      const prev = get()[kind]
      const q: GhListQuery = { ...prev.query, ...query, page: more ? (prev.query.page ?? 1) + 1 : 1 }
      set({ [kind]: { ...prev, query: q, loading: true, error: null, ...(more ? {} : { items: [] }) } } as Partial<GithubState>)
      try {
        const res =
          kind === 'pulls' ? await api.invoke('gh:pulls', repoId, q) : await api.invoke('gh:issues', repoId, q)
        if (!current(g, repoId)) return
        const items = more ? [...get()[kind].items, ...res.items] : res.items
        set({ [kind]: { query: q, items, hasMore: res.hasMore, loading: false, error: null } } as Partial<GithubState>)
        // 남은 호출 수 등 인증 상태가 바뀌었을 수 있다
        void get().refreshAuth()
      } catch (err) {
        if (current(g, repoId)) set({ [kind]: { ...get()[kind], loading: false, error: errorMessage(err) } } as Partial<GithubState>)
      }
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
