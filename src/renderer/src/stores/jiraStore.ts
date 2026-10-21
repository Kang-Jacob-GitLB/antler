import { create } from 'zustand'
import type { JiraAuthState, JiraComment, JiraConnectInput, JiraIssueDetail, JiraIssueSummary, JiraSavedFilter, JiraSiteData, JiraUser } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useAppStore } from './appStore'
import { useGithubStore } from './githubStore'

/** "나에게 할당"(Jira 화면 왼쪽 목록 개수 · Launchpad)과 그 목록이 쓰는 JQL(고정) */
export const MINE_JQL = 'assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC'
const MAX_RECENT = 15
const MAX_FILTERS = 50
/** 워크로그 리포트에 둘 수 있는 팀원 수(main 의 조회 상한과 같다) */
export const MAX_TEAM = 30

/** Jira 화면(창 전체를 덮는 작업 공간, null 이면 닫힘). 목록은 'mine'(나에게 할당), 저장 필터의 인덱스, 'custom'(직접 입력한 JQL). worklog = 워크로그 달력·리포트 */
export type JiraView = { kind: 'list'; source: 'mine' | 'custom' | number } | { kind: 'worklog' } | { kind: 'issue'; key: string; from: JiraView | null }

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
  /** "나에게 할당"(Jira 화면 왼쪽 개수 · Launchpad) */
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
  /** 이슈의 상태 전환을 실행하고 성공하면 상세·목록을 새로 읽는다. 실패는 이슈 오류로 보인다 */
  transitionIssue(key: string, transitionId: string): Promise<void>
  loadMine(): Promise<void>
  loadList(jql: string, more?: boolean): Promise<void>
  saveFilter(f: JiraSavedFilter): void
  removeFilter(index: number): void
  addTeamMember(user: JiraUser): void
  removeTeamMember(id: string): void
}

/** 요청 순번 — 늦게 온 응답을 버린다 */
let issueSeq = 0
const listSeq = { mine: 0, list: 0 }
/** 이슈·댓글은 요청 ID 를 따로 쓴다(취소가 둘 다 되도록) */
let issueReqs: string[] = []

/** 지금 연결한 사이트의 저장 필터·최근 본 이슈(사이트별로 분리 저장) */
export function useJiraSiteData(): JiraSiteData {
  const site = useJiraStore((s) => s.auth.siteUrl)
  return useAppStore((s) => (site ? s.config?.ui.jiraSites?.[site] : undefined) ?? NO_SITE_DATA)
}
const NO_SITE_DATA: JiraSiteData = {}

function currentSite(): string | undefined {
  return useJiraStore.getState().auth.siteUrl
}

function siteData(): JiraSiteData {
  const site = currentSite()
  return (site ? useAppStore.getState().config?.ui.jiraSites?.[site] : undefined) ?? NO_SITE_DATA
}

function saveSiteData(site: string, data: JiraSiteData): void {
  const app = useAppStore.getState()
  void app.setUi({ jiraSites: { ...(app.config?.ui.jiraSites ?? {}), [site]: data } })
}

export const useJiraStore = create<JiraState>((set, get) => {
  const fetchList = async (key: 'mine' | 'list', jql: string, more: boolean): Promise<void> => {
    const prev = get()[key]
    const seq = ++listSeq[key]
    const base: JiraList = more && prev.jql === jql ? prev : emptyList(jql)
    const page = more ? base.page + 1 : 1
    // 같은 목록을 새로 고칠 때는 받아 둔 항목을 그대로 보여 주다가 응답이 오면 바꾼다(빈 화면 깜박임 방지)
    const shown = !more && prev.jql === jql ? prev : base
    set({ [key]: { ...shown, jql, loading: true, error: null } } as Partial<JiraState>)
    try {
      const res = await api.invoke('jira:search', { jql, page, ...(more && base.nextCursor ? { cursor: base.nextCursor } : {}) })
      if (seq !== listSeq[key]) return
      set({ [key]: { jql, items: more ? [...base.items, ...res.items] : res.items, nextCursor: res.nextCursor, page, loading: false, error: null } } as Partial<JiraState>)
    } catch (err) {
      if (seq !== listSeq[key] || isAbort(err)) return
      set({ [key]: { ...base, jql, loading: false, error: errorMessage(err) } } as Partial<JiraState>)
    }
  }

  /** keep: 같은 이슈를 새로 고칠 때 받아 둔 내용을 응답이 올 때까지 그대로 보인다(깜박임 방지) */
  const loadIssue = async (key: string, keep = false): Promise<void> => {
    const seq = ++issueSeq
    cancelIssueReqs()
    const reqIssue = newRequestId('jira')
    const reqComments = newRequestId('jira')
    issueReqs = [reqIssue, reqComments]
    const prev = get().issue
    set({ issue: keep && prev?.key === key ? { ...prev, error: null, commentsError: null } : { key, detail: null, comments: null, error: null, commentsError: null } })
    const patch = (p: Partial<JiraIssueState>): void => {
      const cur = get().issue
      if (seq === issueSeq && cur?.key === key) set({ issue: { ...cur, ...p } })
    }
    void api
      .invoke('jira:issue', key, reqIssue)
      .then((detail) => {
        patch({ detail })
        remember({ key: detail.key || key, summary: detail.summary })
      })
      .catch((err) => !isAbort(err) && patch({ error: errorMessage(err) }))
    void api
      .invoke('jira:comments', key, reqComments)
      .then((comments) => patch({ comments }))
      .catch((err) => !isAbort(err) && patch({ commentsError: errorMessage(err) }))
  }

  /** 최근 본 이슈를 설정에 기억한다(맨 앞으로, 중복 제거) */
  const remember = (r: { key: string; summary: string }): void => {
    const site = currentSite()
    if (!site) return
    const cur = siteData().recent ?? []
    if (cur[0]?.key === r.key && cur[0].summary === r.summary) return
    saveSiteData(site, { ...siteData(), recent: [r, ...cur.filter((x) => x.key !== r.key)].slice(0, MAX_RECENT) })
  }

  const cancelIssueReqs = (): void => {
    for (const id of issueReqs) void api.invoke('req:cancel', id).catch(() => {})
    issueReqs = []
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
      const site = currentSite()
      const auth = await api.invoke('jira:disconnect')
      // 이 사이트의 저장 필터·최근 본 이슈는 지운다(다른 사이트로 연결했을 때 옛 키로 조회하지 않게)
      const app = useAppStore.getState()
      const sites = { ...(app.config?.ui.jiraSites ?? {}) }
      if (site && site in sites) {
        delete sites[site]
        void app.setUi({ jiraSites: Object.keys(sites).length ? sites : undefined })
      }
      cancelIssueReqs()
      issueSeq++
      listSeq.mine++
      listSeq.list++
      // Jira 화면은 열어 둔 채 '연결 안 됨' 안내로 바꾼다
      set({ auth, view: get().view ? { kind: 'list', source: 'mine' } : null, issue: null, mine: emptyList(MINE_JQL), list: emptyList(MINE_JQL) })
    },

    setDialogOpen(dialogOpen) {
      set({ dialogOpen })
    },

    show(view) {
      // Jira 화면은 창 전체를 덮는다 — 아래 화면(그래프·GitHub·Launchpad)은 그대로 두어 뒤로가기로 돌아간다.
      // 연결 전에는 불러오지 않고 연결 안내만 보인다
      if (view && get().auth.connected) {
        // 화면을 새로 열 때마다 "나에게 할당" 개수도 최신으로
        if (!get().view) void fetchList('mine', MINE_JQL, false)
        if (view.kind === 'issue') {
          void loadIssue(view.key)
        } else if (view.kind === 'list' && view.source !== 'custom') {
          const apps = siteData().filters ?? []
          const jql = view.source === 'mine' ? MINE_JQL : apps[view.source]?.jql
          if (!jql) return
          // 목록으로 올 때마다 다시 조회한다(받아 둔 결과는 응답이 올 때까지 그대로 보인다)
          void fetchList('list', jql, false)
        }
      } else if (!view) {
        issueSeq++
        cancelIssueReqs()
      }
      set({ view })
    },

    openIssue(key) {
      const cur = get().view
      get().show({ kind: 'issue', key, from: cur?.kind === 'issue' ? cur.from : cur })
    },

    async transitionIssue(key, transitionId) {
      const setError = (error: string | null): void => {
        const cur = get().issue
        if (cur?.key === key) set({ issue: { ...cur, error } })
      }
      setError(null)
      try {
        await api.invoke('jira:transition', key, transitionId)
      } catch (err) {
        setError(errorMessage(err))
        return
      }
      // 전환하는 사이 연결을 끊었거나 다른 화면·이슈로 옮겼으면 그쪽 요청을 건드리지 않는다
      const { auth, view } = get()
      if (!auth.connected) return
      // 상세는 아직 그 이슈를 보고 있을 때만 새로 읽는다. 목록은 다시 열 때마다 조회하므로(show) 여기서 되돌리지 않는다
      if (view?.kind === 'issue' && view.key === key) void loadIssue(key, true)
      void fetchList('mine', MINE_JQL, false)
    },

    loadMine: () => fetchList('mine', MINE_JQL, false),

    loadList: (jql, more = false) => fetchList('list', jql, more),

    saveFilter(f) {
      const site = currentSite()
      if (!site) return
      const cur = siteData().filters ?? []
      saveSiteData(site, { ...siteData(), filters: [...cur.filter((x) => x.name !== f.name), f].slice(0, MAX_FILTERS) })
    },

    addTeamMember(user) {
      const site = currentSite()
      if (!site) return
      const cur = siteData().team ?? []
      if (cur.length >= MAX_TEAM || cur.some((u) => u.id === user.id)) return
      saveSiteData(site, { ...siteData(), team: [...cur, { id: user.id, name: user.name }] })
    },

    removeTeamMember(id) {
      const site = currentSite()
      if (!site) return
      const team = (siteData().team ?? []).filter((u) => u.id !== id)
      saveSiteData(site, { ...siteData(), team: team.length ? team : undefined })
    },

    removeFilter(index) {
      const site = currentSite()
      if (!site) return
      saveSiteData(site, { ...siteData(), filters: (siteData().filters ?? []).filter((_, i) => i !== index) })
      const v = get().view
      // 보고 있던 필터를 지웠으면 '나에게 할당'으로(Jira 화면은 닫지 않는다)
      if (v?.kind === 'list' && v.source === index) get().show({ kind: 'list', source: 'mine' })
      // 뒤쪽 필터를 보고 있었으면 번호가 하나 당겨진다
      else if (v?.kind === 'list' && typeof v.source === 'number' && v.source > index) set({ view: { kind: 'list', source: v.source - 1 } })
    }
  }
})

// GitHub 화면이 열리면(커맨드 팔레트의 PR·이슈 열기 등) Jira 화면은 닫아 그 화면이 보이게 한다
useGithubStore.subscribe((s, prev) => {
  if (s.view && !prev.view && useJiraStore.getState().view) useJiraStore.getState().show(null)
})
