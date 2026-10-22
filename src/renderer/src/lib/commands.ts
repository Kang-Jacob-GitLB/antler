// 명령 레지스트리: 명령 팔레트·단축키 목록·전역 단축키(App 의 onKey)가 모두 이 한 곳을 출처로 쓴다.
// 명령의 run 은 툴바·상태바 버튼이 부르는 것과 같은 함수·스토어 액션을 호출한다.
import type { EditorKind, ExternalAction, Locale, ThemePref } from '@shared/types'
import type { MessageKey, useT } from '../i18n'
import { toggleDetail, toggleSidebar, useAppStore } from '../stores/appStore'
import { useDetailStore } from '../stores/detailStore'
import { useFetchStore } from '../stores/fetchStore'
import { useGithubStore } from '../stores/githubStore'
import { useJiraStore } from '../stores/jiraStore'
import { useLaunchpadStore } from '../stores/launchpadStore'
import { useLogStore } from '../stores/logStore'
import { usePaletteStore } from '../stores/paletteStore'
import { useSearchStore } from '../stores/searchStore'
import { useUpdateStore } from '../stores/updateStore'
import type { CommandCategory, CommandCtx, CommandLike } from './commandFilter'
import { externalSupported, openExternal, openOnGithub, sourceKindOf } from './external'
import { matchesShortcut, type KeyEventLike, type Shortcut } from './shortcuts'

type T = ReturnType<typeof useT>

export const CATEGORY_ORDER: CommandCategory[] = ['general', 'list', 'diff', 'panel']
export const CATEGORY_KEY: Record<CommandCategory, MessageKey> = {
  general: 'cmd.cat.general',
  list: 'cmd.cat.list',
  diff: 'cmd.cat.diff',
  panel: 'cmd.cat.panel'
}

interface CommandDef {
  id: string
  category: CommandCategory
  title(t: T, c: CommandCtx): string
  keywords?: string
  shortcut?: Shortcut
  when?(c: CommandCtx): boolean
  checked?(c: CommandCtx): boolean
  paletteHidden?: boolean
  run(): void
}

export interface ResolvedCommand extends CommandLike {
  run(): void
}

/** 현재 앱 상태 스냅샷(팔레트를 열 때·단축키를 누를 때 읽는다) */
export function readCommandCtx(): CommandCtx {
  const app = useAppStore.getState()
  const ui = app.config?.ui
  return {
    hasRepo: !!app.activeRepoId,
    github: !!app.activeRepoId && !!useGithubStore.getState().ref,
    filtered: useLogStore.getState().scopeSig !== '',
    theme: ui?.theme ?? 'system',
    locale: ui?.locale ?? 'ko',
    diffMode: ui?.diffMode ?? 'unified',
    autoFetch: ui?.autoFetchMinutes ?? 5,
    minimapCollapsed: ui?.minimapCollapsed === true,
    initials: ui?.graphInitials !== false,
    sidebarShown: ui?.sidebarHidden !== true,
    detailShown: ui?.detailHidden !== true,
    sourceKind: sourceKindOf(app.activeRepoId),
    editor: ui?.editor ?? 'vscode'
  }
}

const setUi = (patch: Parameters<ReturnType<typeof useAppStore.getState>['setUi']>[0]): void => void useAppStore.getState().setUi(patch)

const THEMES: { v: ThemePref; key: MessageKey; kw: string }[] = [
  { v: 'system', key: 'theme.system', kw: 'theme system auto' },
  { v: 'dark', key: 'theme.dark', kw: 'theme dark night' },
  { v: 'light', key: 'theme.light', kw: 'theme light day' }
]
const LOCALES: { v: Locale; name: string; kw: string }[] = [
  { v: 'ko', name: '한국어', kw: 'language korean' },
  { v: 'en', name: 'English', kw: 'language english 영어' }
]
const AUTO_FETCH = [0, 1, 5, 15, 30]
const EDITORS: { v: EditorKind; key: MessageKey; kw: string }[] = [
  { v: 'vscode', key: 'editor.vscode', kw: 'editor vscode code' },
  { v: 'cursor', key: 'editor.cursor', kw: 'editor cursor' },
  { v: 'jetbrains', key: 'editor.jetbrains', kw: 'editor jetbrains intellij idea' },
  { v: 'custom', key: 'editor.custom', kw: 'editor custom' }
]
const EXTERNALS: { action: ExternalAction; key: MessageKey; kw: string }[] = [
  { action: 'fileManager', key: 'cmd.extFileManager', kw: 'open reveal file manager explorer finder folder' },
  { action: 'editor', key: 'cmd.extEditor', kw: 'open editor vscode code ide' },
  { action: 'terminal', key: 'cmd.extTerminal', kw: 'open terminal shell console' }
]

export const COMMAND_DEFS: CommandDef[] = [
  // ── 일반 ──
  {
    id: 'palette.open',
    category: 'general',
    title: (t) => t('cmd.palette'),
    shortcut: { code: 'KeyP', shift: true },
    paletteHidden: true,
    run: () => usePaletteStore.getState().setPaletteOpen(true)
  },
  {
    id: 'repo.switch',
    category: 'general',
    title: (t) => t('toolbar.switchRepo'),
    keywords: 'repository switch repo',
    shortcut: { code: 'KeyP' },
    run: () => useAppStore.getState().setSwitcherOpen(true)
  },
  {
    id: 'launchpad.open',
    category: 'general',
    title: (t) => t('cmd.launchpad'),
    keywords: 'launchpad dashboard pull requests issues all repositories',
    run: () => useLaunchpadStore.getState().show(true)
  },
  {
    id: 'repo.add',
    category: 'general',
    title: (t) => t('cmd.addRepo'),
    keywords: 'add repository open clone',
    run: () => useAppStore.getState().openAddDialog(true)
  },
  {
    id: 'update.check',
    category: 'general',
    title: (t) => t('update.check'),
    keywords: 'update check version',
    run: () => void useUpdateStore.getState().check()
  },
  {
    id: 'update.notes',
    category: 'general',
    title: (t) => t('update.notes'),
    keywords: 'release notes changelog',
    run: () => usePaletteStore.getState().setNotesOpen(true)
  },
  ...THEMES.map(
    (th): CommandDef => ({
      id: `theme.${th.v}`,
      category: 'general',
      title: (t) => `${t('toolbar.themeMenu')}: ${t(th.key)}`,
      keywords: th.kw,
      run: () => useAppStore.getState().setTheme(th.v)
    })
  ),
  ...LOCALES.map(
    (l): CommandDef => ({
      id: `locale.${l.v}`,
      category: 'general',
      title: (t) => `${t('toolbar.localeMenu')}: ${l.name}`,
      keywords: l.kw,
      run: () => useAppStore.getState().setLocale(l.v)
    })
  ),
  ...EXTERNALS.map(
    (x): CommandDef => ({
      id: `ext.${x.action}`,
      category: 'general',
      title: (t) => t(x.key),
      keywords: x.kw,
      when: (c) => c.hasRepo && externalSupported(c.sourceKind, x.action),
      run: () => {
        const id = useAppStore.getState().activeRepoId
        if (id) void openExternal(id, x.action)
      }
    })
  ),
  {
    // GitHub 원격 전용 저장소는 폴더·에디터·터미널 명령 대신 이것만 나온다
    id: 'ext.github',
    category: 'general',
    title: (t) => t('cmd.extGithub'),
    keywords: 'open github web browser repository',
    when: (c) => c.hasRepo && c.sourceKind === 'github',
    run: () => {
      const id = useAppStore.getState().activeRepoId
      if (id) void openOnGithub(id)
    }
  },
  ...EDITORS.map(
    (e): CommandDef => ({
      id: `editor.${e.v}`,
      category: 'general',
      title: (t) => t('cmd.editorPick', { name: t(e.key) }),
      keywords: e.kw,
      run: () => setUi({ editor: e.v })
    })
  ),
  {
    id: 'settings.open',
    category: 'general',
    title: (t) => t('cmd.settings'),
    keywords: 'settings preferences editor external tools',
    run: () => useAppStore.getState().openSettings(true)
  },
  {
    id: 'shortcuts.open',
    category: 'general',
    title: (t) => t('cmd.shortcuts'),
    keywords: 'keyboard shortcuts keys help',
    shortcut: { code: 'Slash' },
    run: () => usePaletteStore.getState().setShortcutsOpen(true)
  },

  // ── 커밋 목록 ──
  {
    id: 'log.search',
    category: 'list',
    title: (t) => t('toolbar.search'),
    keywords: 'find search commits',
    shortcut: { code: 'KeyF' },
    when: (c) => c.hasRepo,
    run: () => {
      // 검색 바는 커밋 그래프 쪽에 있으므로 diff 를 보고 있었다면 닫고 연다
      useDetailStore.getState().closeFile()
      useSearchStore.getState().show()
    }
  },
  {
    id: 'log.refresh',
    category: 'list',
    title: (t) => t('toolbar.refresh'),
    keywords: 'fetch reload refresh',
    when: (c) => c.hasRepo,
    run: () => void useFetchStore.getState().refresh()
  },
  ...AUTO_FETCH.map(
    (n): CommandDef => ({
      id: `autofetch.${n}`,
      category: 'list',
      title: (t) => `${t('fetch.autoMenu')}: ${n === 0 ? t('fetch.autoOff') : t('fetch.autoEvery', { n })}`,
      keywords: 'auto fetch interval',
      when: (c) => c.hasRepo,
      run: () => setUi({ autoFetchMinutes: n })
    })
  ),
  {
    id: 'log.clearFilter',
    category: 'list',
    title: (t) => t('cmd.clearFilter'),
    keywords: 'filter clear reset all branches',
    when: (c) => c.hasRepo && c.filtered,
    run: () => useLogStore.getState().setFilter(null)
  },
  {
    id: 'minimap.toggle',
    category: 'list',
    title: (t, c) => t(c.minimapCollapsed ? 'minimap.expand' : 'minimap.collapse'),
    keywords: 'minimap activity collapse expand',
    when: (c) => c.hasRepo,
    run: () => setUi({ minimapCollapsed: !readCommandCtx().minimapCollapsed })
  },
  {
    id: 'graph.initials',
    category: 'list',
    title: (t, c) => t(c.initials ? 'minimap.initialsOff' : 'minimap.initialsOn'),
    keywords: 'graph initials avatar author',
    when: (c) => c.hasRepo,
    run: () => setUi({ graphInitials: !readCommandCtx().initials })
  },
  {
    id: 'gh.pulls',
    category: 'list',
    title: (t) => t('cmd.ghPulls'),
    keywords: 'github pull requests pr',
    when: (c) => c.github,
    run: () => useGithubStore.getState().show({ kind: 'pulls' })
  },
  {
    id: 'gh.issues',
    category: 'list',
    title: (t) => t('cmd.ghIssues'),
    keywords: 'github issues',
    when: (c) => c.github,
    run: () => useGithubStore.getState().show({ kind: 'issues' })
  },
  {
    id: 'jira.connect',
    category: 'general',
    title: (t) => t('cmd.jiraConnect'),
    keywords: 'jira atlassian connect disconnect token',
    run: () => useJiraStore.getState().setDialogOpen(true)
  },
  {
    id: 'jira.disconnect',
    category: 'general',
    title: (t) => t('cmd.jiraDisconnect'),
    keywords: 'jira disconnect sign out',
    when: () => useJiraStore.getState().auth.connected,
    run: () => void useJiraStore.getState().disconnect().catch(() => {})
  },
  {
    id: 'jira.mine',
    category: 'list',
    title: (t) => t('cmd.jiraMine'),
    keywords: 'jira assigned issues mine',
    when: () => useJiraStore.getState().auth.connected,
    run: () => useJiraStore.getState().show({ kind: 'list', source: 'mine' })
  },

  // ── diff ──
  {
    id: 'diff.unified',
    category: 'diff',
    title: (t) => `${t('diff.mode')}: ${t('diff.unified')}`,
    keywords: 'diff unified inline',
    when: (c) => c.hasRepo,
    run: () => setUi({ diffMode: 'unified' })
  },
  {
    id: 'diff.split',
    category: 'diff',
    title: (t) => `${t('diff.mode')}: ${t('diff.split')}`,
    keywords: 'diff split side by side',
    when: (c) => c.hasRepo,
    run: () => setUi({ diffMode: 'split' })
  },

  // ── 패널 ──
  {
    id: 'panel.sidebar',
    category: 'panel',
    title: (t, c) => t(c.sidebarShown ? 'toolbar.hideSidebar' : 'toolbar.showSidebar'),
    keywords: 'sidebar panel toggle',
    shortcut: { code: 'KeyB' },
    run: () => toggleSidebar()
  },
  {
    id: 'panel.detail',
    category: 'panel',
    title: (t, c) => t(c.detailShown ? 'toolbar.hideDetail' : 'toolbar.showDetail'),
    keywords: 'detail panel toggle',
    shortcut: { code: 'KeyB', alt: true },
    when: (c) => c.hasRepo,
    run: () => toggleDetail()
  }
]

/** 현재 상태 표시(체크)를 계산하는 선택지형 명령 */
function isChecked(id: string, c: CommandCtx): boolean {
  if (id.startsWith('theme.')) return c.theme === id.slice(6)
  if (id.startsWith('locale.')) return c.locale === id.slice(7)
  if (id.startsWith('autofetch.')) return c.autoFetch === Number(id.slice(10))
  if (id.startsWith('editor.')) return c.editor === id.slice(7)
  if (id === 'diff.unified') return c.diffMode === 'unified'
  if (id === 'diff.split') return c.diffMode === 'split'
  return false
}

/** 레지스트리를 현재 언어·상태로 풀어 낸다. 숨겨야 할 명령도 available=false 로 남긴다(단축키 목록은 전부 보여 준다) */
export function resolveCommands(t: T, ctx: CommandCtx): ResolvedCommand[] {
  return COMMAND_DEFS.map((d) => ({
    id: d.id,
    category: d.category,
    label: d.title(t, ctx),
    keywords: d.keywords,
    available: d.when ? d.when(ctx) : true,
    paletteHidden: d.paletteHidden,
    shortcut: d.shortcut,
    checked: isChecked(d.id, ctx),
    run: d.run
  }))
}

/** 전역 단축키: 이벤트에 맞는 명령을 찾아 상황에 맞으면 실행한다. 단축키가 정의된 키 조합이면 true(브라우저 기본 동작을 막는다) */
export function runShortcut(e: KeyEventLike): boolean {
  const def = COMMAND_DEFS.find((d) => d.shortcut && matchesShortcut(e, d.shortcut))
  if (!def) return false
  if (!def.when || def.when(readCommandCtx())) def.run()
  return true
}

/** 단축키 목록에만 보이는 항목(명령이 아니라 각 화면이 직접 처리하는 키). keys 의 각 원소는 대안 조합 하나 */
export interface ExtraShortcut {
  id: string
  category: CommandCategory
  title: MessageKey
  chords: string[][]
}

export const EXTRA_SHORTCUTS: ExtraShortcut[] = [
  { id: 'list.move', category: 'list', title: 'sc.move', chords: [['↑'], ['↓']] },
  { id: 'list.page', category: 'list', title: 'sc.page', chords: [['PageUp'], ['PageDown']] },
  { id: 'list.edge', category: 'list', title: 'sc.edge', chords: [['Home'], ['End']] },
  { id: 'search.step', category: 'list', title: 'sc.searchStep', chords: [['Enter'], ['Shift', 'Enter']] },
  { id: 'search.close', category: 'list', title: 'sc.searchClose', chords: [['Esc']] },
  { id: 'diff.close', category: 'diff', title: 'sc.diffClose', chords: [['Esc'], ['Mouse4']] },
  { id: 'diff.step', category: 'diff', title: 'sc.diffStep', chords: [['Alt', '↑'], ['Alt', '↓']] },
  { id: 'panel.files', category: 'panel', title: 'sc.files', chords: [['↑'], ['↓'], ['Enter']] },
  { id: 'general.gh', category: 'general', title: 'sc.ghBack', chords: [['Esc']] },
  { id: 'general.popup', category: 'general', title: 'sc.popup', chords: [['↑'], ['↓'], ['Enter'], ['Esc']] }
]
