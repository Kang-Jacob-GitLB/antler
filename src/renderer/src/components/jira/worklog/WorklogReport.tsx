import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../../i18n'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IconChevronDown, IconChevronRight, IconCopy } from '../../icons'
import { IssueKey, userColor, type WorklogPaneProps } from './WorklogBits'
import { buildReport, dayStatus, fmtDuration, isWeekend, parseYmd, reportToTsv, weekdayShort, type ReportRow } from './worklogModel'

const shortDate = (ymd: string): string => `${parseYmd(ymd).getMonth() + 1}/${parseYmd(ymd).getDate()}`

/** 팀 시간표: 행 = 사용자(또는 이슈), 열 = 날짜, 오른쪽 합계 열, 아래 합계 행. 가로 스크롤은 표 영역 안에서만, 첫 열은 고정 */
export function WorklogReport({ days, logs, users, today, locale }: WorklogPaneProps): React.JSX.Element {
  const t = useT()
  // 묶음 기준·접힘은 스토어에 둔다(이슈를 열었다 돌아와도 유지)
  const group = useWorklogStore((s) => s.reportGroup)
  const collapsedIds = useWorklogStore((s) => s.collapsed)
  const { setReportGroup: setGroup, toggleCollapsed: toggle, setCollapsed } = useWorklogStore.getState()
  const collapsed = useMemo(() => new Set(collapsedIds), [collapsedIds])
  const scroller = useRef<HTMLDivElement>(null)
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])

  const report = useMemo(() => buildReport(logs, users, group), [logs, users, group])
  const expandable = report.rows.filter((r) => r.children.length > 0).map((r) => r.id)
  const allCollapsed = expandable.length > 0 && expandable.every((id) => collapsed.has(id))

  // 오늘이 범위 안이면 오늘 열이 보이게 가로로 맞춘다(월간처럼 표가 넓을 때)
  const first = days[0]
  useEffect(() => {
    const box = scroller.current
    const th = box?.querySelector<HTMLElement>('thead th.is-today')
    if (!box || !th || box.scrollWidth <= box.clientWidth) return
    const lead = box.querySelector<HTMLElement>('thead .wl-t__first')?.offsetWidth ?? 0
    const tail = box.querySelector<HTMLElement>('thead .wl-t__sum')?.offsetWidth ?? 0
    box.scrollLeft = Math.max(0, th.offsetLeft - lead - (box.clientWidth - lead - tail - th.offsetWidth) / 2)
  }, [first, days.length, today])

  const copy = (): void => {
    const tsv = reportToTsv(report, days, { name: t('jira.worklog.col.name'), total: t('jira.worklog.total') })
    void navigator.clipboard
      .writeText(tsv)
      .then(() => {
        setCopied(true)
        clearTimeout(timer.current)
        timer.current = setTimeout(() => setCopied(false), 1500)
      })
      .catch(() => {})
  }

  const cell = (row: ReportRow, ymd: string): React.JSX.Element => {
    const sec = row.byDay[ymd] ?? 0
    const status = row.target ? dayStatus(ymd, sec, row.target, today) : 'none'
    return (
      <td key={ymd} className={`wl-t__num${isWeekend(ymd) ? ' is-weekend' : ''}${ymd === today ? ' is-today' : ''}${status === 'under' ? ' is-under' : ''}${status === 'over' ? ' is-over' : ''}${sec === 0 ? ' is-zero' : ''}`}>
        {sec > 0 ? fmtDuration(sec) : '–'}
      </td>
    )
  }

  const label = (r: ReportRow): React.JSX.Element =>
    r.issue ? (
      <span className="wl-t__issue">
        <IssueKey issueKey={r.issue.key} title={`${r.issue.key} ${r.issue.summary}`} />
        <span className="wl-t__summary" title={r.issue.summary}>
          {r.issue.summary}
        </span>
      </span>
    ) : (
      <span className="wl-t__who">
        <span className="wl-user__dot" style={{ background: userColor(r.userId ?? r.id) }} />
        <span className="wl-t__name">{r.label}</span>
      </span>
    )

  const rowEl = (r: ReportRow, child: boolean): React.JSX.Element => {
    const open = !collapsed.has(r.id)
    return (
      <Fragment key={r.id}>
        <tr className={child ? 'wl-t__child' : 'wl-t__top'}>
          <th scope="row" className="wl-t__first">
            <div className={`wl-t__label${child ? ' is-child' : ''}`}>
              {!child && r.children.length > 0 ? (
                <button className="wl-t__fold" aria-expanded={open} aria-label={r.label || r.issue?.key} onClick={() => toggle(r.id)}>
                  {open ? <IconChevronDown size={12} /> : <IconChevronRight size={12} />}
                </button>
              ) : (
                <span className="wl-t__fold" />
              )}
              {label(r)}
            </div>
          </th>
          {days.map((d) => cell(r, d))}
          <td className="wl-t__sum">{r.total > 0 ? fmtDuration(r.total) : '–'}</td>
        </tr>
        {!child && open && r.children.map((c) => rowEl(c, true))}
      </Fragment>
    )
  }

  return (
    <div className="wl-rpt">
      <div className="wl-rpt__tools">
        <div className="seg" role="radiogroup" aria-label={t('jira.worklog.group')}>
          {(['user', 'issue'] as const).map((g) => (
            <button key={g} role="radio" aria-checked={group === g} className={`seg__btn${group === g ? ' is-on' : ''}`} onClick={() => setGroup(g)}>
              {t(`jira.worklog.group.${g}`)}
            </button>
          ))}
        </div>
        {expandable.length > 0 && (
          <button className="btn btn--sm" onClick={() => setCollapsed(allCollapsed ? [] : expandable)}>
            {allCollapsed ? t('jira.worklog.expandAll') : t('jira.worklog.collapseAll')}
          </button>
        )}
        <span className="wl-rpt__legend gh-muted">{t('jira.worklog.legend')}</span>
        <span className="wl-spacer" />
        <button className="btn btn--sm" onClick={copy}>
          <IconCopy size={13} />
          {copied ? t('jira.worklog.copied') : t('jira.worklog.copy')}
        </button>
      </div>
      <div className="wl-rpt__scroll" ref={scroller}>
        <table className="wl-t">
          <thead>
            <tr>
              <th className="wl-t__first wl-t__corner" scope="col">
                {t('jira.worklog.col.name')}
              </th>
              {days.map((d) => (
                <th key={d} scope="col" className={`wl-t__day${isWeekend(d) ? ' is-weekend' : ''}${d === today ? ' is-today' : ''}`}>
                  <span className="wl-t__dow">{weekdayShort(d, locale)}</span>
                  <span className="wl-t__date">{shortDate(d)}</span>
                </th>
              ))}
              <th scope="col" className="wl-t__sum">
                {t('jira.worklog.total')}
              </th>
            </tr>
          </thead>
          <tbody>{report.rows.map((r) => rowEl(r, false))}</tbody>
          <tfoot>
            <tr>
              <th scope="row" className="wl-t__first">
                <div className="wl-t__label">
                  <span className="wl-t__fold" />
                  <strong>{t('jira.worklog.total')}</strong>
                </div>
              </th>
              {days.map((d) => cell(report.total, d))}
              <td className="wl-t__sum">{report.total.total > 0 ? fmtDuration(report.total.total) : '–'}</td>
            </tr>
          </tfoot>
        </table>
      </div>
    </div>
  )
}
