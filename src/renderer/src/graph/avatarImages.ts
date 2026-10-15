// 그래프 노드용 아바타 이미지 로더와 스프라이트 캐시.
// 노드마다 clip/drawImage 를 새로 하지 않도록 (주소, 반지름, dpr, 레인) 별로 원형 스프라이트를 한 번만 만들어 두고
// 프레임에서는 drawImage 한 번으로 찍는다. 이미지가 도착하면 구독자에게 한꺼번에(프레임당 한 번) 알린다.

type Slot = HTMLImageElement | 'loading' | 'failed'

const MAX_IMAGES = 600
const MAX_SPRITES = 2500
const images = new Map<string, Slot>()
const listeners = new Set<() => void>()
let notifyFrame = 0

function notifyLater(): void {
  if (notifyFrame) return
  notifyFrame = requestAnimationFrame(() => {
    notifyFrame = 0
    for (const fn of listeners) fn()
  })
}

/** 이미지가 새로 도착(또는 실패 확정)할 때마다 불린다. 한 프레임 안의 여러 도착은 한 번으로 합친다 */
export function onAvatarImage(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

/** src 의 로드된 이미지. 아직이면 로드를 시작하고 null, 실패한 주소도 null(이니셜로 대신한다) */
export function avatarImage(src: string): HTMLImageElement | null {
  const slot = images.get(src)
  if (slot === undefined) {
    if (images.size >= MAX_IMAGES) {
      // 가장 오래된 것부터 버린다(Map 은 삽입 순서)
      const oldest = images.keys().next().value
      if (oldest !== undefined) images.delete(oldest)
    }
    const img = new Image()
    // CORS 헤더가 있는 호스트라 canvas 가 오염되지 않는다. <img> 쪽과 같은 모드라 브라우저 캐시도 공유한다
    img.crossOrigin = 'anonymous'
    img.referrerPolicy = 'no-referrer'
    img.decoding = 'async'
    images.set(src, 'loading')
    img.onload = () => {
      if (images.get(src) === 'loading') images.set(src, img)
      notifyLater()
    }
    img.onerror = () => {
      if (images.get(src) === 'loading') images.set(src, 'failed')
      notifyLater()
    }
    img.src = src
    return null
  }
  return typeof slot === 'object' ? slot : null
}

export interface Sprite {
  canvas: HTMLCanvasElement
  /** CSS px 단위 한 변의 길이(중심이 한가운데) */
  size: number
}

const sprites = new Map<string, Map<number, Sprite>>()
let spriteEnv = ''
let spriteCount = 0

/**
 * 레인 색 링 + 원형으로 잘린 아바타 스프라이트. 링 바깥에는 배경색 테두리를 둬 뒤를 지나는 선과 분리한다.
 * env 는 스프라이트가 의존하는 값(반지름·dpr·배경색)의 요약 — 바뀌면 캐시를 비운다.
 */
export function avatarSprite(img: HTMLImageElement, src: string, lane: number, laneColor: string, bg: string, r: number, dpr: number): Sprite {
  const env = `${r}|${dpr}|${bg}`
  if (env !== spriteEnv || spriteCount > MAX_SPRITES) {
    sprites.clear()
    spriteCount = 0
    spriteEnv = env
  }
  let byLane = sprites.get(src)
  if (!byLane) sprites.set(src, (byLane = new Map()))
  let sp = byLane.get(lane)
  if (sp) return sp

  const size = Math.ceil((r + 1) * 2)
  const canvas = document.createElement('canvas')
  canvas.width = Math.ceil(size * dpr)
  canvas.height = Math.ceil(size * dpr)
  const ctx = canvas.getContext('2d')!
  ctx.scale(dpr, dpr)
  const c = size / 2
  const ring = 1.5
  ctx.fillStyle = bg
  ctx.beginPath()
  ctx.arc(c, c, r + 0.75, 0, Math.PI * 2)
  ctx.fill()
  ctx.fillStyle = laneColor
  ctx.beginPath()
  ctx.arc(c, c, r, 0, Math.PI * 2)
  ctx.fill()
  ctx.save()
  ctx.beginPath()
  ctx.arc(c, c, r - ring, 0, Math.PI * 2)
  ctx.clip()
  ctx.imageSmoothingQuality = 'high'
  ctx.drawImage(img, c - (r - ring), c - (r - ring), (r - ring) * 2, (r - ring) * 2)
  ctx.restore()
  sp = { canvas, size }
  byLane.set(lane, sp)
  spriteCount++
  return sp
}
