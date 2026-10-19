import { useEffect, useState } from 'react'
import type { Locale, ThemePref } from '@shared/types'
import { useT } from '../i18n'
import { fetchDetail } from '../lib/fetchText'
import { useActiveRepo, useAppStore } from '../stores/appStore'
import { useFetchStore } from '../stores/fetchStore'
import { useLogStore } from '../stores/logStore'
import { shortcutText } from '../lib/shortcuts'
import { usePaletteStore } from '../stores/paletteStore'
import { useGitflow, useRepoUsesLfs } from '../stores/repoExtrasStore'
import { useUpdateStore } from '../stores/updateStore'
import { LfsBadge } from './LfsBadge'
import { EnvBadge } from './EnvBadge'
import { MenuButton } from './MenuButton'
import { IconBranch, IconGlobe, IconMonitor, IconMoon, IconRefresh, IconSun, IconTerminal } from './icons'
import './StatusBar.css'

const RELEASES_URL = 'https://github.com/Kang-Jacob-GitLB/antler/releases'
/** "n분 전" 표시를 갱신하는 주기 */
const TICK_MS = 30_000

/**
 * 창 맨 아래 상태 표시줄(전체 폭, 평면). 왼쪽 = 저장소 환경·브랜치·커밋 수·fetch 상태,
 * 오른쪽 = 업데이트·릴리스 노트·테마·언어(메뉴는 위로 열린다). 드래그 영역이 아니다.
 */
export function StatusBar(): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const source = useAppStore((s) => s.config?.sources.find((x) => x.id === repo?.sourceId))
  const head = useAppStore((s) => s.head)
  const theme = useAppStore((s) => s.config?.ui.theme ?? 'system')
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { setTheme, setLocale } = useAppStore.getState()
  const fetchStatus = useFetchStore((s) => (repo ? s.status[repo.id] : undefined))
  const count = useLogStore((s) => s.commits.length)
  const hasMore = useLogStore((s) => s.hasMore)
  const usesLfs = useRepoUsesLfs()
  const flow = useGitflow()
  const appVersion =useUpdateStore((s) => s.state?.currentVersion ?? '')
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), TICK_MS)
    return () => clearInterval(id)
  }, [])

  const bad = !!fetchStatus && !fetchStatus.fetching && (!!fetchStatus.blocked || !!fetchStatus.error || !!fetchStatus.authPaused)
  const fetchShort = !fetchStatus
    ? ''
    : fetchStatus.fetching
      ? t('fetch.fetching')
      : fetchStatus.blocked
        ? t('fetch.noteBlocked')
        : fetchStatus.error || fetchStatus.authPaused
          ? t('fetch.noteFailed')
          : fetchDetail(t, fetchStatus, now)
  const ThemeIcon = theme === 'system' ? IconMonitor : theme === 'dark' ? IconMoon : IconSun

  return (
    <footer className="statusbar" aria-label={t('status.bar')}>
      <div className="statusbar__left">
        {repo && (
          <>
            {source && <EnvBadge kind={source.kind} />}
            <span className="statusbar__item" title={head?.hash ?? ''}>
              <IconBranch size={12} className="statusbar__icon" />
              <span className="statusbar__text">{head ? (head.branch ?? t('toolbar.detached')) : '…'}</span>
            </span>
            <span className="statusbar__item statusbar__count">
              {t('list.count', { n: count.toLocaleString(locale), more: hasMore ? '+' : '' })}
            </span>
            {usesLfs && (
              <span className="statusbar__item">
                <LfsBadge title={t('lfs.repo')} />
              </span>
            )}
            {flow?.active && (
              <span className="statusbar__item" title={t(flow.source === 'config' ? 'flow.statusConfig' : 'flow.statusDetected')}>
                <span className="statusbar__text">Gitflow</span>
              </span>
            )}
            {fetchShort && (
              <span
                className={`statusbar__item statusbar__fetch${bad ? ' is-bad' : ''}`}
                role="status"
                title={fetchDetail(t, fetchStatus, now)}
              >
                {fetchStatus?.fetching && <IconRefresh size={12} className="statusbar__spin" />}
                <span className="statusbar__text">{fetchShort}</span>
              </span>
            )}
          </>
        )}
      </div>

      <div className="statusbar__right">
        {appVersion && <span className="statusbar__ver">v{appVersion}</span>}
        <button
          className="statusbar__btn"
          title={`${t('palette.open')} (${shortcutText({ code: 'KeyP', shift: true }, window.antler.platform === 'darwin')})`}
          aria-label={t('palette.open')}
          onClick={() => usePaletteStore.getState().setPaletteOpen(true)}
        >
          <IconTerminal size={13} />
        </button>
        <button className="statusbar__btn" onClick={() => void useUpdateStore.getState().check()}>
          <IconRefresh size={12} />
          {t('update.check')}
        </button>
        <button
          className="statusbar__btn"
          onClick={() => void window.antler.invoke('gh:openExternal', RELEASES_URL).catch(() => {})}
        >
          <IconGlobe size={12} />
          {t('update.notes')}
        </button>
        <MenuButton<ThemePref>
          label={t('toolbar.themeMenu')}
          value={theme}
          onChange={setTheme}
          placement="top"
          className="statusbar__btn"
          options={[
            { value: 'system', label: t('theme.system'), icon: <IconMonitor size={14} /> },
            { value: 'light', label: t('theme.light'), icon: <IconSun size={14} /> },
            { value: 'dark', label: t('theme.dark'), icon: <IconMoon size={14} /> }
          ]}
        >
          <ThemeIcon size={13} />
        </MenuButton>
        <MenuButton<Locale>
          label={t('toolbar.localeMenu')}
          value={locale}
          onChange={setLocale}
          placement="top"
          className="statusbar__btn"
          options={[
            { value: 'ko', label: '한국어' },
            { value: 'en', label: 'English' }
          ]}
        >
          <IconGlobe size={13} />
          <span>{locale.toUpperCase()}</span>
        </MenuButton>
      </div>
    </footer>
  )
}
