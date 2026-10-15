import { create } from 'zustand'
import type {
  AddRepoInput,
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

  init(): Promise<void>
  selectRepo(id: string | null): void
  refreshRefs(): Promise<void>
  /** 저장소 추가 대화상자 */
  openAddDialog(open?: boolean): void
  addRepo(input: AddRepoInput): Promise<void>
  updateRepo(id: string, patch: RepoPatch): Promise<void>
  removeRepo(id: string): Promise<void>
  setUi(patch: Partial<UiPrefs>): Promise<void>
  cycleTheme(): void
  toggleLocale(): void
  setSwitcherOpen(open: boolean): void
  clearError(): void
}

const media = window.matchMedia('(prefers-color-scheme: dark)')

export function resolveTheme(pref: ThemePref, systemDark: boolean): ResolvedTheme {
  return pref === 'system' ? (systemDark ? 'dark' : 'light') : pref
}

function applyTheme(pref: ThemePref, systemDark: boolean): void {
  document.documentElement.dataset['theme'] = resolveTheme(pref, systemDark)
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
      const refs = await api.invoke('git:refs', id)
      if (get().activeRepoId === id) set({ refs, head: refs.head })
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

  cycleTheme() {
    const cur = get().config?.ui.theme ?? 'system'
    const next: ThemePref = cur === 'system' ? 'dark' : cur === 'dark' ? 'light' : 'system'
    applyTheme(next, get().systemDark)
    void get().setUi({ theme: next })
  },

  toggleLocale() {
    const next: Locale = get().config?.ui.locale === 'en' ? 'ko' : 'en'
    document.documentElement.lang = next
    void get().setUi({ locale: next })
  },

  setSwitcherOpen(open) {
    set({ switcherOpen: open })
  },

  clearError() {
    set({ error: null })
  }
}))

export function useActiveRepo(): Repo | null {
  return useAppStore((s) => s.config?.repos.find((r) => r.id === s.activeRepoId) ?? null)
}
