// 창 위치·크기 기억: 저장한 값의 검사와, 지금 연결된 화면에 맞춰 되살릴 위치 계산(순수 로직)
import type { WindowState } from '@shared/types'

export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

export const MIN_WINDOW = { width: 960, height: 600 }
export const DEFAULT_WINDOW = { width: 1440, height: 900 }

const isInt = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && Math.abs(v) <= 100_000

/** 설정 파일에서 읽은 값의 모양 검사. 이상하면 undefined(기본 크기로 연다) */
export function sanitizeWindowState(v: unknown): WindowState | undefined {
  if (!v || typeof v !== 'object') return undefined
  const { x, y, width, height, maximized } = v as Record<string, unknown>
  if (!isInt(x) || !isInt(y) || !isInt(width) || !isInt(height) || width <= 0 || height <= 0) return undefined
  return { x, y, width, height, ...(maximized === true ? { maximized: true } : {}) }
}

function overlap(a: Rect, b: Rect): number {
  const w = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x)
  const h = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y)
  return w > 0 && h > 0 ? w * h : 0
}

/**
 * 저장한 창 영역을 지금 화면들의 작업 영역에 맞춘다.
 * 가장 많이 겹치는 화면을 고르고(모니터를 뺐으면 겹치는 화면이 없어 첫 화면), 그 화면보다 크면 줄이고 밖으로 나간 만큼 안으로 민다.
 * 저장한 값이 없거나 화면 정보가 없으면 undefined — Electron 기본(가운데)에 맡긴다.
 */
export function restoreBounds(saved: WindowState | undefined, workAreas: Rect[]): Rect | undefined {
  if (!saved || workAreas.length === 0) return undefined
  const best = workAreas.reduce((a, b) => (overlap(saved, b) > overlap(saved, a) ? b : a), workAreas[0]!)
  const visible = overlap(saved, best) > 0
  const area = visible ? best : workAreas[0]!
  const width = Math.max(Math.min(saved.width, area.width), Math.min(MIN_WINDOW.width, area.width))
  const height = Math.max(Math.min(saved.height, area.height), Math.min(MIN_WINDOW.height, area.height))
  // 어느 화면에도 걸치지 않으면(모니터 분리) 첫 화면 가운데에 둔다
  const sx = visible ? saved.x : area.x + Math.round((area.width - width) / 2)
  const sy = visible ? saved.y : area.y + Math.round((area.height - height) / 2)
  const x = Math.min(Math.max(sx, area.x), area.x + area.width - width)
  const y = Math.min(Math.max(sy, area.y), area.y + area.height - height)
  return { x, y, width, height }
}
