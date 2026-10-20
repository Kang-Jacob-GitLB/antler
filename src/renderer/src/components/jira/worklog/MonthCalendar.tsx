import { useMemo } from 'react'
import { useT } from '../../../i18n'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IssueKey, type WorklogPaneProps } from './WorklogBits'
import { bucketByDay, DAY_TARGET_SEC, dayStatus, fmtDuration, isWeekend, parseYmd, weekdayShort } from './worklogModel'

const MAX_CHIPS = 3

/** 월 캘린더: 월~일 7열. 셀 = 날짜 · 그날 합계 · 이슈별 칩 · 하루 기준 대비 막대 */
export function MonthCalendar({ gridDays, logs, users, anchor, today, locale }: WorklogPaneProps): React.JSX.Element {
  const t = useT()
  const buckets = useMemo(() => bucketByDay(logs), [logs])
  const month = anchor.slice(0, 7)
  const target = DAY_TARGET_SEC * Math.max(1, users.length)
  const { openDay } = useWorklogStore.getState()

  return (
    <div className="wl-month">
      <div className="wl-month__head" aria-hidden="true">
        {gridDays.slice(0, 7).map((d) => (
          <div key={d} className={`wl-month__dow${isWeekend(d) ? ' is-weekend' : ''}`}>
            {weekdayShort(d, locale)}
          </div>
        ))}
      </div>
      <div className="wl-month__grid">
        {gridDays.map((d) => {
          const b = buckets.get(d)
          const sec = b?.seconds ?? 0
          const inMonth = d.startsWith(month)
          const status = inMonth ? dayStatus(d, sec, target, today) : 'none'
          const date = parseYmd(d)
          const label = date.getDate() === 1 ? (locale === 'ko' ? `${date.getMonth() + 1}월 1일` : date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })) : String(date.getDate())
          const hidden = (b?.issues.length ?? 0) - MAX_CHIPS
          const goDay = (): void => openDay(d)
          return (
            <div
              key={d}
              role="button"
              tabIndex={0}
              aria-label={t('jira.worklog.goDay', { date: d })}
              className={`wl-cell${isWeekend(d) ? ' is-weekend' : ''}${d === today ? ' is-today' : ''}${inMonth ? '' : ' is-other'} wl-cell--${status}`}
              onClick={goDay}
              onKeyDown={(e) => {
                if (e.target === e.currentTarget && (e.key === 'Enter' || e.key === ' ')) {
                  e.preventDefault()
                  goDay()
                }
              }}
            >
              <div className="wl-cell__top">
                <span className="wl-cell__date">{label}</span>
                {sec > 0 && <span className="wl-cell__sum">{fmtDuration(sec)}</span>}
              </div>
              <div className="wl-cell__chips">
                {b?.issues.slice(0, MAX_CHIPS).map((i) => (
                  <span key={i.key} className="wl-chip" title={`${i.key} ${i.summary}`}>
                    <IssueKey issueKey={i.key} title={`${i.key} ${i.summary}`} />
                    <span className="wl-chip__time">{fmtDuration(i.seconds)}</span>
                  </span>
                ))}
                {hidden > 0 && (
                  <button
                    className="wl-more"
                    onClick={(e) => {
                      e.stopPropagation()
                      goDay()
                    }}
                  >
                    {t('jira.worklog.more', { n: hidden })}
                  </button>
                )}
              </div>
              <div className="wl-cell__bar" aria-hidden="true">
                <span style={{ width: `${Math.min(100, (sec / target) * 100)}%` }} />
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}

