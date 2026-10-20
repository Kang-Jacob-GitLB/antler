import { useEffect, useMemo } from 'react'
import type { JiraUser } from '@shared/types'
import { useT } from '../../../i18n'
import { useAppStore } from '../../../stores/appStore'
import { useJiraSiteData, useJiraStore } from '../../../stores/jiraStore'
import { queryUsers, requestKey, useWorklogStore, type WorklogTab } from '../../../stores/jiraWorklogStore'
import { IconChevronRight, IconRefresh } from '../../icons'
import { MonthCalendar } from './MonthCalendar'
import { DayList, TimeGrid } from './TimeGrid'
import { WorklogReport } from './WorklogReport'
import { WorklogUsers } from './WorklogUsers'
import { filterWorklogs, fmtDuration, rangeFor, rangeLabel, selectedUsers, sumSeconds, todayYmd, type WorklogMode } from './worklogModel'
import './WorklogView.css'

const NO_TEAM: JiraUser[] = []
const TABS: WorklogTab[] = ['calendar', 'report']
const MODES: WorklogMode[] = ['day', 'week', 'month']

/** 워크로그: 캘린더(일·주·월)와 리포트(팀원별 시간표). 읽기 전용 — 조회만 한다 */
export function WorklogView(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const s = useWorklogStore()
  const team = useJiraSiteData().team ?? NO_TEAM
  const site = useJiraStore((x) => x.auth.siteUrl)
  const { setTab, setMode, shift, goToday, load } = useWorklogStore.getState()

  // 처음 열 때(이슈를 보고 돌아올 때 포함) 불러온다 — 받아 둔 결과가 신선하면 다시 묻지 않는다
  useEffect(() => {
    void load()
  }, [load, site])
  // 팀에서 지운 사람은 선택에서도 뺀다
  useEffect(() => useWorklogStore.getState().pruneTeam(team.map((u) => u.id)), [team])

  const range = useMemo(() => rangeFor(s.mode, s.anchor), [s.mode, s.anchor])
  const today = todayYmd()
  const users = useMemo(() => selectedUsers(s.me, s.includeMe, team, new Set(s.selectedTeam)), [s.me, s.includeMe, team, s.selectedTeam])
  const authors = useMemo(() => new Set(users.map((u) => u.id)), [users])

  // main 은 범위 앞뒤로 하루씩 더 준다 — 로컬 날짜로 다시 거른다
  const logs = useMemo(() => (s.data ? filterWorklogs(s.data.worklogs, range.from, range.to, authors) : []), [s.data, range.from, range.to, authors])
  const focus = useMemo(() => filterWorklogs(logs, range.days[0] ?? range.from, range.days[range.days.length - 1] ?? range.to, null), [logs, range.days])
  const total = sumSeconds(focus)

  const fresh = s.dataKey === requestKey(range.from, range.to, queryUsers(s), s.includeMe)
  const pane = { gridDays: range.gridDays, days: range.days, logs: s.tab === 'report' ? focus : logs, users, anchor: s.anchor, today, locale }
  const ready = !!s.data
  const invalid = fresh && s.data?.invalidUsers?.length ? s.data.invalidUsers.map((id) => team.find((u) => u.id === id)?.name ?? id) : []
  const dayLogs = s.mode === 'day' ? logs : []

  return (
    <>
      <header className="gh-head wl-head">
        <h2 className="wl-title">{t('jira.worklog.title')}</h2>
        <div className="seg" role="radiogroup" aria-label={t('jira.worklog.title')}>
          {TABS.map((k) => (
            <button key={k} role="radio" aria-checked={s.tab === k} className={`seg__btn${s.tab === k ? ' is-on' : ''}`} onClick={() => setTab(k)}>
              {t(`jira.worklog.tab.${k}`)}
            </button>
          ))}
        </div>
        <span className="wl-spacer" />
        <span className="wl-total" title={t('jira.worklog.total')}>
          {t('jira.worklog.total')} <strong>{fmtDuration(total)}</strong>
        </span>
        <button className="icon-btn icon-btn--sm" title={t('toolbar.refresh')} aria-label={t('toolbar.refresh')} disabled={s.loading} onClick={() => void load(true)}>
          <IconRefresh size={14} />
        </button>
      </header>

      <div className="gh-filters wl-bar">
        <div className="seg" role="radiogroup" aria-label={t('jira.worklog.mode')}>
          {MODES.map((m) => (
            <button key={m} role="radio" aria-checked={s.mode === m} className={`seg__btn${s.mode === m ? ' is-on' : ''}`} onClick={() => setMode(m)}>
              {t(`jira.worklog.mode.${m}`)}
            </button>
          ))}
        </div>
        <div className="wl-nav">
          <button className="icon-btn icon-btn--sm wl-nav__prev" title={t('jira.worklog.prev')} aria-label={t('jira.worklog.prev')} onClick={() => shift(-1)}>
            <IconChevronRight size={14} />
          </button>
          <button className="btn btn--sm" onClick={goToday}>
            {t('jira.worklog.today')}
          </button>
          <button className="icon-btn icon-btn--sm" title={t('jira.worklog.next')} aria-label={t('jira.worklog.next')} onClick={() => shift(1)}>
            <IconChevronRight size={14} />
          </button>
        </div>
        <strong className="wl-range" aria-live="polite">
          {rangeLabel(s.mode, s.anchor, locale)}
        </strong>
        <span className="wl-spacer" />
        <WorklogUsers team={team} me={s.me} />
      </div>

      {s.error && <div className="gh-notice is-error">{s.error}</div>}
      {s.data?.truncated && fresh && (
        <div className="wl-warn" role="status">
          {t('jira.worklog.truncated')}
        </div>
      )}

      {invalid.length > 0 && (
        <div className="wl-warn" role="status">
          {t('jira.worklog.invalidUsers', { names: invalid.join(', ') })}
        </div>
      )}

      {!ready && !s.error && <div className="gh-notice">{t('jira.worklog.loading')}</div>}
      {ready && (
        <div className={`wl-body${s.loading || !fresh ? ' is-loading' : ''}`} aria-busy={s.loading}>
          {s.loading && <div className="wl-loading gh-muted">{t('jira.worklog.loading')}</div>}
          {s.tab === 'report' ? (
            <WorklogReport {...pane} />
          ) : s.mode === 'month' ? (
            <MonthCalendar {...pane} />
          ) : s.mode === 'week' ? (
            <TimeGrid {...pane} />
          ) : (
            <div className="wl-day">
              <TimeGrid {...pane} />
              <DayList logs={dayLogs} users={users} />
            </div>
          )}
        </div>
      )}
    </>
  )
}
