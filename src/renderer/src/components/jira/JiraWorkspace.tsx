import type { JiraSavedFilter } from '@shared/types'
import { useT } from '../../i18n'
import { useJiraSiteData, useJiraStore, type JiraView as View } from '../../stores/jiraStore'
import { IconArrowLeft, IconClose, IconCloud, IconFunnel, IconHistory, IconJira, IconPlus, IconUser } from '../icons'
import { JiraView } from './JiraView'
import './JiraWorkspace.css'

const NO_FILTERS: JiraSavedFilter[] = []

/**
 * Jira 작업 공간: 툴바의 Jira 버튼으로 열고, 창 전체(사이드바 포함)를 덮는다.
 * 머리 = 뒤로가기 · 사이트 · 연결 관리 / 왼쪽 = 나에게 할당 · JQL 검색 · 저장 필터 · 최근 본 이슈 / 본문 = 목록·이슈(JiraView).
 * 뒤로가기(버튼 · Esc · 마우스 뒤로)는 이슈 → 목록 → 아래 화면 순으로 물러난다.
 */
export function JiraWorkspace(): React.JSX.Element {
  const t = useT()
  const auth = useJiraStore((s) => s.auth)
  const view = useJiraStore((s) => s.view)
  const { show } = useJiraStore.getState()
  const back = (): void => show(view?.kind === 'issue' ? view.from : null)
  const host = auth.siteUrl ? auth.siteUrl.replace(/^https?:\/\//, '') : ''

  return (
    <section className="jira-ws" aria-label="Jira">
      <header className="jira-ws__head">
        <button className="btn btn--sm jira-ws__back" title={`${t('jira.back')} (Esc)`} onClick={back}>
          <IconArrowLeft size={13} />
          {t('jira.back')}
        </button>
        <span className="jira-ws__brand">
          <IconJira size={16} />
          Jira
        </span>
        {host && <span className="jira-ws__site">{host}</span>}
        <span className="jira-ws__spacer" />
        <button className="btn btn--sm" onClick={() => useJiraStore.getState().setDialogOpen(true)}>
          <IconCloud size={13} />
          {auth.connected ? t('jira.manage') : t('jira.connect')}
        </button>
      </header>
      {auth.connected ? (
        <div className="jira-ws__body">
          <Nav view={view} />
          <div className="jira-ws__main">
            <JiraView />
          </div>
        </div>
      ) : (
        <div className="jira-ws__empty">
          <IconJira size={40} />
          <p>{t('jira.notConnected')}</p>
          <button className="btn btn--primary" onClick={() => useJiraStore.getState().setDialogOpen(true)}>
            {t('jira.connect')}
          </button>
        </div>
      )}
    </section>
  )
}

function Nav({ view }: { view: View | null }): React.JSX.Element {
  const t = useT()
  const mine = useJiraStore((s) => s.mine)
  const site = useJiraSiteData()
  const filters = site.filters ?? NO_FILTERS
  const recent = site.recent ?? []
  const { show, openIssue, removeFilter } = useJiraStore.getState()
  const listOf = (v: View | null): View | null => (v?.kind === 'issue' ? v.from : v)
  const cur = listOf(view)
  const isList = (source: 'mine' | 'custom' | number): boolean => cur?.kind === 'list' && cur.source === source

  return (
    <nav className="jira-ws__nav">
      <NavItem icon={<IconUser size={14} />} label={t('jira.mine')} count={mine.items.length} active={isList('mine')} onClick={() => show({ kind: 'list', source: 'mine' })} />
      <NavItem icon={<IconPlus size={14} />} label={t('jira.customJql')} active={isList('custom')} onClick={() => show({ kind: 'list', source: 'custom' })} />

      <div className="jira-ws__group">{t('jira.filters')}</div>
      {filters.length === 0 && <div className="jira-ws__none">{t('jira.filtersEmpty')}</div>}
      {filters.map((f, n) => (
        <NavItem
          key={f.name}
          icon={<IconFunnel size={14} />}
          label={f.name}
          title={f.jql}
          active={isList(n)}
          onClick={() => show({ kind: 'list', source: n })}
          trailing={
            <button
              className="jira-ws__remove"
              title={t('jira.removeFilter')}
              aria-label={t('jira.removeFilter')}
              onClick={(e) => {
                e.stopPropagation()
                removeFilter(n)
              }}
            >
              <IconClose size={11} />
            </button>
          }
        />
      ))}

      <div className="jira-ws__group">{t('jira.recent')}</div>
      {recent.length === 0 && <div className="jira-ws__none">{t('jira.recentEmpty')}</div>}
      {recent.map((r) => (
        <NavItem
          key={r.key}
          icon={<IconHistory size={14} />}
          label={r.summary}
          title={`${r.key} ${r.summary}`}
          sub={r.key}
          active={view?.kind === 'issue' && view.key === r.key}
          onClick={() => openIssue(r.key)}
        />
      ))}
    </nav>
  )
}

function NavItem({
  icon,
  label,
  title,
  count,
  sub,
  active,
  trailing,
  onClick
}: {
  icon: React.ReactNode
  label: string
  title?: string
  count?: number
  sub?: string
  active?: boolean
  trailing?: React.ReactNode
  onClick(): void
}): React.JSX.Element {
  return (
    <div
      role="button"
      tabIndex={0}
      className={`jira-ws__item${active ? ' is-active' : ''}`}
      title={title ?? label}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onClick()
        }
      }}
    >
      <span className="jira-ws__icon">{icon}</span>
      <span className="jira-ws__label">{label}</span>
      {sub && <span className="jira-ws__sub mono">{sub}</span>}
      {count !== undefined && <span className="side-count">{count}</span>}
      {trailing}
    </div>
  )
}
