import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../../../i18n'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IssueKey, type WorklogPaneProps } from './WorklogBits'
import { bucketByDay, DAY_TARGET_SEC, dayStatus, fmtDuration, isWeekend, issueColorVar, parseYmd, weekdayShort } from './worklogModel'

/** 셀에서 날짜 줄·여백·막대가 차지하는 높이(px)와 칩 한 줄(칩 20 + 간격 2) */
const CELL_CHROME_PX = 54
const CHIP_ROW_PX = 22

/** 월 캘린더: 일~토 7열. 셀 = 날짜 · 그날 합계 · 이슈별 칩 · 하루 기준 대비 막대 */
export function MonthCalendar({ gridDays, logs, users, anchor, today, locale }: WorklogPaneProps): React.JSX.Element {
  const t = useT()
  const buckets = useMemo(() => bucketByDay(logs), [logs])
  const month = anchor.slice(0, 7)
  const target = DAY_TARGET_SEC * Math.max(1, users.length)
  const { openDay } = useWorklogStore.getState()

  // 셀 높이에 맞춰 보일 칩 수를 정한다(좁은 창에서 '+N개 더'가 막대와 겹치지 않게)
  const grid = useRef<HTMLDivElement>(null)
  const [rowH, setRowH] = useState(104)
  useEffect(() => {
    const el = grid.current
    if (!el) return
    const measure = (): void => {
      const cell = el.firstElementChild as HTMLElement | null
      if (cell) setRowH(cell.offsetHeight)
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const slots = Math.max(1, Math.floor((rowH - CELL_CHROME_PX + 2) / CHIP_ROW_PX))

  return (
    <div className="wl-month">
      <div className="wl-month__head" aria-hidden="true">
        {gridDays.slice(0, 7).map((d) => (
          <div key={d} className={`wl-month__dow${isWeekend(d) ? ' is-weekend' : ''}`}>
            {weekdayShort(d, locale)}
          </div>
        ))}
      </div>
      <div className="wl-month__grid" ref={grid}>
        {gridDays.map((d) => {
          const b = buckets.get(d)
          const sec = b?.seconds ?? 0
          const inMonth = d.startsWith(month)
          const status = inMonth ? dayStatus(d, sec, target, today) : 'none'
          const date = parseYmd(d)
          const label = date.getDate() === 1 ? (locale === 'ko' ? `${date.getMonth() + 1}월 1일` : date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })) : String(date.getDate())
          const count = b?.issues.length ?? 0
          // 넘치면 마지막 한 칸은 '+N개 더'에 쓴다
          const shownN = count <= slots ? count : slots - 1
          const hidden = count - shownN
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
                {b?.issues.slice(0, shownN).map((i) => (
                  <span key={i.key} className="wl-chip" title={`${i.key} ${i.summary}`} style={{ '--wl-c': issueColorVar(i.key) } as React.CSSProperties}>
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

