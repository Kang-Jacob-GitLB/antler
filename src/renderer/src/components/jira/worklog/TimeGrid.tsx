import { useEffect, useMemo, useRef, useState } from 'react'
import type { JiraWorklog } from '@shared/types'
import { useT } from '../../../i18n'
import { useJiraStore } from '../../../stores/jiraStore'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IssueKey, makeColorOf, type WorklogPaneProps } from './WorklogBits'
import {
  bucketByDay,
  fmtClock,
  fmtDuration,
  hourRange,
  isWeekend,
  layoutBlocks,
  minutesOfDay,
  parseYmd,
  weekdayShort,
  type TimeBlock
} from './worklogModel'

/** 한 시간의 높이(px) */
const HOUR_H = 48

/** 시간 축 타임라인: 주간은 7열, 일간은 1열. started 와 seconds 로 블록을 놓고 겹치면 나란히 열을 나눈다 */
export function TimeGrid({ gridDays, logs, users, today, locale }: WorklogPaneProps): React.JSX.Element {
  const t = useT()
  const colorOf = useMemo(() => makeColorOf(), [])
  const { openDay } = useWorklogStore.getState()
  const single = gridDays.length === 1

  const perDay = useMemo(() => {
    const buckets = bucketByDay(logs)
    return gridDays.map((d) => ({ ymd: d, seconds: buckets.get(d)?.seconds ?? 0, blocks: layoutBlocks(buckets.get(d)?.logs ?? []) }))
  }, [logs, gridDays])
  const [h0, h1] = useMemo(() => hourRange(perDay.flatMap((d) => d.blocks)), [perDay])
  const hours = Array.from({ length: h1 - h0 }, (_, i) => h0 + i)

  // 지금 시각 줄(오늘 열에만). 1분마다 갱신
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000)
    return () => clearInterval(id)
  }, [])
  const nowMin = now.getHours() * 60 + now.getMinutes()

  // 처음 열 때 첫 항목(없으면 업무 시작 시각) 근처가 보이게
  const scroller = useRef<HTMLDivElement>(null)
  const firstStart = Math.min(...perDay.flatMap((d) => d.blocks.map((b) => b.start)), 9 * 60)
  useEffect(() => {
    const el = scroller.current
    if (el) el.scrollTop = Math.max(0, ((firstStart - h0 * 60) / 60) * HOUR_H - HOUR_H)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [gridDays[0], gridDays.length])

  return (
    <div className="wl-tg" ref={scroller} style={{ '--wl-days': gridDays.length, '--wl-hour-h': `${HOUR_H}px` } as React.CSSProperties}>
      <div className="wl-tg__head">
        <div className="wl-tg__corner" />
        {perDay.map((d) => {
          const date = parseYmd(d.ymd)
          const inner = (
            <>
              <span className="wl-tg__dow">{weekdayShort(d.ymd, locale)}</span>
              <span className="wl-tg__date">{date.getDate()}</span>
              {d.seconds > 0 && <span className="wl-tg__sum">{fmtDuration(d.seconds)}</span>}
            </>
          )
          const cls = `wl-tg__day${isWeekend(d.ymd) ? ' is-weekend' : ''}${d.ymd === today ? ' is-today' : ''}`
          return single ? (
            <div key={d.ymd} className={cls}>
              {inner}
            </div>
          ) : (
            <button key={d.ymd} className={cls} title={t('jira.worklog.goDay', { date: d.ymd })} onClick={() => openDay(d.ymd)}>
              {inner}
            </button>
          )
        })}
      </div>
      <div className="wl-tg__body" style={{ height: hours.length * HOUR_H }}>
        <div className="wl-tg__gutter">
          {hours.map((h) => (
            <div key={h} className="wl-tg__hour" style={{ height: HOUR_H }}>
              <span>{`${String(h).padStart(2, '0')}:00`}</span>
            </div>
          ))}
        </div>
        {perDay.map((d) => (
          <div key={d.ymd} className={`wl-tg__col${isWeekend(d.ymd) ? ' is-weekend' : ''}${d.ymd === today ? ' is-today' : ''}`}>
            {d.blocks.map((b) => (
              <Block key={b.log.id} b={b} h0={h0} color={colorOf(b.log.authorId)} multi={users.length > 1} />
            ))}
            {d.ymd === today && nowMin >= h0 * 60 && nowMin <= h1 * 60 && <div className="wl-tg__now" style={{ top: ((nowMin - h0 * 60) / 60) * HOUR_H }} />}
          </div>
        ))}
      </div>
    </div>
  )
}

function Block({ b, h0, color, multi }: { b: TimeBlock; h0: number; color: string; multi: boolean }): React.JSX.Element {
  const { log } = b
  const top = ((b.start - h0 * 60) / 60) * HOUR_H
  const height = ((b.end - b.start) / 60) * HOUR_H
  const tip = [`${log.issueKey} ${log.issueSummary}`, `${fmtClock(b.start)} – ${fmtClock(b.start + log.seconds / 60)} (${fmtDuration(log.seconds)})`, multi ? log.author : '', log.comment].filter(Boolean).join('\n')
  return (
    <button
      className="wl-block"
      title={tip}
      style={{ top, height: Math.max(height - 1, 14), left: `calc(${(b.col / b.cols) * 100}% + 1px)`, width: `calc(${100 / b.cols}% - 2px)`, '--wl-c': color } as React.CSSProperties}
      onClick={() => useJiraStore.getState().openIssue(log.issueKey)}
    >
      <span className="wl-block__line">
        <span className="wl-block__key mono">{log.issueKey}</span>
        <span className="wl-block__time">{fmtDuration(log.seconds)}</span>
      </span>
      <span className="wl-block__text">{log.issueSummary}</span>
    </button>
  )
}

/** 일간 보기의 그날 항목 목록: 시간 · 이슈 · 작성자 · 코멘트 */
export function DayList({ logs, users }: { logs: JiraWorklog[]; users: WorklogPaneProps['users'] }): React.JSX.Element {
  const t = useT()
  const colorOf = useMemo(() => makeColorOf(), [])
  const sorted = useMemo(() => [...logs].sort((a, b) => Date.parse(a.started) - Date.parse(b.started)), [logs])
  return (
    <section className="wl-dl" aria-label={t('jira.worklog.dayList')}>
      <h3 className="wl-dl__title">{t('jira.worklog.dayList')}</h3>
      {sorted.length === 0 && <p className="gh-muted">{t('jira.worklog.empty')}</p>}
      <ul className="wl-dl__list">
        {sorted.map((w) => {
          const start = minutesOfDay(w.started)
          return (
            <li key={w.id} className="wl-dl__item" style={{ '--wl-c': colorOf(w.authorId) } as React.CSSProperties}>
              <div className="wl-dl__time">
                <span className="wl-dl__range">{`${fmtClock(start)} – ${fmtClock(start + w.seconds / 60)}`}</span>
                <span className="wl-dl__dur">{fmtDuration(w.seconds)}</span>
              </div>
              <div className="wl-dl__main">
                <div className="wl-dl__issue">
                  <IssueKey issueKey={w.issueKey} />
                  <span className="wl-dl__summary">{w.issueSummary}</span>
                </div>
                <div className="wl-dl__author">{w.author}</div>
                <div className={`wl-dl__comment${w.comment ? '' : ' is-none'}`}>{w.comment || t('jira.worklog.noComment')}</div>
              </div>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
