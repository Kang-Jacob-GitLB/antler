import { useEffect, useRef } from 'react'
import { graphEngine } from '../graph/graphEngine'
import { useAppStore } from '../stores/appStore'
import { useLogStore } from '../stores/logStore'
import { GRAPH_PAD, laneWidth, useGraphVersion } from './GraphCanvas'

/** 이니셜 노드를 쓸 만큼 레인 간격이 넓을 때(GraphCanvas 의 AVATAR_MIN_LANE_W 와 같은 기준)의 WIP 노드 반지름 */
const WIP_R_WIDE = 7
const WIP_R_NARROW = 4.5
const AVATAR_MIN_LANE_W = 13

interface Props {
  scrollEl: HTMLDivElement | null
  rowHeight: number
  /** 그래프 컬럼의 왼쪽 위치와 폭(px) */
  left: number
  width: number
  /** 스크롤 영역 맨 위에서 WIP 행이 차지하는 높이(커밋 행은 이만큼 아래에서 시작한다) */
  topOffset: number
}

/**
 * 맨 위 WIP 행의 그래프: 속 빈 점선 노드와, HEAD 커밋 노드까지 이어지는 점선 연결선(HEAD 의 레인).
 * 레인 그래프(GraphCanvas)와 따로 그려 노드 모양을 건드리지 않는다 — 이 캔버스가 아래에 깔리고 커밋 노드가 위에 덮인다.
 */
export function WipGraph({ scrollEl, rowHeight, left, width, topOffset }: Props): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const frame = useRef(0)
  const version = useGraphVersion()
  const avatars = useAppStore((s) => s.config?.ui.graphInitials !== false)
  const headHash = useAppStore((s) => s.head?.hash ?? null)
  // HEAD 커밋이 불러온 목록에 있으면 그 행 번호(없으면 연결선 없이 노드만 그린다)
  const headIndex = useLogStore((s) => (headHash ? (s.index.get(headHash) ?? -1) : -1))

  const draw = (): void => {
    frame.current = 0
    const canvas = canvasRef.current
    if (!canvas || !scrollEl) return
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

    const cs = getComputedStyle(document.documentElement)
    const muted = cs.getPropertyValue('--text-muted').trim()
    const bg = cs.getPropertyValue('--list-bg').trim() || cs.getPropertyValue('--bg-base').trim()
    const top = scrollEl.scrollTop
    const lw = laneWidth(graphEngine.maxWidth, avatars)
    const r = avatars && lw >= AVATAR_MIN_LANE_W ? WIP_R_WIDE : Math.min(WIP_R_NARROW, lw / 2 - 0.5)
    const x = (lane: number): number => GRAPH_PAD + lane * lw + lw / 2
    const headRow = headIndex >= 0 ? graphEngine.row(headIndex) : null
    const cx = x(headRow?.lane ?? 0)
    const wipY = topOffset / 2 - top
    const headY = topOffset + headIndex * rowHeight + rowHeight / 2 - top

    ctx.lineWidth = 1.5
    ctx.strokeStyle = muted
    ctx.lineCap = 'butt'
    if (headRow) {
      ctx.setLineDash([3, 3])
      ctx.beginPath()
      ctx.moveTo(cx, wipY + r)
      ctx.lineTo(cx, headY)
      ctx.stroke()
    }
    // 속 빈 점선 노드
    ctx.setLineDash([2.5, 2.5])
    ctx.beginPath()
    ctx.arc(cx, wipY, r, 0, Math.PI * 2)
    ctx.fillStyle = bg
    ctx.fill()
    ctx.stroke()
    ctx.setLineDash([])
  }

  const drawRef = useRef(draw)
  drawRef.current = draw
  const schedule = useRef((): void => {
    if (!frame.current) frame.current = requestAnimationFrame(() => drawRef.current())
  }).current

  useEffect(() => {
    if (!scrollEl) return
    scrollEl.addEventListener('scroll', schedule, { passive: true })
    const ro = new ResizeObserver(schedule)
    ro.observe(scrollEl)
    // 테마가 바뀌면 색을 다시 읽는다
    const mo = new MutationObserver(schedule)
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => {
      scrollEl.removeEventListener('scroll', schedule)
      ro.disconnect()
      mo.disconnect()
      if (frame.current) cancelAnimationFrame(frame.current)
      frame.current = 0
    }
  }, [scrollEl, schedule])

  useEffect(schedule, [schedule, version, width, headIndex, avatars, topOffset, left])

  return <canvas ref={canvasRef} className="graph-canvas" style={{ left }} aria-hidden="true" />
}
