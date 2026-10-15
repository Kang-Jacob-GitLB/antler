import type { Locale, ThemePref } from '@shared/types'
import { useT } from '../i18n'
import { useActiveRepo, useAppStore } from '../stores/appStore'
import { useLogStore } from '../stores/logStore'
import { useSearchStore } from '../stores/searchStore'
import { GhAuthButton } from './github/GhAuthButton'
import { AntlerMark } from './brand'
import { EnvBadge } from './EnvBadge'
import { MenuButton } from './MenuButton'
import { useUpdateStore } from '../stores/updateStore'
import {
  IconBranch,
  IconChevronDown,
  IconClose,
  IconFunnel,
  IconGlobe,
  IconInfo,
  IconMonitor,
  IconMoon,
  IconRefresh,
  IconSearch,
  IconSun
} from './icons'
import './Toolbar.css'

const RELEASES_URL = 'https://github.com/Kang-Jacob-GitLB/antler/releases'

export function Toolbar(): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const source = useAppStore((s) => s.config?.sources.find((x) => x.id === repo?.sourceId))
  const head = useAppStore((s) => s.head)
  const theme = useAppStore((s) => s.config?.ui.theme ?? 'system')
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { setTheme, setLocale, refreshRefs, setSwitcherOpen } = useAppStore.getState()
  const reload = useLogStore((s) => s.reload)
  const filter = useLogStore((s) => s.filter)
  const setFilter = useLogStore((s) => s.setFilter)

  const appVersion = useUpdateStore((s) => s.state?.currentVersion ?? '')
  const ThemeIcon = theme === 'system' ? IconMonitor : theme === 'dark' ? IconMoon : IconSun
  const mod = window.antler.platform === 'darwin' ? '⌘' : 'Ctrl+'

  return (
    <header className="toolbar">
      <div className="toolbar__brand">
        <AntlerMark />
        <span className="toolbar__name">Antler</span>
      </div>

      {repo && (
        <div className="toolbar__crumbs">
          <button
            className="crumb"
            onClick={() => setSwitcherOpen(true)}
            title={`${t('toolbar.switchRepo')} (${mod}P)`}
          >
            {source && <EnvBadge kind={source.kind} />}
            <span className="crumb__label">{repo.name}</span>
            <IconChevronDown size={14} className="crumb__chev" />
          </button>
          <span className="toolbar__sep">/</span>
          <span className="crumb crumb--static" title={head?.hash ?? ''}>
            <IconBranch size={14} className="crumb__icon" />
            <span className="crumb__label">{head ? (head.branch ?? t('toolbar.detached')) : '…'}</span>
          </span>
          {filter && (
            <button className="filter-chip" title={t('toolbar.clearFilter')} onClick={() => setFilter(null)}>
              <IconFunnel size={12} />
              <span className="filter-chip__label">{t('toolbar.filter', { label: filter.label })}</span>
              <IconClose size={11} />
            </button>
          )}
        </div>
      )}

      <div className="toolbar__spacer" />

      <div className="toolbar__actions">
        <GhAuthButton />
        {repo && (
          <button className="icon-btn" title={`${t('toolbar.search')} (${mod}F)`} onClick={() => useSearchStore.getState().show()}>
            <IconSearch />
          </button>
        )}
        {repo && (
          <button
            className="icon-btn"
            title={t('toolbar.refresh')}
            onClick={() => {
              reload()
              void refreshRefs()
            }}
          >
            <IconRefresh />
          </button>
        )}
        <MenuButton<ThemePref>
          label={t('toolbar.themeMenu')}
          value={theme}
          onChange={setTheme}
          options={[
            { value: 'system', label: t('theme.system'), icon: <IconMonitor size={14} /> },
            { value: 'light', label: t('theme.light'), icon: <IconSun size={14} /> },
            { value: 'dark', label: t('theme.dark'), icon: <IconMoon size={14} /> }
          ]}
        >
          <ThemeIcon />
        </MenuButton>
        <MenuButton<'check' | 'releases' | ''>
          label={`${t('update.menu')} · v${appVersion}`}
          value=""
          onChange={(v) => {
            if (v === 'check') void useUpdateStore.getState().check()
            else if (v === 'releases') void window.antler.invoke('gh:openExternal', RELEASES_URL).catch(() => {})
          }}
          options={[
            { value: 'check', label: t('update.check'), icon: <IconRefresh size={14} /> },
            { value: 'releases', label: t('update.notes'), icon: <IconGlobe size={14} /> }
          ]}
        >
          <IconInfo />
        </MenuButton>
        <MenuButton<Locale>
          label={t('toolbar.localeMenu')}
          value={locale}
          onChange={setLocale}
          className="icon-btn--text"
          options={[
            { value: 'ko', label: '한국어' },
            { value: 'en', label: 'English' }
          ]}
        >
          <IconGlobe />
          <span>{locale.toUpperCase()}</span>
        </MenuButton>
      </div>
    </header>
  )
}
