import { useEffect, useRef, useSyncExternalStore } from 'react'
import type { Commit } from '@shared/types'
import { graphEngine } from '../graph/graphEngine'
import { SEG_BOLD, SEG_FULL, SEG_TOP } from '../graph/lanes'
import { avatarImage, avatarSprite, onAvatarImage } from '../graph/avatarImages'
import { authorInitials, nodeColors } from '../graph/nodeStyle'
import { LANE_COUNT } from '../lib/laneColor'
import { avatarPx, avatarSrc } from '@shared/avatar'
import { useAppStore } from '../stores/appStore'
import { avatarBaseOf, useAvatarStore } from '../stores/avatarStore'
import { useLogStore } from '../stores/logStore'

export const LANE_W = 14
export const GRAPH_PAD = 8
const MAX_GRAPH_W = 260
const MIN_LANE_W = 4
const NODE_R = 4.5
/** 이니셜 노드를 쓸 때의 레인 간격(px)과 노드 최대 반지름 */
const AVATAR_LANE_W = 18
const AVATAR_R = 8.5
/** 레인 간격이 이보다 좁아지면(레인이 아주 많으면) 이니셜 대신 작은 점으로 그린다 */
const AVATAR_MIN_LANE_W = 13

/** 레인 간격(px). 레인이 많아 상한 폭을 넘으면 간격을 줄여 모든 레인이 컬럼 안에 들어오게 한다 */
export function laneWidth(maxLanes: number, avatars = false): number {
  const fit = (MAX_GRAPH_W - GRAPH_PAD * 2) / Math.max(1, maxLanes)
  return Math.max(MIN_LANE_W, Math.min(avatars ? AVATAR_LANE_W : LANE_W, fit))
}

/** 그래프 컬럼 폭(px). 지금까지 본 최대 레인 수에 맞추되 상한을 둔다 */
export function graphWidth(maxLanes: number, avatars = false): number {
  return Math.min(MAX_GRAPH_W, Math.max(56, GRAPH_PAD * 2 + maxLanes * laneWidth(maxLanes, avatars)))
}

/** 작성자 이름 → 이니셜 캐시(프레임마다 정규식을 돌리지 않도록) */
const initialsCache = new Map<string, string>()
function initialsOf(name: string): string {
  let s = initialsCache.get(name)
  if (s === undefined) {
    if (initialsCache.size > 5000) initialsCache.clear()
    initialsCache.set(name, (s = authorInitials(name)))
  }
  return s
}

/** 아바타 기본 주소 → 크기(`s=`)를 붙인 src 캐시(프레임마다 문자열을 만들지 않도록). 요청 크기가 바뀌면 비운다 */
const avatarSrcs = new Map<string, string>()
let avatarSrcPx = 0

export function useGraphVersion(): number {
  return useSyncExternalStore(
    (fn) => graphEngine.subscribe(fn),
    () => graphEngine.version
  )
}

interface Palette {
  lanes: string[]
  /** 이니셜 노드의 채움색·글자색(글자 대비 AA 를 맞춘 값) */
  nodeFill: string[]
  nodeText: string[]
  font1: string
  font2: string
  bg: string
  accent: string
}

function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement)
  const v = (name: string): string => cs.getPropertyValue(name).trim()
  const lanes = Array.from({ length: LANE_COUNT }, (_, i) => v(`--lane-${i}`))
  const nodes = lanes.map((c) => nodeColors(c))
  const family = v('--font-ui') || 'sans-serif'
  return {
    lanes,
    nodeFill: nodes.map((n) => n.fill),
    nodeText: nodes.map((n) => n.text),
    font1: `700 11px ${family}`,
    font2: `700 9px ${family}`,
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
  // 작성자 이니셜 표시(설정이 없으면 켜짐)
  const avatars = useAppStore((s) => s.config?.ui.graphInitials !== false)

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
    const lw = laneWidth(graphEngine.maxWidth, avatars)
    // 이니셜 노드는 레인 간격이 충분할 때만(아니면 작은 점)
    const av = avatars && lw >= AVATAR_MIN_LANE_W
    // 레인 간격이 좁아지면 노드·선도 함께 줄인다. 이니셜 노드는 행 높이에 맞춘다(위아래 4px 여유)
    const r = av ? Math.min(AVATAR_R, rowHeight / 2 - 4) : Math.min(NODE_R, lw / 2 - 0.5)
    // merge 커밋은 이니셜 없이 더 작은 속 빈 링
    const rMerge = av ? Math.min(NODE_R + 0.5, lw / 2 - 0.5) : r
    const stroke = lw < 10 ? 1.5 : 2
    const x = (lane: number): number => GRAPH_PAD + lane * lw + lw / 2

    // 선: 색별로 Path2D 에 모아 한 번씩 stroke 한다
    const paths = Array.from({ length: LANE_COUNT }, () => new Path2D())
    // 기본 브랜치 줄기(SEG_BOLD 표지)는 따로 모아 더 굵게 위에 덧그린다. 줄기가 없으면 만들지 않는다
    let boldPaths: Path2D[] | null = null
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
        const raw = s[k + 3]!
        const p =
          raw >= SEG_BOLD
            ? (boldPaths ??= Array.from({ length: LANE_COUNT }, () => new Path2D()))[(raw - SEG_BOLD) % LANE_COUNT]!
            : paths[raw % LANE_COUNT]!
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
    if (boldPaths) {
      ctx.lineWidth = stroke * 1.7
      boldPaths.forEach((p, i) => {
        ctx.strokeStyle = pal.lanes[i]!
        ctx.stroke(p)
      })
    }

    // 노드
    const list = commitsRef.current
    // 글자 크기는 이니셜 길이별로 둘만 쓴다(문자열은 팔레트를 읽을 때 한 번만 만든다)
    const { font1, font2 } = pal
    let curFont = ''
    // 아바타: 해석된 주소 표와 요청 픽셀 크기(크기가 바뀌면 주소 캐시를 비운다)
    const avatarUrls = useAvatarStore.getState().urls
    const avPx = avatarPx(r * 2, dpr)
    if (avatarSrcPx !== avPx) {
      avatarSrcs.clear()
      avatarSrcPx = avPx
    }
    ctx.textAlign = 'center'
    ctx.textBaseline = 'middle'
    for (let i = first; i <= last; i++) {
      const row = graphEngine.row(i)
      if (!row) continue
      const cx = x(row.lane)
      const cy = topOffset + i * rowHeight - top + rowHeight / 2
      const lane = row.color % LANE_COUNT
      const color = pal.lanes[lane]!
      const nr = row.merge ? rMerge : r
      ctx.beginPath()
      ctx.arc(cx, cy, nr, 0, Math.PI * 2)
      if (row.merge) {
        // merge 커밋은 속이 빈 링
        ctx.fillStyle = pal.bg
        ctx.fill()
        ctx.lineWidth = stroke
        ctx.strokeStyle = color
        ctx.stroke()
      } else if (av) {
        // 프로필 사진이 로드돼 있으면 레인 색 링이 둘린 원형 스프라이트를 찍는다(없으면 아래 이니셜)
        const base = avatarBaseOf(list[i]?.authorEmail ?? '', avatarUrls)
        let img: HTMLImageElement | null = null
        let src = ''
        if (base) {
          src = avatarSrcs.get(base) ?? ''
          if (!src) avatarSrcs.set(base, (src = avatarSrc(base, avPx)))
          img = avatarImage(src)
        }
        if (img) {
          const sp = avatarSprite(img, src, lane, color, pal.bg, nr, dpr)
          ctx.drawImage(sp.canvas, cx - sp.size / 2, cy - sp.size / 2, sp.size, sp.size)
        } else {
          // 이니셜 노드: 배경색 테두리로 뒤를 지나는 선과 분리한다
          ctx.fillStyle = pal.nodeFill[lane]!
          ctx.fill()
          ctx.lineWidth = 1.5
          ctx.strokeStyle = pal.bg
          ctx.stroke()
          const text = initialsOf(list[i]?.authorName ?? '')
          const font = text.length > 1 ? font2 : font1
          if (curFont !== font) ctx.font = curFont = font
          ctx.fillStyle = pal.nodeText[lane]!
          ctx.fillText(text, cx, cy + 0.5)
        }
      } else {
        ctx.fillStyle = color
        ctx.fill()
      }
      if (list[i]?.refs.some((r) => r.kind === 'head')) {
        // HEAD 는 앰버 외곽선
        ctx.beginPath()
        ctx.arc(cx, cy, nr + (av && !row.merge ? 2.5 : 3), 0, Math.PI * 2)
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

  // 기본 브랜치 tip 이 정해지면 그래프에 알린다. 이미 계산한 뒤에 정해졌다면 읽어 둔 커밋으로 레인만 다시 계산해 굵은 줄기를 반영한다(로그는 다시 읽지 않는다)
  const defaultTip = useAppStore((s) => s.refs?.defaultBranch?.hash ?? null)
  useEffect(() => {
    // 목록이 지금 활성 저장소의 것일 때만 넘긴다(저장소 전환 중에는 이전 저장소의 커밋을 다시 계산하지 않는다)
    const log = useLogStore.getState()
    graphEngine.setDefaultTip(defaultTip, log.repoId === useAppStore.getState().activeRepoId ? log.commits : [])
  }, [defaultTip])

  // 아바타 주소가 해석되거나 이미지가 도착하면 다시 그린다(도착은 프레임당 한 번으로 합쳐 온다)
  useEffect(() => {
    const offStore = useAvatarStore.subscribe((s, prev) => {
      if (s.urls !== prev.urls) schedule()
    })
    const offImg = onAvatarImage(schedule)
    return () => {
      offStore()
      offImg()
    }
  }, [])

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
  useEffect(schedule, [version, width, commits, avatars, topOffset])

  // data-lane-version: 레인 계산 결과가 바뀔 때마다 증가(자동 측정 스크립트가 그래프까지 안정됐는지 보는 신호)
  return <canvas ref={canvasRef} className="graph-canvas" style={{ left }} aria-hidden="true" data-lane-version={version} />
}
