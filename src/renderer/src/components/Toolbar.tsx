import { useState } from 'react'
import type { FetchStatus, Locale, ThemePref } from '@shared/types'
import { useT } from '../i18n'
import { useActiveRepo, useAppStore } from '../stores/appStore'
import { useFetchStore } from '../stores/fetchStore'
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
/** 새로고침 버튼 옆의 작은 상태 글자: 실패·차단일 때만 보인다(자세한 내용은 버튼 툴팁) */
function FetchNote({ status }: { status: FetchStatus | undefined }): React.JSX.Element | null {
  const t = useT()
  if (!status || status.fetching) return null
  const text = status.blocked ? t('fetch.noteBlocked') : status.error || status.authPaused ? t('fetch.noteFailed') : null
  return text ? (
    <span className="fetch-note" role="status">
      {text}
    </span>
  ) : null
}

export function Toolbar(): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const source = useAppStore((s) => s.config?.sources.find((x) => x.id === repo?.sourceId))
  const head = useAppStore((s) => s.head)
  const theme = useAppStore((s) => s.config?.ui.theme ?? 'system')
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const autoFetch = useAppStore((s) => s.config?.ui.autoFetchMinutes ?? 5)
  const { setTheme, setLocale, setUi, setSwitcherOpen } = useAppStore.getState()
  const fetchStatus = useFetchStore((s) => (repo ? s.status[repo.id] : undefined))
  // 툴팁의 "n분 전"은 마우스를 올릴 때 갱신한다(타이머 없이)
  const [now, setNow] = useState(() => Date.now())
  const filter = useLogStore((s) => s.filter)
  const setFilter = useLogStore((s) => s.setFilter)

  const appVersion = useUpdateStore((s) => s.state?.currentVersion ?? '')
  const fetchTitle = (s: FetchStatus | undefined, at: number): string => {
    const base = t('toolbar.refresh')
    if (!s) return base
    if (s.fetching) return `${base} — ${t('fetch.fetching')}`
    let detail: string
    if (s.blocked) detail = t(s.blocked.confirmable ? 'fetch.blocked' : 'fetch.blockedHard', { keys: s.blocked.keys.join(', ') })
    else if (s.error) detail = t('fetch.failed', { msg: s.error }) + (s.needsManual ? `\n${t('fetch.needsManual')}` : '')
    else if (s.authPaused) detail = t('fetch.authPaused')
    else if (s.noRemote) detail = t('fetch.noRemote')
    else if (s.lastOkAt) detail = t('fetch.last', { ago: agoText(at - s.lastOkAt) })
    else detail = t('fetch.never')
    return `${base}\n${detail}`
  }
  const agoText = (ms: number): string => {
    const min = Math.floor(Math.max(0, ms) / 60_000)
    if (min < 1) return t('fetch.agoNow')
    return min < 60 ? t('fetch.agoMin', { n: min }) : t('fetch.agoHour', { n: Math.floor(min / 60) })
  }

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
          <FetchNote status={fetchStatus} />
        )}
        {repo && (
          <div className="refresh-group">
            <button
              className={`icon-btn${fetchStatus?.fetching ? ' is-spinning' : ''}`}
              title={fetchTitle(fetchStatus, now)}
              aria-busy={fetchStatus?.fetching === true}
              onMouseEnter={() => setNow(Date.now())}
              onClick={() => void useFetchStore.getState().refresh()}
            >
              <IconRefresh />
            </button>
            <MenuButton<string>
              label={t('fetch.autoMenu')}
              value={String(autoFetch)}
              onChange={(v) => void setUi({ autoFetchMinutes: Number(v) })}
              className="icon-btn--chev"
              options={[
                { value: '0', label: t('fetch.autoOff') },
                ...[1, 5, 15, 30].map((n) => ({ value: String(n), label: t('fetch.autoEvery', { n }) }))
              ]}
            >
              <IconChevronDown size={12} />
            </MenuButton>
          </div>
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
