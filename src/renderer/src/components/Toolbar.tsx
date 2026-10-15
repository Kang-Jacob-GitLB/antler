import { useT } from '../i18n'
import { useActiveRepo, useAppStore } from '../stores/appStore'
import { useLogStore } from '../stores/logStore'
import { useSearchStore } from '../stores/searchStore'
import { AntlerMark } from './brand'
import { EnvBadge } from './EnvBadge'
import {
  IconBranch,
  IconChevronDown,
  IconClose,
  IconFunnel,
  IconGlobe,
  IconMonitor,
  IconMoon,
  IconRefresh,
  IconSearch,
  IconSun
} from './icons'
import './Toolbar.css'

export function Toolbar(): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const source = useAppStore((s) => s.config?.sources.find((x) => x.id === repo?.sourceId))
  const head = useAppStore((s) => s.head)
  const theme = useAppStore((s) => s.config?.ui.theme ?? 'system')
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { cycleTheme, toggleLocale, refreshRefs, setSwitcherOpen } = useAppStore.getState()
  const reload = useLogStore((s) => s.reload)
  const filter = useLogStore((s) => s.filter)
  const setFilter = useLogStore((s) => s.setFilter)

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
        <button className="icon-btn" title={t('toolbar.theme', { mode: t(`theme.${theme}`) })} onClick={cycleTheme}>
          <ThemeIcon />
        </button>
        <button className="icon-btn icon-btn--text" title={t('toolbar.locale')} onClick={toggleLocale}>
          <IconGlobe />
          <span>{locale.toUpperCase()}</span>
        </button>
      </div>
    </header>
  )
}
