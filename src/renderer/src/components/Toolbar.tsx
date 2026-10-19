import { useState } from 'react'
import type { FetchStatus } from '@shared/types'
import { useT } from '../i18n'
import { fetchDetail } from '../lib/fetchText'
import { toggleDetail, toggleSidebar, useActiveRepo, useAppStore } from '../stores/appStore'
import { useFetchStore } from '../stores/fetchStore'
import { useLogStore } from '../stores/logStore'
import { GhAuthButton } from './github/GhAuthButton'
import { AntlerMark } from './brand'
import { EnvBadge } from './EnvBadge'
import { MenuButton } from './MenuButton'
import {
  IconBranch,
  IconChevronDown,
  IconClose,
  IconFunnel,
  IconPanelLeft,
  IconPanelRight,
  IconRefresh
} from './icons'
import './Toolbar.css'

export function Toolbar(): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const source = useAppStore((s) => s.config?.sources.find((x) => x.id === repo?.sourceId))
  const head = useAppStore((s) => s.head)
  const autoFetch = useAppStore((s) => s.config?.ui.autoFetchMinutes ?? 5)
  const { setUi, setSwitcherOpen } = useAppStore.getState()
  const sidebarShown = useAppStore((s) => s.config?.ui.sidebarHidden !== true)
  const detailShown = useAppStore((s) => s.config?.ui.detailHidden !== true)
  const fetchStatus = useFetchStore((s) => (repo ? s.status[repo.id] : undefined))
  // 툴팁의 "n분 전"은 마우스를 올릴 때 갱신한다(타이머 없이)
  const [now, setNow] = useState(() => Date.now())
  const filter = useLogStore((s) => s.filter)
  const setFilter = useLogStore((s) => s.setFilter)

  const fetchTitle = (s: FetchStatus | undefined, at: number): string => {
    const base = t('toolbar.refresh')
    return s ? `${base}\n${fetchDetail(t, s, at)}` : base
  }

  const mod = window.antler.platform === 'darwin' ? '⌘' : 'Ctrl+'
  const altKey = window.antler.platform === 'darwin' ? '⌥' : 'Alt+'

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
        <div className="panel-toggles">
          <button
            className="icon-btn"
            aria-pressed={sidebarShown}
            title={`${t(sidebarShown ? 'toolbar.hideSidebar' : 'toolbar.showSidebar')} (${mod}B)`}
            onClick={() => toggleSidebar()}
          >
            <IconPanelLeft on={sidebarShown} />
          </button>
          {repo && (
            <button
              className="icon-btn"
              aria-pressed={detailShown}
              title={`${t(detailShown ? 'toolbar.hideDetail' : 'toolbar.showDetail')} (${mod}${altKey}B)`}
              onClick={() => toggleDetail()}
            >
              <IconPanelRight on={detailShown} />
            </button>
          )}
        </div>
      </div>
    </header>
  )
}
