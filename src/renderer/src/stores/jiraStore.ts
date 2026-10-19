import { create } from 'zustand'
import type { JiraAuthState, JiraComment, JiraConnectInput, JiraIssueDetail, JiraIssueSummary, JiraSavedFilter } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'
import { useGithubStore } from './githubStore'

/** 사이드바 "나에게 할당" 과 중앙 목록이 쓰는 JQL(고정) */
export const MINE_JQL = 'assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC'
const MAX_RECENT = 15
const MAX_FILTERS = 50

/** 중앙에 띄우는 Jira 화면. 목록은 'mine'(나에게 할당), 저장 필터의 인덱스, 'custom'(직접 입력한 JQL) */
export type JiraView = { kind: 'list'; source: 'mine' | 'custom' | number } | { kind: 'issue'; key: string; from: JiraView | null }

export interface JiraList {
  /** 어느 목록의 결과인지(JQL 문자열) */
  jql: string
  items: JiraIssueSummary[]
  nextCursor: string | undefined
  page: number
  loading: boolean
  error: string | null
}

const emptyList = (jql: string): JiraList => ({ jql, items: [], nextCursor: undefined, page: 1, loading: false, error: null })

interface JiraIssueState {
  key: string
  detail: JiraIssueDetail | null
  comments: JiraComment[] | null
  error: string | null
  commentsError: string | null
}

interface JiraState {
  auth: JiraAuthState
  authLoaded: boolean
  dialogOpen: boolean
  view: JiraView | null
  /** 사이드바의 "나에게 할당" */
  mine: JiraList
  /** 중앙 목록 */
  list: JiraList
  issue: JiraIssueState | null

  refreshAuth(): Promise<void>
  connect(input: JiraConnectInput): Promise<void>
  disconnect(): Promise<void>
  setDialogOpen(open: boolean): void
  show(view: JiraView | null): void
  openIssue(key: string): void
  loadMine(): Promise<void>
  loadList(jql: string, more?: boolean): Promise<void>
  saveFilter(f: JiraSavedFilter): void
  removeFilter(index: number): void
}

/** 요청 순번 — 늦게 온 응답을 버린다 */
let issueSeq = 0
const listSeq = { mine: 0, list: 0 }
let issueReq: string | null = null

export const useJiraStore = create<JiraState>((set, get) => {
  const fetchList = async (key: 'mine' | 'list', jql: string, more: boolean): Promise<void> => {
    const prev = get()[key]
    const seq = ++listSeq[key]
    const base: JiraList = more && prev.jql === jql ? prev : emptyList(jql)
    const page = more ? base.page + 1 : 1
    set({ [key]: { ...base, jql, loading: true, error: null } } as Partial<JiraState>)
    try {
      const res = await api.invoke('jira:search', { jql, page, ...(more && base.nextCursor ? { cursor: base.nextCursor } : {}) })
      if (seq !== listSeq[key]) return
      set({ [key]: { jql, items: more ? [...base.items, ...res.items] : res.items, nextCursor: res.nextCursor, page, loading: false, error: null } } as Partial<JiraState>)
    } catch (err) {
      if (seq !== listSeq[key] || isAbort(err)) return
      set({ [key]: { ...base, jql, loading: false, error: errorMessage(err) } } as Partial<JiraState>)
    }
  }

  const loadIssue = async (key: string): Promise<void> => {
    const seq = ++issueSeq
    if (issueReq) void api.invoke('req:cancel', issueReq).catch(() => {})
    const req = newRequestId('jira')
    issueReq = req
    set({ issue: { key, detail: null, comments: null, error: null, commentsError: null } })
    const patch = (p: Partial<JiraIssueState>): void => {
      const cur = get().issue
      if (seq === issueSeq && cur?.key === key) set({ issue: { ...cur, ...p } })
    }
    void api
      .invoke('jira:issue', key, req)
      .then((detail) => {
        patch({ detail })
        remember({ key: detail.key || key, summary: detail.summary })
      })
      .catch((err) => !isAbort(err) && patch({ error: errorMessage(err) }))
    void api
      .invoke('jira:comments', key, req)
      .then((comments) => patch({ comments }))
      .catch((err) => !isAbort(err) && patch({ commentsError: errorMessage(err) }))
  }

  /** 최근 본 이슈를 설정에 기억한다(맨 앞으로, 중복 제거) */
  const remember = (r: { key: string; summary: string }): void => {
    const app = useAppStore.getState()
    const cur = app.config?.ui.jiraRecent ?? []
    if (cur[0]?.key === r.key && cur[0].summary === r.summary) return
    void app.setUi({ jiraRecent: [r, ...cur.filter((x) => x.key !== r.key)].slice(0, MAX_RECENT) })
  }

  return {
    auth: { connected: false },
    authLoaded: false,
    dialogOpen: false,
    view: null,
    mine: emptyList(MINE_JQL),
    list: emptyList(MINE_JQL),
    issue: null,

    async refreshAuth() {
      try {
        const auth = await api.invoke('jira:auth')
        set({ auth, authLoaded: true })
        if (auth.connected) void get().loadMine()
      } catch {
        set({ authLoaded: true })
      }
    },

    async connect(input) {
      // 실패는 대화상자 안에서 보여 주도록 그대로 던진다
      const auth = await api.invoke('jira:connect', input)
      set({ auth, mine: emptyList(MINE_JQL), issue: null })
      void get().loadMine()
    },

    async disconnect() {
      const auth = await api.invoke('jira:disconnect')
      issueSeq++
      listSeq.mine++
      listSeq.list++
      set({ auth, view: null, issue: null, mine: emptyList(MINE_JQL), list: emptyList(MINE_JQL) })
    },

    setDialogOpen(dialogOpen) {
      set({ dialogOpen })
    },

    show(view) {
      if (view) {
        // 중앙에는 GitHub 화면과 Jira 화면 중 하나만 둔다
        if (useGithubStore.getState().view) useGithubStore.getState().show(null)
        if (view.kind === 'issue') {
          void loadIssue(view.key)
        } else if (view.source !== 'custom') {
          const apps = useAppStore.getState().config?.ui.jiraFilters ?? []
          const jql = view.source === 'mine' ? MINE_JQL : apps[view.source]?.jql
          if (!jql) return
          if (get().list.jql !== jql || get().list.items.length === 0) void fetchList('list', jql, false)
        }
      } else {
        issueSeq++
        if (issueReq) void api.invoke('req:cancel', issueReq).catch(() => {})
        issueReq = null
      }
      set({ view })
    },

    openIssue(key) {
      const cur = get().view
      get().show({ kind: 'issue', key, from: cur?.kind === 'list' ? cur : (cur?.kind === 'issue' ? cur.from : null) })
    },

    loadMine: () => fetchList('mine', MINE_JQL, false),

    loadList: (jql, more = false) => fetchList('list', jql, more),

    saveFilter(f) {
      const app = useAppStore.getState()
      const cur = app.config?.ui.jiraFilters ?? []
      void app.setUi({ jiraFilters: [...cur.filter((x) => x.name !== f.name), f].slice(0, MAX_FILTERS) })
    },

    removeFilter(index) {
      const app = useAppStore.getState()
      const cur = app.config?.ui.jiraFilters ?? []
      void app.setUi({ jiraFilters: cur.filter((_, i) => i !== index) })
      const v = get().view
      if (v?.kind === 'list' && typeof v.source === 'number') get().show(null)
    }
  }
})

// GitHub 화면이 열리면(사이드바의 PR·이슈 선택 등) Jira 화면은 닫는다
useGithubStore.subscribe((s, prev) => {
  if (s.view && !prev.view && useJiraStore.getState().view) useJiraStore.getState().show(null)
})
