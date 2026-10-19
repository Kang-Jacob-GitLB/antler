import { useEffect, useMemo, useRef, useState } from 'react'
import type { CommitActivity, Locale } from '@shared/types'
import { useT } from '../i18n'
import { api, newRequestId } from '../lib/api'
import { useAppStore } from '../stores/appStore'
import { useLogStore } from '../stores/logStore'
import { useSearchStore } from '../stores/searchStore'
import { cancelJump, jumpToTime, useJumpStore } from '../stores/timeJump'
import { IconChevronDown, IconSearch } from './icons'
import './CommitMinimap.css'

const CHART_H = 52
const PAD_TOP = 8
const PAD_BOTTOM = 3
const DAY = 86400
/** 데이터를 요청하기 전 잠깐 기다린다(저장소 전환·필터 변경이 연달아 올 때 헛요청을 줄인다) */
const FETCH_DELAY_MS = 120

const dtfCache = new Map<string, Intl.DateTimeFormat>()
function dtf(locale: Locale, withTime: boolean): Intl.DateTimeFormat {
  const key = `${locale}${withTime ? 't' : ''}`
  let f = dtfCache.get(key)
  if (!f) dtfCache.set(key, (f = new Intl.DateTimeFormat(locale, withTime ? { dateStyle: 'medium', timeStyle: 'short' } : { dateStyle: 'medium' })))
  return f
}

/** 구간의 날짜 범위 표시. 하루보다 짧으면 시각까지, 하루면 날짜, 더 길면 시작–끝 날짜 */
export function bucketRangeLabel(t: number, sec: number, locale: Locale): string {
  if (sec < DAY) return dtf(locale, true).format(new Date(t * 1000))
  const a = dtf(locale, false).format(new Date(t * 1000))
  if (sec === DAY) return a
  return `${a} – ${dtf(locale, false).format(new Date((t + sec - 1) * 1000))}`
}

/** 저장소·필터가 바뀌거나 목록을 다시 읽을 때마다(generation) 활동 데이터를 새로 받는다. 이전 요청은 취소한다 */
function useActivity(enabled: boolean): CommitActivity | null {
  const repoId = useLogStore((s) => s.repoId)
  const scope = useLogStore((s) => s.scope)
  const scopeSig = useLogStore((s) => s.scopeSig)
  const generation = useLogStore((s) => s.generation)
  const [data, setData] = useState<{ repoId: string; value: CommitActivity } | null>(null)

  useEffect(() => {
    if (!enabled || !repoId) return
    const requestId = newRequestId('act')
    let dead = false
    const timer = setTimeout(() => {
      api
        .invoke('git:activity', repoId, scope, requestId)
        .then((value) => {
          if (!dead) setData({ repoId, value })
        })
        .catch(() => {
          // 미니맵은 부가 정보 — 실패해도 목록 사용을 막지 않는다(취소·오류 모두 조용히 넘긴다)
        })
    }, FETCH_DELAY_MS)
    return () => {
      dead = true
      clearTimeout(timer)
      void api.invoke('req:cancel', requestId).catch(() => {})
    }
  }, [enabled, repoId, scopeSig, generation])

  // 다른 저장소의 데이터는 보여 주지 않는다(같은 저장소의 필터 전환 중에는 이전 그림을 유지해 깜박이지 않게)
  return data && data.repoId === repoId ? data.value : null
}

interface Colors {
  accent: string
  line: string
  text: string
}

function readColors(): Colors {
  const cs = getComputedStyle(document.documentElement)
  const v = (n: string): string => cs.getPropertyValue(n).trim()
  return { accent: v('--accent'), line: v('--border-strong'), text: v('--text-secondary') }
}

interface Props {
  scrollEl: HTMLDivElement | null
  rowHeight: number
  /** 스크롤 영역 맨 위의 커밋이 아닌 줄(작업 폴더 변경 행)의 높이(px). GraphCanvas 의 topOffset 과 같은 값 */
  topOffset?: number
}

/**
 * 커밋 활동 미니맵: 전체 이력의 시간 구간별 커밋 수를 얇은 영역 차트로 보여 주고,
 * 지금 목록에 보이는 시간 범위를 띠로 표시한다. 클릭하면 그 시점의 커밋으로 목록을 옮긴다(오래된 쪽이면 더 불러온다).
 * 접기 상태·이니셜 표시는 설정(ui)에 기억한다.
 */
export function CommitMinimap({ scrollEl, rowHeight, topOffset = 0 }: Props): React.JSX.Element {
  const t = useT()
  const mod = window.antler.platform === 'darwin' ? '⌘' : 'Ctrl+'
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const collapsed = useAppStore((s) => s.config?.ui.minimapCollapsed === true)
  const initialsOn = useAppStore((s) => s.config?.ui.graphInitials !== false)
  const setUi = useAppStore((s) => s.setUi)
  const jumping = useJumpStore((s) => s.jumping)
  const data = useActivity(!collapsed)

  const wrapRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const colors = useRef<Colors | null>(null)
  const frame = useRef(0)
  const hoverX = useRef(-1)
  const [hover, setHover] = useState<{ index: number; x: number } | null>(null)
  const [width, setWidth] = useState(0)
  const dataRef = useRef(data)
  dataRef.current = data
  const widthRef = useRef(width)
  widthRef.current = width

  const max = useMemo(() => (data ? data.buckets.reduce((m, b) => Math.max(m, b.count), 1) : 1), [data])
  const maxRef = useRef(max)
  maxRef.current = max

  const draw = (): void => {
    frame.current = 0
    const canvas = canvasRef.current
    const act = dataRef.current
    const w = widthRef.current
    if (!canvas || w <= 0) return
    const dpr = window.devicePixelRatio || 1
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(CHART_H * dpr)) {
      canvas.width = Math.round(w * dpr)
      canvas.height = Math.round(CHART_H * dpr)
    }
    const ctx = canvas.getContext('2d')!
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, w, CHART_H)
    if (!act || act.buckets.length === 0) return
    const col = (colors.current ??= readColors())
    const { buckets, bucketSec } = act
    const n = buckets.length
    const base = CHART_H - PAD_BOTTOM
    const span = base - PAD_TOP
    const m = maxRef.current
    // 개수가 몰린 구간 하나가 나머지를 납작하게 만들지 않도록 제곱근 눈금을 쓴다(툴팁은 실제 개수)
    const yOf = (count: number): number => base - Math.sqrt(count / m) * span

    // 기준선
    ctx.strokeStyle = col.line
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(0, base + 0.5)
    ctx.lineTo(w, base + 0.5)
    ctx.stroke()

    // 영역 + 선 (구간 중앙에 점을 찍고 양 끝은 가장자리까지 늘린다)
    ctx.beginPath()
    ctx.moveTo(0, base)
    ctx.lineTo(0, yOf(buckets[0]!.count))
    for (let i = 0; i < n; i++) ctx.lineTo(((i + 0.5) / n) * w, yOf(buckets[i]!.count))
    ctx.lineTo(w, yOf(buckets[n - 1]!.count))
    ctx.lineTo(w, base)
    ctx.closePath()
    ctx.globalAlpha = 0.2
    ctx.fillStyle = col.accent
    ctx.fill()
    ctx.globalAlpha = 1
    ctx.beginPath()
    ctx.moveTo(0, yOf(buckets[0]!.count))
    for (let i = 0; i < n; i++) ctx.lineTo(((i + 0.5) / n) * w, yOf(buckets[i]!.count))
    ctx.lineTo(w, yOf(buckets[n - 1]!.count))
    ctx.lineWidth = 1.5
    ctx.lineJoin = 'round'
    ctx.strokeStyle = col.accent
    ctx.stroke()

    // 지금 목록에 보이는 시간 범위
    const view = visibleRange()
    if (view) {
      const t0 = buckets[0]!.t
      const total = n * bucketSec
      const x1 = Math.max(0, ((view.lo - t0) / total) * w)
      const x2 = Math.min(w, ((view.hi + 1 - t0) / total) * w)
      const bw = Math.max(4, x2 - x1)
      const bx = Math.min(w - bw, x1)
      ctx.globalAlpha = 0.16
      ctx.fillStyle = col.accent
      ctx.fillRect(bx, 0, bw, CHART_H)
      ctx.globalAlpha = 1
      ctx.strokeStyle = col.accent
      ctx.lineWidth = 1
      ctx.strokeRect(bx + 0.5, 0.5, bw - 1, CHART_H - 1)
    }

    // 호버 위치
    if (hoverX.current >= 0) {
      ctx.globalAlpha = 0.6
      ctx.strokeStyle = col.text
      ctx.beginPath()
      ctx.moveTo(Math.round(hoverX.current) + 0.5, 0)
      ctx.lineTo(Math.round(hoverX.current) + 0.5, CHART_H)
      ctx.stroke()
      ctx.globalAlpha = 1
    }
  }

  /** 스크롤 위치에서 보이는 첫·끝 행의 작성 시각 범위 */
  const visibleRange = (): { lo: number; hi: number } | null => {
    if (!scrollEl) return null
    const commits = useLogStore.getState().commits
    if (commits.length === 0) return null
    const top = scrollEl.scrollTop
    const clamp = (i: number): number => Math.max(0, Math.min(commits.length - 1, i))
    const a = commits[clamp(Math.floor((top - topOffset) / rowHeight))]!.authorDate
    const b = commits[clamp(Math.ceil((top + scrollEl.clientHeight - topOffset) / rowHeight) - 1)]!.authorDate
    return { lo: Math.min(a, b), hi: Math.max(a, b) }
  }

  const drawRef = useRef(draw)
  drawRef.current = draw
  const schedule = useRef((): void => {
    if (!frame.current) frame.current = requestAnimationFrame(() => drawRef.current())
  }).current

  // 스크롤·불러온 커밋 변화 → 보이는 범위 띠 갱신 (React 리렌더 없이 canvas 만 다시 그린다)
  useEffect(() => {
    if (!scrollEl || collapsed) return
    scrollEl.addEventListener('scroll', schedule, { passive: true })
    const ro = new ResizeObserver(schedule)
    ro.observe(scrollEl)
    const unsub = useLogStore.subscribe(schedule)
    return () => {
      scrollEl.removeEventListener('scroll', schedule)
      ro.disconnect()
      unsub()
    }
  }, [scrollEl, collapsed, schedule])

  // 폭 측정
  useEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const measure = (): void => setWidth(el.clientWidth)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [collapsed])

  // 테마가 바뀌면 색을 다시 읽는다
  useEffect(() => {
    const mo = new MutationObserver(() => {
      colors.current = null
      schedule()
    })
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => {
      mo.disconnect()
      if (frame.current) cancelAnimationFrame(frame.current)
      frame.current = 0
    }
  }, [schedule])

  useEffect(schedule, [data, width, collapsed, schedule])

  // 저장소를 바꾸거나 접으면 진행 중인 찾기를 멈춘다
  const repoId = useLogStore((s) => s.repoId)
  useEffect(() => cancelJump, [repoId, collapsed])

  const indexAt = (clientX: number): { index: number; x: number } | null => {
    const act = dataRef.current
    const el = wrapRef.current
    if (!act || !el || act.buckets.length === 0) return null
    const rect = el.getBoundingClientRect()
    const x = Math.max(0, Math.min(rect.width - 1, clientX - rect.left))
    return { index: Math.min(act.buckets.length - 1, Math.floor((x / rect.width) * act.buckets.length)), x }
  }

  const onMove = (e: React.MouseEvent): void => {
    const hit = indexAt(e.clientX)
    hoverX.current = hit ? hit.x : -1
    setHover(hit)
    schedule()
  }
  const onLeave = (): void => {
    hoverX.current = -1
    setHover(null)
    schedule()
  }
  const onClick = (e: React.MouseEvent): void => {
    if (jumping) return cancelJump()
    const act = dataRef.current
    const hit = indexAt(e.clientX)
    if (!act || !hit) return
    const b = act.buckets[hit.index]!
    // 구간 끝 시각 — 그 구간의 가장 최근 커밋이 목록에 나타나도록
    void jumpToTime(Math.min(act.last, b.t + act.bucketSec - 1))
  }

  const tipBucket = hover && data ? data.buckets[hover.index] : undefined
  const tipLeft = hover ? Math.max(90, Math.min(width - 90, hover.x)) : 0

  return (
    <div className={`minimap${collapsed ? ' is-collapsed' : ''}`}>
      <div className="minimap__bar">
        <span className="minimap__title">{t('minimap.title')}</span>
        {!collapsed && data && <span className="minimap__total">{data.total.toLocaleString(locale)}</span>}
        {jumping && <span className="minimap__spinner" role="status" aria-label={t('minimap.jumping')} title={t('minimap.jumping')} />}
        <span className="minimap__spacer" />
        <button
          className="minimap__btn"
          title={`${t('toolbar.search')} (${mod}F)`}
          aria-label={t('toolbar.search')}
          onClick={() => useSearchStore.getState().show()}
        >
          <IconSearch size={13} />
        </button>
        <button
          className={`minimap__btn${initialsOn ? ' is-on' : ''}`}
          aria-pressed={initialsOn}
          title={t(initialsOn ? 'minimap.initialsOff' : 'minimap.initialsOn')}
          aria-label={t(initialsOn ? 'minimap.initialsOff' : 'minimap.initialsOn')}
          onClick={() => void setUi({ graphInitials: !initialsOn })}
        >
          <span className="minimap__ini" aria-hidden="true">
            A
          </span>
        </button>
        <button
          className="minimap__btn"
          aria-expanded={!collapsed}
          title={t(collapsed ? 'minimap.expand' : 'minimap.collapse')}
          aria-label={t(collapsed ? 'minimap.expand' : 'minimap.collapse')}
          onClick={() => void setUi({ minimapCollapsed: !collapsed })}
        >
          <IconChevronDown size={14} className={collapsed ? 'minimap__chev is-collapsed' : 'minimap__chev'} />
        </button>
      </div>
      {!collapsed && (
        <div
          ref={wrapRef}
          className={`minimap__chart${jumping ? ' is-busy' : ''}`}
          role="img"
          aria-label={t('minimap.aria', { n: (data?.total ?? 0).toLocaleString(locale) })}
          onMouseMove={onMove}
          onMouseLeave={onLeave}
          onClick={onClick}
        >
          <canvas ref={canvasRef} style={{ width, height: CHART_H }} />
          {data && data.buckets.length === 0 && <div className="minimap__empty">{t('minimap.empty')}</div>}
          {hover && tipBucket && data && (
            <div className="minimap__tip" style={{ left: tipLeft }}>
              {t('minimap.tip', { range: bucketRangeLabel(tipBucket.t, data.bucketSec, locale), n: tipBucket.count.toLocaleString(locale) })}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
