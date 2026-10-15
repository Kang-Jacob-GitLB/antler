import { useEffect, useRef, useSyncExternalStore } from 'react'
import type { Commit } from '@shared/types'
import { graphEngine } from '../graph/graphEngine'
import { SEG_FULL, SEG_TOP } from '../graph/lanes'
import { LANE_COUNT } from '../lib/laneColor'

export const LANE_W = 14
export const GRAPH_PAD = 8
const MAX_GRAPH_W = 260
const MIN_LANE_W = 4
const NODE_R = 4.5

/** 레인 간격(px). 레인이 많아 상한 폭을 넘으면 간격을 줄여 모든 레인이 컬럼 안에 들어오게 한다 */
export function laneWidth(maxLanes: number): number {
  const fit = (MAX_GRAPH_W - GRAPH_PAD * 2) / Math.max(1, maxLanes)
  return Math.max(MIN_LANE_W, Math.min(LANE_W, fit))
}

/** 그래프 컬럼 폭(px). 지금까지 본 최대 레인 수에 맞추되 상한을 둔다 */
export function graphWidth(maxLanes: number): number {
  return Math.min(MAX_GRAPH_W, Math.max(56, GRAPH_PAD * 2 + maxLanes * laneWidth(maxLanes)))
}

export function useGraphVersion(): number {
  return useSyncExternalStore(
    (fn) => graphEngine.subscribe(fn),
    () => graphEngine.version
  )
}

interface Palette {
  lanes: string[]
  bg: string
  accent: string
}

function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement)
  const v = (name: string): string => cs.getPropertyValue(name).trim()
  return {
    lanes: Array.from({ length: LANE_COUNT }, (_, i) => v(`--lane-${i}`)),
    bg: v('--list-bg') || v('--bg-base'),
    accent: v('--accent')
  }
}

interface Props {
  scrollEl: HTMLDivElement | null
  rowHeight: number
  /** 그래프 컬럼의 왼쪽 위치와 폭(px) */
  left: number
  width: number
  commits: Commit[]
  /** 스크롤 영역 맨 위의 커밋 행이 아닌 줄(WIP 행)의 높이(px). 커밋 행은 이만큼 아래에서 시작한다 */
  topOffset?: number
}

/**
 * 보이는 행만 그리는 Canvas 레인 그래프. 스크롤 컨테이너 안에 sticky 로 고정되어
 * 스크롤할 때마다 현재 화면 범위를 다시 그린다. 색은 --lane-N 토큰에서 읽는다.
 */
export function GraphCanvas({ scrollEl, rowHeight, left, width, commits, topOffset = 0 }: Props): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const palette = useRef<Palette | null>(null)
  const frame = useRef(0)
  const commitsRef = useRef(commits)
  commitsRef.current = commits
  const version = useGraphVersion()

  const draw = (): void => {
    frame.current = 0
    const canvas = canvasRef.current
    if (!canvas || !scrollEl) return
    const pal = (palette.current ??= readPalette())
    const dpr = window.devicePixelRatio || 1
    const h = scrollEl.clientHeight
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(width * dpr)
      canvas.height = Math.round(h * dpr)
      canvas.style.width = `${width}px`
      canvas.style.height = `${h}px`
    }
    const ctx = canvas.getContext('2d')!
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, width, h)

    const top = scrollEl.scrollTop
    const first = Math.max(0, Math.floor((top - topOffset) / rowHeight))
    const last = Math.min(graphEngine.count - 1, Math.ceil((top + h - topOffset) / rowHeight))
    const lw = laneWidth(graphEngine.maxWidth)
    // 레인 간격이 좁아지면 노드·선도 함께 줄인다
    const r = Math.min(NODE_R, lw / 2 - 0.5)
    const stroke = lw < 10 ? 1.5 : 2
    const x = (lane: number): number => GRAPH_PAD + lane * lw + lw / 2

    // 선: 색별로 Path2D 에 모아 한 번씩 stroke 한다
    const paths = Array.from({ length: LANE_COUNT }, () => new Path2D())
    for (let i = first; i <= last; i++) {
      const row = graphEngine.row(i)
      if (!row) continue
      const y0 = topOffset + i * rowHeight - top
      const ym = y0 + rowHeight / 2
      const y1 = y0 + rowHeight
      const s = row.segs
      for (let k = 0; k < s.length; k += 4) {
        const kind = s[k]!
        const xf = x(s[k + 1]!)
        const xt = x(s[k + 2]!)
        const p = paths[s[k + 3]! % LANE_COUNT]!
        const [ya, yb] = kind === SEG_FULL ? [y0, y1] : kind === SEG_TOP ? [y0, ym] : [ym, y1]
        p.moveTo(xf, ya)
        if (xf === xt) p.lineTo(xt, yb)
        else {
          // 갈라지거나 모이는 선은 수직 접선을 갖는 S 곡선
          const yc = (ya + yb) / 2
          p.bezierCurveTo(xf, yc, xt, yc, xt, yb)
        }
      }
    }
    ctx.lineWidth = stroke
    ctx.lineCap = 'round'
    paths.forEach((p, i) => {
      ctx.strokeStyle = pal.lanes[i]!
      ctx.stroke(p)
    })

    // 노드
    const list = commitsRef.current
    for (let i = first; i <= last; i++) {
      const row = graphEngine.row(i)
      if (!row) continue
      const cx = x(row.lane)
      const cy = topOffset + i * rowHeight - top + rowHeight / 2
      const color = pal.lanes[row.color % LANE_COUNT]!
      ctx.beginPath()
      ctx.arc(cx, cy, r, 0, Math.PI * 2)
      if (row.merge) {
        // merge 커밋은 속이 빈 링
        ctx.fillStyle = pal.bg
        ctx.fill()
        ctx.lineWidth = stroke
        ctx.strokeStyle = color
        ctx.stroke()
      } else {
        ctx.fillStyle = color
        ctx.fill()
      }
      if (list[i]?.refs.some((r) => r.kind === 'head')) {
        // HEAD 는 앰버 외곽선
        ctx.beginPath()
        ctx.arc(cx, cy, r + 3, 0, Math.PI * 2)
        ctx.lineWidth = 1.5
        ctx.strokeStyle = pal.accent
        ctx.stroke()
      }
    }
  }

  // 이벤트 리스너가 오래된 props 를 붙잡지 않도록 항상 최신 draw 를 부른다
  const drawRef = useRef(draw)
  drawRef.current = draw
  const schedule = useRef((): void => {
    if (!frame.current) frame.current = requestAnimationFrame(() => drawRef.current())
  }).current

  // 스크롤·크기 변화
  useEffect(() => {
    if (!scrollEl) return
    scrollEl.addEventListener('scroll', schedule, { passive: true })
    const ro = new ResizeObserver(schedule)
    ro.observe(scrollEl)
    return () => {
      scrollEl.removeEventListener('scroll', schedule)
      ro.disconnect()
      if (frame.current) cancelAnimationFrame(frame.current)
      frame.current = 0
    }
  }, [scrollEl])

  // 테마가 바뀌면 토큰 색을 다시 읽는다
  useEffect(() => {
    const mo = new MutationObserver(() => {
      palette.current = null
      schedule()
    })
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => mo.disconnect()
  }, [])

  // 창을 다른 배율의 모니터로 옮기면 devicePixelRatio 가 바뀐다 — 흐려지지 않게 다시 그린다.
  // matchMedia 는 현재 배율에 대해서만 change 를 알리므로 바뀔 때마다 새로 건다
  useEffect(() => {
    let mq: MediaQueryList | null = null
    const watch = (): void => {
      mq?.removeEventListener('change', onChange)
      mq = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`)
      mq.addEventListener('change', onChange)
    }
    const onChange = (): void => {
      watch()
      schedule()
    }
    watch()
    return () => mq?.removeEventListener('change', onChange)
  }, [])

  // 데이터·폭·커밋 목록 변화
  useEffect(schedule, [version, width, commits, topOffset])

  return <canvas ref={canvasRef} className="graph-canvas" style={{ left }} aria-hidden="true" />
}
