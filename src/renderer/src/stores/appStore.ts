import { create } from 'zustand'
import type {
  AddRepoInput,
  AddReposInput,
  AddReposResult,
  AppConfig,
  HeadInfo,
  Locale,
  RefsSnapshot,
  Repo,
  RepoPatch,
  ThemePref,
  UiPrefs
} from '@shared/types'
import { api, errorMessage } from '../lib/api'
import { carryDetails, withDetails } from '../lib/refsDetails'
import { setTitleBarDark } from '../lib/titleBarSync'

type ResolvedTheme = 'dark' | 'light'

interface AppState {
  config: AppConfig | null
  activeRepoId: string | null
  head: HeadInfo | null
  /** 활성 저장소의 ref 스냅샷(사이드바) */
  refs: RefsSnapshot | null
  systemDark: boolean
  error: string | null
  switcherOpen: boolean
  addOpen: boolean
  /** 설정(외부 도구·보일 그룹) 대화상자 */
  settingsOpen: boolean

  init(): Promise<void>
  selectRepo(id: string | null): void
  refreshRefs(): Promise<void>
  /** 저장소 추가 대화상자 */
  openAddDialog(open?: boolean): void
  addRepo(input: AddRepoInput): Promise<void>
  /** 같은 환경의 여러 저장소를 한 번에 추가하고 첫 저장소를 연다. 실패한 경로는 결과로 돌려준다 */
  addRepos(input: AddReposInput): Promise<AddReposResult>
  updateRepo(id: string, patch: RepoPatch): Promise<void>
  removeRepo(id: string): Promise<void>
  setUi(patch: Partial<UiPrefs>): Promise<void>
  setTheme(theme: ThemePref): void
  setLocale(locale: Locale): void
  setSwitcherOpen(open: boolean): void
  openSettings(open?: boolean): void
  /** 에디터 실행 파일 선택 창(JetBrains·사용자 지정) */
  pickEditorExe(): Promise<void>
  clearError(): void
}

const media = window.matchMedia('(prefers-color-scheme: dark)')

export function resolveTheme(pref: ThemePref, systemDark: boolean): ResolvedTheme {
  return pref === 'system' ? (systemDark ? 'dark' : 'light') : pref
}

function applyTheme(pref: ThemePref, systemDark: boolean): void {
  const theme = resolveTheme(pref, systemDark)
  document.documentElement.dataset['theme'] = theme
  // 제목 표시줄 오버레이(창 컨트롤) 색도 같이 바꾼다
  setTitleBarDark(theme === 'dark')
}

export const useAppStore = create<AppState>((set, get) => ({
  config: null,
  activeRepoId: null,
  head: null,
  refs: null,
  systemDark: media.matches,
  error: null,
  switcherOpen: false,
  addOpen: false,
  settingsOpen: false,

  async init() {
    const config = await api.invoke('config:get')
    const last = config.ui.lastRepoId
    const activeRepoId = config.repos.some((r) => r.id === last) ? last! : (config.repos[0]?.id ?? null)
    set({ config, activeRepoId })
    applyTheme(config.ui.theme, get().systemDark)
    document.documentElement.lang = config.ui.locale
    media.addEventListener('change', (e) => {
      set({ systemDark: e.matches })
      applyTheme(get().config?.ui.theme ?? 'system', e.matches)
    })
    void api.invoke('repos:watch', activeRepoId)
    void get().refreshRefs()
  },

  selectRepo(id) {
    if (id === get().activeRepoId) return
    set({ activeRepoId: id, head: null, refs: null })
    // 활성 저장소만 감시한다(비활성 저장소는 폴링하지 않는다)
    void api.invoke('repos:watch', id)
    if (id) {
      void get().setUi({ lastRepoId: id })
      void get().refreshRefs()
    }
  },

  async refreshRefs() {
    const id = get().activeRepoId
    if (!id) return
    try {
      const prev = get().refs
      const fresh = await api.invoke('git:refs', id)
      if (get().activeRepoId !== id) return
      // 첫 화면에 필요한 스냅샷을 먼저 보여 주고, 비싼 부가 정보는 나중에 합친다
      const refs = carryDetails(fresh, prev)
      set({ refs, head: refs.head })
      void api
        .invoke('git:refsDetails', id)
        .then((d) => {
          // 이전 스냅샷의 값이 아니라 방금 읽은 스냅샷에 합친다(push 로 동기화된 브랜치의 낡은 ↑ 가 남지 않게)
          if (get().activeRepoId === id && get().refs === refs) set({ refs: withDetails(fresh, d) })
        })
        .catch(() => {})
    } catch (err) {
      set({ error: errorMessage(err) })
    }
  },

  openAddDialog(open = true) {
    set({ addOpen: open })
  },

  async addRepo(input) {
    // 실패는 대화상자 안에서 보여 주도록 그대로 던진다
    const repo: Repo = await api.invoke('repos:add', input)
    const config = await api.invoke('config:get')
    set({ config })
    get().selectRepo(repo.id)
  },

  async addRepos(input) {
    const result = await api.invoke('repos:addMany', input)
    const first = result.added[0]
    if (first) {
      const config = await api.invoke('config:get')
      set({ config })
      get().selectRepo(first.id)
    }
    return result
  },

  async updateRepo(id, patch) {
    try {
      const config = await api.invoke('repos:update', id, patch)
      set({ config })
    } catch (err) {
      set({ error: errorMessage(err) })
    }
  },

  async removeRepo(id) {
    try {
      const config = await api.invoke('repos:remove', id)
      const active = get().activeRepoId === id ? (config.repos[0]?.id ?? null) : get().activeRepoId
      set({ config })
      if (active !== get().activeRepoId) get().selectRepo(active)
    } catch (err) {
      set({ error: errorMessage(err) })
    }
  },

  async setUi(patch) {
    const prev = get().config
    if (prev) set({ config: { ...prev, ui: { ...prev.ui, ...patch } } }) // 낙관적 반영
    try {
      const config = await api.invoke('config:setUi', patch)
      set({ config })
    } catch (err) {
      // 저장에 실패하면 화면도 이전 설정으로 되돌린다
      if (prev) {
        set({ config: prev })
        applyTheme(prev.ui.theme, get().systemDark)
        document.documentElement.lang = prev.ui.locale
      }
      set({ error: errorMessage(err) })
    }
  },

  setTheme(next) {
    applyTheme(next, get().systemDark)
    void get().setUi({ theme: next })
  },

  setLocale(next) {
    document.documentElement.lang = next
    void get().setUi({ locale: next })
  },

  setSwitcherOpen(open) {
    set({ switcherOpen: open })
  },

  openSettings(open = true) {
    set({ settingsOpen: open })
  },

  async pickEditorExe() {
    try {
      const config = await api.invoke('external:pickEditorExe')
      if (config) set({ config })
    } catch (err) {
      set({ error: errorMessage(err) })
    }
  },

  clearError() {
    set({ error: null })
  }
}))

export function useActiveRepo(): Repo | null {
  return useAppStore((s) => s.config?.repos.find((r) => r.id === s.activeRepoId) ?? null)
}

/** 왼쪽 사이드바 보이기/숨기기(설정에 기억) */
export function toggleSidebar(): void {
  const s = useAppStore.getState()
  void s.setUi({ sidebarHidden: s.config?.ui.sidebarHidden !== true })
}

/** 오른쪽 상세 패널 보이기/숨기기(설정에 기억) */
export function toggleDetail(): void {
  const s = useAppStore.getState()
  void s.setUi({ detailHidden: s.config?.ui.detailHidden !== true })
}
