// Tauri 용 window.antler 어댑터. Electron 의 preload(src/preload/index.ts)와 같은 규약(허용 목록·"[CODE] msg" 에러)으로
// Rust 의 antler_invoke 를 부른다. React 가 렌더되기 전에 실행돼야 하므로 vite 플러그인이 main.tsx 앞에 끼워 넣는다.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { IPC_CHANNELS, IPC_EVENTS, type AntlerApi, type IpcEnvelope } from '@shared/ipc'
import { UNDEFINED_ARG } from '../../sidecar/protocol'
import './titlebar.css'

const channels = new Set<string>(IPC_CHANNELS)
const events = new Set<string>(IPC_EVENTS)

type Listener = (payload: never) => void
const listeners = new Map<string, Set<Listener>>()

void listen<{ name: string; payload: unknown }>('antler:evt', (e) => {
  for (const l of listeners.get(e.payload.name) ?? []) (l as (p: unknown) => void)(e.payload.payload)
})

/**
 * JSON 은 배열 속 undefined 를 null 로 바꾼다. Electron IPC(structured clone)는 undefined 를 그대로 넘기고 main 의 검증은
 * "생략(undefined)" 과 "잘못된 값(null)" 을 구분한다(예: git:search 의 scope 가 null 이면 invalid scope).
 * 그래서 빈 자리는 표식으로 보내고 사이드카가 undefined 로 되돌린다. 끝의 undefined 는 그냥 뗀다(인자 생략과 같다).
 */
function encodeArgs(args: unknown[]): unknown[] {
  let end = args.length
  while (end > 0 && args[end - 1] === undefined) end--
  return args.slice(0, end).map((a) => (a === undefined ? { [UNDEFINED_ARG]: true } : a))
}

const root = document.documentElement
root.dataset['shell'] = 'tauri'

const api: AntlerApi = {
  platform: 'win32',
  async invoke(channel, ...args) {
    if (!channels.has(channel)) throw new Error(`blocked channel: ${channel}`)
    // 창 컨트롤은 bridge 가 직접 그리므로 제목 표시줄 색은 CSS 로 처리한다(Rust 로 보내지 않는다)
    if (channel === 'win:setTitleBarTheme') {
      const theme = args[0] as { dim?: boolean } | undefined
      root.toggleAttribute('data-titlebar-dim', theme?.dim === true)
      return undefined as never
    }
    const res = (await invoke<IpcEnvelope<never>>('antler_invoke', { channel, args: encodeArgs(args) })) as IpcEnvelope<never>
    if (res.ok) return res.data
    throw new Error(`[${res.code ?? 'ERROR'}] ${res.error}`)
  },
  on(event, listener) {
    if (!events.has(event)) throw new Error(`blocked event: ${event}`)
    let set = listeners.get(event)
    if (!set) listeners.set(event, (set = new Set()))
    const l = listener as Listener
    set.add(l)
    return () => {
      set.delete(l)
    }
  }
}
window.antler = api

// ── 창 버튼 ─────────────────────────────────────────────
const win = getCurrentWindow()
const SNAP_HOVER_MS = 620

function makeButton(cls: string, label: string, glyph: string): HTMLButtonElement {
  const b = document.createElement('button')
  b.type = 'button'
  b.className = `tb-btn ${cls}`
  b.setAttribute('aria-label', label)
  b.tabIndex = -1
  b.textContent = glyph
  return b
}

function mountWindowControls(): void {
  const box = document.createElement('div')
  box.className = 'tb-controls'
  const min = makeButton('tb-btn--min', '최소화', '')
  const max = makeButton('tb-btn--max', '최대화', '')
  const close = makeButton('tb-btn--close', '닫기', '')
  box.append(min, max, close)
  document.body.append(box)

  min.addEventListener('click', () => void win.minimize())
  max.addEventListener('click', () => void win.toggleMaximize())
  close.addEventListener('click', () => void win.close())

  const syncMax = async (): Promise<void> => {
    const maximized = await win.isMaximized()
    max.textContent = maximized ? '' : ''
    max.setAttribute('aria-label', maximized ? '이전 크기로 복원' : '최대화')
  }
  void win.onResized(() => void syncMax())
  void syncMax()

  // 최대화 버튼에 잠시 머물면 Win11 스냅 레이아웃(Win+Z 플라이아웃)을 띄운다
  let snapTimer: number | undefined
  max.addEventListener('mouseenter', () => {
    window.clearTimeout(snapTimer)
    snapTimer = window.setTimeout(() => void invoke('show_snap_overlay'), SNAP_HOVER_MS)
  })
  max.addEventListener('mouseleave', () => window.clearTimeout(snapTimer))
  max.addEventListener('click', () => window.clearTimeout(snapTimer))
}

// ── 툴바 드래그 ─────────────────────────────────────────
// WebView2 가 CSS app-region 을 따르면 이 핸들러는 드래그 영역에서 호출되지 않는다(네이티브가 먼저 처리).
// 따르지 않는 경우를 위한 대체: 위로 올라가며 처음 drag/no-drag 인 요소를 찾는다. 계산값을 읽지 못하면 Toolbar.css 의 no-drag 목록을 쓴다.
const NO_DRAG = '.crumb, .view-chip, .crumb-more, .filter-chip, .toolbar__actions, .panel-toggles, button, input, a, select, textarea'

function regionOf(el: Element): string {
  const cs = getComputedStyle(el)
  return (cs.getPropertyValue('app-region') || cs.getPropertyValue('-webkit-app-region')).trim()
}

function isDragTarget(target: Element, toolbar: Element): boolean {
  if (regionOf(toolbar) === 'drag') {
    for (let el: Element | null = target; el; el = el.parentElement) {
      const r = regionOf(el)
      if (r === 'drag') return true
      if (r === 'no-drag') return false
      if (el === toolbar) break
    }
    return false
  }
  return target.closest(NO_DRAG) === null
}

function installToolbarDrag(): void {
  document.addEventListener(
    'mousedown',
    (e) => {
      if (e.button !== 0 || !(e.target instanceof Element)) return
      const toolbar = e.target.closest('.toolbar')
      if (!toolbar || !isDragTarget(e.target, toolbar)) return
      if (e.detail === 2) void win.toggleMaximize()
      else void win.startDragging()
    },
    true
  )
}

// ── 측정 모드: 첫 커밋 행이 DOM 에 나타난 시각 ────────────
function watchFirstRow(): void {
  const mo = new MutationObserver(() => {
    if (!document.querySelector('.commit-subject')) return
    ;(window as unknown as Record<string, unknown>)['__ANTLER_FIRST_ROW__'] = performance.now()
    mo.disconnect()
  })
  mo.observe(document.documentElement, { childList: true, subtree: true })
}

const start = (): void => {
  mountWindowControls()
  installToolbarDrag()
  if ((window as unknown as Record<string, unknown>)['__ANTLER_MEASURE__']) watchFirstRow()
}
if (document.body) start()
else document.addEventListener('DOMContentLoaded', start, { once: true })
