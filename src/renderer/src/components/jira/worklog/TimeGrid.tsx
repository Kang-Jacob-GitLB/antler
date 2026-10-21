import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { JiraUser, JiraWorklog } from '@shared/types'
import { useT } from '../../../i18n'
import { useAppStore } from '../../../stores/appStore'
import { useJiraStore } from '../../../stores/jiraStore'
import { useWorklogStore } from '../../../stores/jiraWorklogStore'
import { IssueKey, makeColorOf, type WorklogPaneProps } from './WorklogBits'
import {
  bucketByDay,
  dayHoursOf,
  fmtClock,
  fmtDuration,
  hourRange,
  isWeekend,
  issueColorVar,
  lunchBand,
  layoutBlocks,
  localYmd,
  minutesOfDay,
  parseYmd,
  weekdayShort,
  type TimeBlock
} from './worklogModel'

/** 한 시간의 높이(px): 표시 시간 수로 보이는 높이를 나눠 정하되 이보다 작아지지 않는다(그땐 스크롤). 측정 전 기본값은 DEFAULT */
const MIN_HOUR_H = 28
const DEFAULT_HOUR_H = 48
/** 블록 폭(px) 기준: 이보다 좁으면 제목을 숨기고, 더 좁으면 시간만 세로로 */
const NARROW_PX = 72
const TINY_PX = 40
const MIN_COL_PX = 96
/** 코멘트 한 줄의 높이(px) = 글자 크기 11 × 줄 간격 1.25 = 13.75 → 14 */
const COMMENT_LINE_PX = 14
/** 블록 안 줄 사이 간격(flex gap 1px)과 위아래 패딩(2px×2) */
const BLOCK_GAP_PX = 1
const BLOCK_PAD_PX = 4
const GUTTER_PX = 52

/** 이슈를 보고 돌아왔을 때 스크롤 위치를 되살리기 위한 기억(범위 키별 하나) */
let savedScroll: { key: string; top: number } | null = null

/** 시간 축 타임라인: 주간은 날짜 7열, 일간은 1열(사용자가 둘 이상이면 사용자별 열). started 와 seconds 로 블록을 놓고 겹치면 나란히 열을 나눈다 */
export function TimeGrid({ gridDays, logs, users, today, locale }: WorklogPaneProps): React.JSX.Element {
  const t = useT()
  const colorOf = useMemo(() => makeColorOf(), [])
  const { openDay } = useWorklogStore.getState()
  const single = gridDays.length === 1
  // 일간 + 사용자 둘 이상 → 사용자별 열(리소스 뷰). 그 밖에는 날짜별 열
  const byUser = single && users.length >= 2

  const perCol = useMemo(() => {
    if (byUser) {
      const day = gridDays[0] ?? ''
      return users.map((u) => {
        const mine = logs.filter((w) => w.authorId === u.id)
        return { key: u.id, ymd: day, user: u, seconds: mine.reduce((n, w) => n + w.seconds, 0), blocks: layoutBlocks(mine) }
      })
    }
    const buckets = bucketByDay(logs)
    return gridDays.map((d) => ({ key: d, ymd: d, user: null as JiraUser | null, seconds: buckets.get(d)?.seconds ?? 0, blocks: layoutBlocks(buckets.get(d)?.logs ?? []) }))
  }, [logs, gridDays, users, byUser])
  const rawStart = useAppStore((s) => s.config?.ui.worklogDayStart)
  const rawEnd = useAppStore((s) => s.config?.ui.worklogDayEnd)
  const [dayStart, dayEnd] = dayHoursOf(rawStart, rawEnd)
  const [h0, h1] = useMemo(() => hourRange(perCol.flatMap((d) => d.blocks), [dayStart, dayEnd]), [perCol, dayStart, dayEnd])
  const hours = Array.from({ length: h1 - h0 }, (_, i) => h0 + i)

  // 지금 시각 줄(오늘 열에만). 1분마다 갱신
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 60_000)
    return () => clearInterval(id)
  }, [])
  const nowMin = now.getHours() * 60 + now.getMinutes()

  // 열 하나의 실제 폭(px) — 블록 폭을 정해 좁으면 글을 줄인다
  const scroller = useRef<HTMLDivElement>(null)
  const [colW, setColW] = useState(160)
  useEffect(() => {
    const el = scroller.current
    if (!el) return
    const measure = (): void => setColW(Math.max(MIN_COL_PX, (el.clientWidth - GUTTER_PX) / Math.max(1, perCol.length)))
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [perCol.length])

  // 한 시간의 높이 = (스크롤 영역 높이 − 날짜 머리줄) / 표시 시간 수. 일간은 바깥(.wl-day)이 스크롤 영역이다
  const [hourH, setHourH] = useState(DEFAULT_HOUR_H)
  const hourHRef = useRef(DEFAULT_HOUR_H)
  const hourCount = hours.length
  useLayoutEffect(() => {
    const el = scroller.current
    if (!el) return
    const sc = el.parentElement?.classList.contains('wl-day') ? el.parentElement : el
    const measure = (): void => {
      const headH = el.querySelector<HTMLElement>('.wl-tg__head')?.offsetHeight ?? 0
      const h = Math.max(MIN_HOUR_H, (sc.clientHeight - headH) / Math.max(1, hourCount))
      hourHRef.current = h
      setHourH((prev) => (Math.abs(prev - h) < 0.01 ? prev : h))
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(sc)
    return () => ro.disconnect()
  }, [hourCount])

  // 처음 열 때 첫 항목(없으면 표시 시작 시각) 근처가 보이게. 이슈를 보고 돌아오면 있던 자리로
  const starts = perCol.flatMap((d) => d.blocks.map((b) => b.start))
  const firstStart = starts.length ? Math.min(...starts) : dayStart * 60
  const scrollKey = `${gridDays[0]}|${gridDays.length}|${byUser}|${h0}`
  useEffect(() => {
    const el = scroller.current
    if (!el) return
    // 일간은 바깥(.wl-day)이 스크롤하고, 주간은 이 요소가 스크롤한다
    const sc = el.parentElement?.classList.contains('wl-day') ? el.parentElement : el
    const saved = savedScroll?.key === scrollKey ? savedScroll.top : null
    if (saved !== null) sc.scrollTop = saved
    else if (sc === el) el.scrollTop = Math.max(0, ((firstStart - h0 * 60) / 60) * hourHRef.current - hourHRef.current)
    return () => {
      savedScroll = { key: scrollKey, top: sc.scrollTop }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollKey])

  return (
    <div className="wl-tg" ref={scroller} style={{ '--wl-days': perCol.length, '--wl-hour-h': `${hourH}px` } as React.CSSProperties}>
      <div className="wl-tg__head">
        <div className="wl-tg__corner" />
        {perCol.map((d) => {
          if (d.user) {
            return (
              <div key={d.key} className="wl-tg__day wl-tg__day--user" title={d.user.name}>
                <span className="wl-user__dot" style={{ background: colorOf(d.user.id) }} />
                <span className="wl-tg__who">{d.user.name}</span>
                <span className="wl-tg__sum">{fmtDuration(d.seconds)}</span>
              </div>
            )
          }
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
            <div key={d.key} className={cls}>
              {inner}
            </div>
          ) : (
            <button key={d.key} className={cls} title={t('jira.worklog.goDay', { date: d.ymd })} onClick={() => openDay(d.ymd)}>
              {inner}
            </button>
          )
        })}
      </div>
      <div className="wl-tg__body" style={{ height: hours.length * hourH }}>
        <div className="wl-tg__gutter">
          {hours.map((h) => (
            <div key={h} className="wl-tg__hour" style={{ height: hourH }}>
              <span>{`${String(h).padStart(2, '0')}:00`}</span>
            </div>
          ))}
        </div>
        {perCol.map((d) => (
          <div key={d.key} className={`wl-tg__col${isWeekend(d.ymd) ? ' is-weekend' : ''}${d.ymd === today ? ' is-today' : ''}`}>
            {(() => {
              const lunch = lunchBand(d.ymd, h0, h1)
              return lunch && <div className="wl-tg__lunch" style={{ top: ((lunch.start - h0 * 60) / 60) * hourH, height: ((lunch.end - lunch.start) / 60) * hourH }} />
            })()}
            {d.blocks.map((b) => (
              <Block key={b.log.id} b={b} h0={h0} hourH={hourH} color={issueColorVar(b.log.issueKey)} width={colW / b.cols - 2} goDay={single ? null : openDay} />
            ))}
            {d.ymd === today && nowMin >= h0 * 60 && nowMin <= h1 * 60 && <div className="wl-tg__now" style={{ top: ((nowMin - h0 * 60) / 60) * hourH }} />}
          </div>
        ))}
      </div>
    </div>
  )
}

/**
 * 블록 하나. 폭이 좁으면 제목 → 키까지 줄인다(툴팁에는 전부).
 * 주간(goDay 있음)에서는 블록을 누르면 그날 일간 보기로 가고(좁아서 읽을 수 없으니 상세는 거기서), 이슈 키만 이슈를 연다.
 * 일간에서는 블록을 누르면 이슈를 연다
 */
function Block({ b, h0, hourH, color, width, goDay }: { b: TimeBlock; h0: number; hourH: number; color: string; width: number; goDay: ((ymd: string) => void) | null }): React.JSX.Element {
  const { log } = b
  const top = ((b.start - h0 * 60) / 60) * hourH
  const height = ((b.end - b.start) / 60) * hourH
  const tip = [`${log.issueKey} ${log.issueSummary}`, log.author, `${fmtClock(b.start)} – ${fmtClock(b.start + log.seconds / 60)} (${fmtDuration(log.seconds)})`, log.comment].filter(Boolean).join('\n')
  // 패딩과 제목·시간 두 줄(줄 사이 gap 2개 포함)을 빼고 남는 높이만큼 코멘트 줄 수(블록이 짧으면 0)
  const lines = Math.max(0, Math.floor((Math.max(height - 1, 14) - BLOCK_PAD_PX - 2 * COMMENT_LINE_PX - 2 * BLOCK_GAP_PX) / COMMENT_LINE_PX))
  const tiny = width < TINY_PX
  const narrow = width < NARROW_PX
  const open = (): void => (goDay ? goDay(localYmd(log.started) ?? '') : useJiraStore.getState().openIssue(log.issueKey))
  return (
    <div
      role="button"
      tabIndex={0}
      className={`wl-block${tiny ? ' is-tiny' : narrow ? ' is-narrow' : ''}`}
      title={tip}
      aria-label={tip}
      style={{ top, height: Math.max(height - 1, 14), left: `calc(${(b.col / b.cols) * 100}% + 1px)`, width: `calc(${100 / b.cols}% - 2px)`, '--wl-c': color } as React.CSSProperties}
      onClick={open}
      onKeyDown={(e) => {
        if (e.target === e.currentTarget && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault()
          open()
        }
      }}
    >
      {tiny ? (
        <span className="wl-block__time">{fmtDuration(log.seconds)}</span>
      ) : (
        <>
          <span className="wl-block__line">
            {goDay ? <IssueKey issueKey={log.issueKey} title={tip} /> : <span className="wl-block__key mono">{log.issueKey}</span>}
            <span className="wl-block__time">{fmtDuration(log.seconds)}</span>
          </span>
          {!narrow && <span className="wl-block__text">{log.issueSummary}</span>}
          {!narrow && log.comment && lines > 0 && <span className="wl-block__comment" style={{ WebkitLineClamp: lines }}>{log.comment}</span>}
        </>
      )}
    </div>
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
