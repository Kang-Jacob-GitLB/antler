// 워크로그 화면의 순수 로직(날짜 범위 · 시간 표기 · 일별 버킷 · 겹침 열 배치 · 리포트 집계 · TSV). React·스토어에 의존하지 않는다.
// 날짜는 모두 로컬 달력 기준 'YYYY-MM-DD' 문자열이고, 주는 월요일에 시작한다.
import type { JiraUser, JiraWorklog } from '@shared/types'

export type WorklogMode = 'day' | 'week' | 'month'
export type WorklogGroup = 'user' | 'issue'

/** 하루 기준 근무 시간(초) */
export const DAY_TARGET_SEC = 8 * 3600

// ── 날짜 ───────────────────────────────────────────────

const pad = (n: number): string => String(n).padStart(2, '0')

export function toYmd(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** 'YYYY-MM-DD' → 로컬 자정 Date */
export function parseYmd(s: string): Date {
  const [y = 1970, m = 1, d = 1] = s.split('-').map(Number)
  return new Date(y, m - 1, d)
}

export function addDays(ymd: string, n: number): string {
  const d = parseYmd(ymd)
  d.setDate(d.getDate() + n)
  return toYmd(d)
}

export function todayYmd(now: Date = new Date()): string {
  return toYmd(now)
}

/** 그 날이 든 주의 월요일 */
export function weekStart(ymd: string): string {
  const dow = (parseYmd(ymd).getDay() + 6) % 7 // 월=0 … 일=6
  return addDays(ymd, -dow)
}

/** 월=0 … 일=6 */
export function weekdayIndex(ymd: string): number {
  return (parseYmd(ymd).getDay() + 6) % 7
}

export function isWeekend(ymd: string): boolean {
  return weekdayIndex(ymd) >= 5
}

export function daysBetween(from: string, to: string): string[] {
  const out: string[] = []
  for (let d = from; d <= to; d = addDays(d, 1)) out.push(d)
  return out
}

export function monthStart(ymd: string): string {
  return `${ymd.slice(0, 7)}-01`
}

export function monthEnd(ymd: string): string {
  const d = parseYmd(monthStart(ymd))
  return toYmd(new Date(d.getFullYear(), d.getMonth() + 1, 0))
}

export interface WorklogRange {
  /** 조회하는 범위(양끝 포함). 월간은 그 달 1일이 든 주 월요일 ~ 말일이 든 주 일요일 */
  from: string
  to: string
  /** 화면에 격자로 깔리는 날짜 = from..to */
  gridDays: string[]
  /** 합계·리포트의 대상 날짜. 일·주는 gridDays 와 같고, 월간은 그 달 날짜만 */
  days: string[]
}

export function rangeFor(mode: WorklogMode, anchor: string): WorklogRange {
  if (mode === 'day') return { from: anchor, to: anchor, gridDays: [anchor], days: [anchor] }
  if (mode === 'week') {
    const from = weekStart(anchor)
    const days = daysBetween(from, addDays(from, 6))
    return { from, to: addDays(from, 6), gridDays: days, days }
  }
  const from = weekStart(monthStart(anchor))
  const to = addDays(weekStart(monthEnd(anchor)), 6)
  return { from, to, gridDays: daysBetween(from, to), days: daysBetween(monthStart(anchor), monthEnd(anchor)) }
}

/** 이전(-1)·다음(+1) 범위의 기준일. 월간은 그 달 1일로 맞춰 말일 넘침을 피한다 */
export function shiftAnchor(mode: WorklogMode, anchor: string, dir: -1 | 1): string {
  if (mode === 'day') return addDays(anchor, dir)
  if (mode === 'week') return addDays(anchor, 7 * dir)
  const d = parseYmd(monthStart(anchor))
  return toYmd(new Date(d.getFullYear(), d.getMonth() + dir, 1))
}

/** 범위 라벨: 2026년 10월 / 10월 19일 – 25일 / 10월 19일 (월) */
export function rangeLabel(mode: WorklogMode, anchor: string, locale: 'ko' | 'en'): string {
  const ko = locale === 'ko'
  const fmt = (ymd: string, o: Intl.DateTimeFormatOptions): string => parseYmd(ymd).toLocaleDateString(ko ? 'ko-KR' : 'en-US', o)
  if (mode === 'month') return fmt(anchor, { year: 'numeric', month: 'long' })
  if (mode === 'day') return fmt(anchor, { month: 'long', day: 'numeric', weekday: 'short' })
  const r = rangeFor('week', anchor)
  const a = parseYmd(r.from)
  const b = parseYmd(r.to)
  const sameYear = a.getFullYear() === b.getFullYear()
  const sameMonth = sameYear && a.getMonth() === b.getMonth()
  const start = fmt(r.from, sameYear ? { month: 'long', day: 'numeric' } : { year: 'numeric', month: 'long', day: 'numeric' })
  const end = sameMonth ? (ko ? `${b.getDate()}일` : String(b.getDate())) : fmt(r.to, sameYear ? { month: 'long', day: 'numeric' } : { year: 'numeric', month: 'long', day: 'numeric' })
  return `${start} – ${end}`
}

export function weekdayShort(ymd: string, locale: 'ko' | 'en'): string {
  return parseYmd(ymd).toLocaleDateString(locale === 'ko' ? 'ko-KR' : 'en-US', { weekday: 'short' })
}

// ── 시간 표기 ──────────────────────────────────────────

/** 초 → '7h 30m' / '45m' / '8h'. 분 단위로 반올림하고, 0 은 '0m' */
export function fmtDuration(seconds: number): string {
  const min = Math.round(seconds / 60)
  if (min <= 0) return '0m'
  const h = Math.floor(min / 60)
  const m = min % 60
  if (h === 0) return `${m}m`
  return m === 0 ? `${h}h` : `${h}h ${m}m`
}

/** 'HH:MM' (로컬) */
export function fmtClock(minutesOfDay: number): string {
  const m = Math.max(0, Math.min(1440, Math.round(minutesOfDay)))
  return `${pad(Math.floor(m / 60) % 24)}:${pad(m % 60)}`
}

// ── 워크로그 거르기·묶기 ────────────────────────────────

/** 로컬 날짜. 해석할 수 없는 시각이면 null */
export function localYmd(started: string): string | null {
  const d = new Date(started)
  return Number.isNaN(d.getTime()) ? null : toYmd(d)
}

/** 로컬 자정부터의 분(소수 가능) */
export function minutesOfDay(started: string): number {
  const d = new Date(started)
  return d.getHours() * 60 + d.getMinutes() + d.getSeconds() / 60
}

/** 로컬 날짜가 [from,to] 에 들고 작성자가 허용된 항목만. authors 가 null 이면 작성자는 거르지 않는다 */
export function filterWorklogs(worklogs: JiraWorklog[], from: string, to: string, authors: ReadonlySet<string> | null): JiraWorklog[] {
  return worklogs.filter((w) => {
    const d = localYmd(w.started)
    return d !== null && d >= from && d <= to && (!authors || authors.has(w.authorId))
  })
}

export const sumSeconds = (logs: JiraWorklog[]): number => logs.reduce((n, w) => n + w.seconds, 0)

export interface IssueSlice {
  key: string
  summary: string
  type: string
  seconds: number
}

export interface DayBucket {
  ymd: string
  seconds: number
  /** 이슈별 합(시간 많은 순) */
  issues: IssueSlice[]
  logs: JiraWorklog[]
}

/** 날짜별 버킷(started 의 로컬 날짜). 항목이 없는 날은 만들지 않는다 */
export function bucketByDay(logs: JiraWorklog[]): Map<string, DayBucket> {
  const out = new Map<string, DayBucket>()
  for (const w of logs) {
    const ymd = localYmd(w.started)
    if (!ymd) continue
    let b = out.get(ymd)
    if (!b) out.set(ymd, (b = { ymd, seconds: 0, issues: [], logs: [] }))
    b.seconds += w.seconds
    b.logs.push(w)
    const s = b.issues.find((i) => i.key === w.issueKey)
    if (s) s.seconds += w.seconds
    else b.issues.push({ key: w.issueKey, summary: w.issueSummary, type: w.issueType, seconds: w.seconds })
  }
  for (const b of out.values()) {
    b.issues.sort((a, c) => c.seconds - a.seconds || a.key.localeCompare(c.key))
    b.logs.sort((a, c) => Date.parse(a.started) - Date.parse(c.started))
  }
  return out
}

export type DayStatus = 'none' | 'under' | 'ok' | 'over'

/**
 * 하루 기준 대비 상태. 주말·오늘 이후는 'none'(초과만 'over'), 지난 평일은 부족이면 'under'.
 * 오늘은 아직 진행 중이므로 부족으로 치지 않는다
 */
export function dayStatus(ymd: string, seconds: number, target: number, today: string): DayStatus {
  if (target > 0 && seconds > target) return 'over'
  if (isWeekend(ymd) || ymd >= today || target <= 0) return seconds >= target && target > 0 ? 'ok' : 'none'
  return seconds >= target ? 'ok' : 'under'
}

// ── 타임라인 배치 ───────────────────────────────────────

export interface TimeBlock {
  log: JiraWorklog
  /** 로컬 자정부터의 분 */
  start: number
  /** 표시용 끝(자정을 넘으면 1440 으로 자름, 너무 짧으면 MIN_BLOCK_MIN 이상으로) */
  end: number
  /** 겹침 묶음 안의 열 번호와 그 묶음의 열 수 */
  col: number
  cols: number
}

/** 너무 짧은 항목도 눌러 볼 수 있게 블록은 이 길이 이상으로 그린다 */
export const MIN_BLOCK_MIN = 20

/** 한 날의 항목을 시작 시각순으로 놓고, 겹치는 것끼리 묶어 열을 나눈다 */
export function layoutBlocks(logs: JiraWorklog[]): TimeBlock[] {
  const items = logs
    .map((log) => {
      const start = minutesOfDay(log.started)
      const end = Math.min(1440, Math.max(start + log.seconds / 60, start + MIN_BLOCK_MIN))
      return { log, start, end, col: 0, cols: 1 }
    })
    .sort((a, b) => a.start - b.start || b.end - a.end)

  let cluster: typeof items = []
  let clusterEnd = -1
  let colEnds: number[] = []
  const flush = (): void => {
    for (const it of cluster) it.cols = colEnds.length || 1
    cluster = []
    colEnds = []
  }
  for (const it of items) {
    if (cluster.length > 0 && it.start >= clusterEnd) flush()
    let col = colEnds.findIndex((e) => e <= it.start)
    if (col < 0) col = colEnds.length
    colEnds[col] = it.end
    it.col = col
    cluster.push(it)
    clusterEnd = cluster.length === 1 ? it.end : Math.max(clusterEnd, it.end)
  }
  flush()
  return items
}

/** 타임라인의 시 범위. 기본 7–21 이고, 그 밖의 항목이 있으면 넓힌다([startHour, endHour) → 축은 endHour 시까지) */
export function hourRange(blocks: TimeBlock[], def: [number, number] = [7, 21]): [number, number] {
  let [a, b] = def
  for (const k of blocks) {
    a = Math.min(a, Math.floor(k.start / 60))
    b = Math.max(b, Math.ceil(k.end / 60))
  }
  return [Math.max(0, a), Math.min(24, b)]
}

// ── 리포트 ─────────────────────────────────────────────

export interface ReportRow {
  /** 펼침 상태 키(그룹 안에서 유일) */
  id: string
  label: string
  /** 이슈 행이면 키·요약, 사용자 행이면 없음 */
  issue?: { key: string; summary: string; type: string }
  /** 사용자 행이면 그 사용자의 id(색을 정할 때 쓴다) */
  userId?: string
  kind: 'user' | 'issue'
  total: number
  byDay: Record<string, number>
  /** 있으면 이 행의 날 칸을 하루 기준과 견줘 부족을 표시한다 */
  target?: number
  children: ReportRow[]
}

export interface Report {
  rows: ReportRow[]
  total: ReportRow
}

function addTo(row: ReportRow, ymd: string, sec: number): void {
  row.byDay[ymd] = (row.byDay[ymd] ?? 0) + sec
  row.total += sec
}

const newRow = (id: string, label: string, kind: ReportRow['kind'], extra: Partial<ReportRow> = {}): ReportRow => ({ id, label, kind, total: 0, byDay: {}, children: [], ...extra })

/**
 * 리포트 집계. logs 는 이미 대상 날짜·작성자로 거른 것.
 * 'user': 행=사용자(선택한 사용자는 0 이어도 보임), 하위=이슈. 'issue': 행=이슈, 하위=사용자
 */
export function buildReport(logs: JiraWorklog[], users: JiraUser[], group: WorklogGroup): Report {
  const total = newRow('total', '', 'user', { target: DAY_TARGET_SEC * Math.max(1, users.length) })
  const top = new Map<string, ReportRow>()
  const sub = new Map<string, ReportRow>()
  const nameOf = new Map(users.map((u) => [u.id, u.name]))

  if (group === 'user') for (const u of users) top.set(u.id, newRow(`u:${u.id}`, u.name, 'user', { target: DAY_TARGET_SEC, userId: u.id }))

  for (const w of logs) {
    const ymd = localYmd(w.started)
    if (!ymd) continue
    const authorName = nameOf.get(w.authorId) ?? w.author
    const issueMeta = { key: w.issueKey, summary: w.issueSummary, type: w.issueType }
    let parent: ReportRow
    let child: ReportRow
    const pk = group === 'user' ? w.authorId : w.issueKey
    const ck = group === 'user' ? w.issueKey : w.authorId
    if (group === 'user') {
      parent = top.get(pk) ?? top.set(pk, newRow(`u:${pk}`, authorName, 'user', { target: DAY_TARGET_SEC, userId: pk })).get(pk)!
      child = sub.get(`${pk}\n${ck}`) ?? newRow(`u:${pk}/i:${ck}`, w.issueKey, 'issue', { issue: issueMeta })
    } else {
      parent = top.get(pk) ?? top.set(pk, newRow(`i:${pk}`, w.issueKey, 'issue', { issue: issueMeta })).get(pk)!
      child = sub.get(`${pk}\n${ck}`) ?? newRow(`i:${pk}/u:${ck}`, authorName, 'user', { target: DAY_TARGET_SEC, userId: ck })
    }
    if (!sub.has(`${pk}\n${ck}`)) {
      sub.set(`${pk}\n${ck}`, child)
      parent.children.push(child)
    }
    addTo(parent, ymd, w.seconds)
    addTo(child, ymd, w.seconds)
    addTo(total, ymd, w.seconds)
  }

  const byLabel = (a: ReportRow, b: ReportRow): number => a.label.localeCompare(b.label)
  const rows = [...top.values()]
  for (const r of rows) r.children.sort(byLabel)
  // 사용자 행은 입력 순서(나 → 팀원), 이슈 행은 키 순
  if (group === 'issue') rows.sort(byLabel)
  return { rows, total }
}

/** 표 복사용 TSV: 시간은 소수 시간(7.5)이라 스프레드시트에서 바로 합산할 수 있다 */
export function reportToTsv(report: Report, days: string[], labels: { name: string; total: string }): string {
  const hours = (s: number): string => (s ? String(Math.round((s / 3600) * 100) / 100) : '')
  const clean = (s: string): string => s.replace(/[\t\r\n]+/g, ' ')
  const lines: string[] = [[labels.name, ...days, labels.total].join('\t')]
  const put = (r: ReportRow, label: string): void => {
    lines.push([clean(label), ...days.map((d) => hours(r.byDay[d] ?? 0)), hours(r.total)].join('\t'))
  }
  for (const r of report.rows) {
    put(r, r.issue ? `${r.issue.key} ${r.issue.summary}` : r.label)
    for (const c of r.children) put(c, `  ${c.issue ? `${c.issue.key} ${c.issue.summary}` : c.label}`)
  }
  put(report.total, labels.total)
  return lines.join('\n')
}

// ── 사용자 ─────────────────────────────────────────────

/** 선택한 사용자를 '나 → 팀원(팀 목록 순)'으로 */
export function selectedUsers(me: JiraUser | null, includeMe: boolean, team: JiraUser[], selectedTeam: ReadonlySet<string>): JiraUser[] {
  const out: JiraUser[] = []
  if (includeMe && me) out.push(me)
  for (const u of team) if (selectedTeam.has(u.id) && u.id !== me?.id) out.push(u)
  return out
}
