// src/main 이 쓰는 electron API 만 흉내 낸다(사이드카 번들에서 `electron` 을 이 파일로 바꾼다).
// 창·대화상자 같은 OS UI 는 Rust 에 부탁한다(host 요청). 사용자 확인이 필요한 경로는 어떤 실패에서도 "거부"(cancelId)로 닫는다(보안 가드를 열지 않는다).
import type { FromSidecar, HostOp, MessageBoxArg, PickPathArg } from './protocol'

type Send = (msg: FromSidecar) => void
let send: Send = () => {}
let seq = 0
const pending = new Map<number, (ok: boolean, value: unknown) => void>()

/** index.ts 가 stdout 쓰기 함수를 연결한다 */
export function bindHost(fn: Send): void {
  send = fn
}

/** Rust 가 돌려준 host 응답을 대기 중인 요청에 넘긴다 */
export function resolveHost(id: number, ok: boolean, value?: unknown): void {
  pending.get(id)?.(ok, value)
  pending.delete(id)
}

function host(op: HostOp, arg: unknown): Promise<unknown> {
  const id = ++seq
  return new Promise<unknown>((resolve, reject) => {
    pending.set(id, (ok, value) => (ok ? resolve(value) : reject(new Error(`호스트 동작 실패: ${op}`))))
    send({ t: 'host', id, op, arg })
  })
}

type Handler = (e: { sender: object }, ...args: unknown[]) => unknown
const handlers = new Map<string, Handler>()

export const ipcMain = {
  handle(channel: string, fn: Handler): void {
    handlers.set(channel, fn)
  }
}

/** 등록된 채널 호출. 없으면 undefined */
export function invokeHandler(channel: string, args: unknown[]): Promise<unknown> | undefined {
  const fn = handlers.get(channel)
  return fn ? Promise.resolve(fn({ sender: {} }, ...args)) : undefined
}

type MessageBoxOpts = {
  type?: unknown
  title?: unknown
  message?: unknown
  detail?: unknown
  buttons?: unknown
  defaultId?: unknown
  cancelId?: unknown
}

const MESSAGE_KINDS: Record<string, MessageBoxArg['kind']> = { warning: 'warning', error: 'error' }

/** 대화상자 인자(창이 있으면 두 번째, 없으면 첫 번째)에서 옵션을 꺼낸다 */
function dialogOpts<T>(a: unknown[]): T | undefined {
  return (a.length > 1 ? a[1] : a[0]) as T | undefined
}

export const dialog = {
  /**
   * Rust 네이티브 대화상자(부모 = 메인 창, 모달)로 묻는다. handlers.ts 에서 response 0/1 은 "fetch 허용"·"SSH 연결"이라
   * 임의 값이 보안 가드를 연다 — 대화상자 실패·잘못된 응답·범위 밖 값은 모두 cancelId 로 답한다.
   * cancelId 가 없으면 취소 응답을 정할 수 없으니 던진다. 네이티브 대화상자에는 체크박스가 없어 checkboxChecked 는 항상 false 다.
   */
  async showMessageBox(...a: unknown[]): Promise<{ response: number; checkboxChecked: boolean }> {
    const opts = dialogOpts<MessageBoxOpts>(a)
    if (typeof opts?.cancelId !== 'number') throw new Error('사이드카: cancelId 없는 확인 창은 지원하지 않습니다')
    const cancel = { response: opts.cancelId, checkboxChecked: false }
    const buttons = opts.buttons
    if (!Array.isArray(buttons) || buttons.length < 1 || buttons.length > 3) return cancel
    if (!Number.isInteger(opts.cancelId) || opts.cancelId < 0 || opts.cancelId >= buttons.length) return cancel
    if (!buttons.every((b) => typeof b === 'string' && b !== '') || new Set(buttons).size !== buttons.length) return cancel
    const str = (v: unknown): string => (typeof v === 'string' ? v : '')
    const arg: MessageBoxArg = {
      kind: MESSAGE_KINDS[str(opts.type)] ?? 'info',
      title: str(opts.title),
      message: [str(opts.message), str(opts.detail)].filter(Boolean).join('\n\n'),
      buttons: buttons as string[],
      cancelId: opts.cancelId
    }
    try {
      const v = (await host('messageBox', arg)) as { response?: unknown } | null
      const r = v?.response
      return typeof r === 'number' && Number.isInteger(r) && r >= 0 && r < buttons.length ? { response: r, checkboxChecked: false } : cancel
    } catch {
      return cancel
    }
  },
  /** 폴더·파일 선택. 실패는 취소로 취급한다 */
  async showOpenDialog(...a: unknown[]): Promise<{ canceled: boolean; filePaths: string[] }> {
    const opts = dialogOpts<{ properties?: unknown; title?: unknown; defaultPath?: unknown; filters?: unknown }>(a)
    const props = Array.isArray(opts?.properties) ? opts.properties : []
    const arg: PickPathArg = {
      mode: props.includes('openDirectory') ? 'folder' : 'file',
      title: typeof opts?.title === 'string' ? opts.title : ''
    }
    if (typeof opts?.defaultPath === 'string' && opts.defaultPath) arg.defaultPath = opts.defaultPath
    if (Array.isArray(opts?.filters)) {
      arg.filters = (opts.filters as { name?: unknown; extensions?: unknown }[])
        .filter((f) => typeof f?.name === 'string' && Array.isArray(f.extensions))
        .map((f) => ({ name: f.name as string, extensions: (f.extensions as unknown[]).filter((x): x is string => typeof x === 'string') }))
    }
    try {
      const v = (await host('pickPath', arg)) as { filePaths?: unknown } | null
      const paths = Array.isArray(v?.filePaths) ? v.filePaths.filter((p): p is string => typeof p === 'string' && p !== '') : []
      return { canceled: paths.length === 0, filePaths: paths.slice(0, 1) }
    } catch {
      return { canceled: true, filePaths: [] }
    }
  }
}

export const BrowserWindow = {
  fromWebContents(_wc: unknown): null {
    return null
  }
}

export const shell = {
  openExternal: (url: string): Promise<void> => host('openExternal', url).then(() => undefined),
  async openPath(_p: string): Promise<string> {
    throw new Error('사이드카(PoC): openPath 미지원')
  },
  showItemInFolder(_p: string): void {
    throw new Error('사이드카(PoC): showItemInFolder 미지원')
  }
}

export const clipboard = {
  // Electron 의 writeText 는 동기라 호출부가 await 하지 않는다 — 거부가 처리되지 않은 rejection 으로 남지 않게 여기서 로그만 남긴다
  writeText(text: string): void {
    host('clipboard', text).catch((err) => console.error('클립보드 쓰기 실패:', err))
  }
}

// index.ts 전용 API 는 사이드카 index 가 직접 대체한다. 번들에 import 가 남아도 깨지지 않게 하는 최소 스텁
export const app = { isPackaged: false }
export const safeStorage = {
  isEncryptionAvailable: (): boolean => false,
  encryptString: (_s: string): Buffer => Buffer.alloc(0),
  decryptString: (_b: Buffer): string => ''
}
export const nativeTheme = { shouldUseDarkColors: true, on(): void {} }
