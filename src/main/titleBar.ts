import type { BrowserWindow } from 'electron'

/**
 * 제목 표시줄 오버레이. 앱 툴바가 곧 제목 표시줄이고, Windows·Linux 의 창 컨트롤만 OS 가 그 위에 그린다.
 * color 는 토큰 --toolbar-bg(= --bg-surface), symbolColor 는 --text-primary 와 같은 값이어야 한다.
 * 툴바 높이는 --toolbar-h(44) 와 같아야 한다.
 * 오버레이는 툴바보다 1px 낮게(OVERLAY_HEIGHT) 잡는다 — 툴바 맨 아래 1px 는 border-bottom 인데, 오버레이가 같은 높이면
 * 창 컨트롤 영역에서 그 경계선이 OS 가 그리는 오버레이에 덮여 선이 끊겨 보인다.
 */
export const TOOLBAR_HEIGHT = 44
export const OVERLAY_HEIGHT = TOOLBAR_HEIGHT - 1

const OVERLAY = {
  dark: { color: '#1C1815', symbolColor: '#EDE6D8' },
  light: { color: '#EFE8DA', symbolColor: '#2A211B' }
}

/**
 * 모달이 떠 있을 때 화면을 덮는 --backdrop(테마별 rgb + 알파). 창 컨트롤은 OS 가 웹 화면 위에 따로 그려
 * CSS 배경막이 덮지 못하므로, 같은 색을 섞은 값으로 오버레이 색을 바꿔 dim 을 흉내 낸다.
 */
const BACKDROP = {
  dark: { rgb: [0, 0, 0], alpha: 0.45 },
  light: { rgb: [42, 33, 27], alpha: 0.18 }
}

function blend(hex: string, over: { rgb: number[]; alpha: number }): string {
  const n = parseInt(hex.slice(1), 16)
  const base = [(n >> 16) & 255, (n >> 8) & 255, n & 255]
  return `#${base
    .map((c, i) => Math.round(c * (1 - over.alpha) + over.rgb[i]! * over.alpha))
    .map((c) => c.toString(16).padStart(2, '0'))
    .join('')}`
}

export function titleBarOverlay(dark: boolean, dim = false): { color: string; symbolColor: string; height: number } {
  const o = dark ? OVERLAY.dark : OVERLAY.light
  if (!dim) return { ...o, height: OVERLAY_HEIGHT }
  const b = dark ? BACKDROP.dark : BACKDROP.light
  return { color: blend(o.color, b), symbolColor: blend(o.symbolColor, b), height: OVERLAY_HEIGHT }
}

/** Windows·Linux 에서만 동작한다(macOS 는 신호등 버튼이라 오버레이가 없다) */
export function applyTitleBarTheme(win: BrowserWindow, dark: boolean, dim = false): void {
  if (process.platform === 'darwin') return
  try {
    win.setTitleBarOverlay(titleBarOverlay(dark, dim))
  } catch {
    // 오버레이가 없는 창이거나 지원하지 않는 환경: 색만 어긋나므로 무시
  }
}
