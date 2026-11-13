// src/main 이 쓰는 electron API 만 흉내 낸다(사이드카 번들에서 `electron` 을 이 파일로 바꾼다).
// 창·대화상자 같은 OS UI 는 없다 — 사용자 확인이 필요한 경로는 "거부"로 닫는다(보안 가드를 열지 않는다).
import type { FromSidecar, HostOp } from './protocol'

type Send = (msg: FromSidecar) => void
let send: Send = () => {}
let seq = 0
const pending = new Map<number, (ok: boolean) => void>()

/** index.ts 가 stdout 쓰기 함수를 연결한다 */
export function bindHost(fn: Send): void {
  send = fn
}

/** Rust 가 돌려준 host 응답을 대기 중인 요청에 넘긴다 */
export function resolveHost(id: number, ok: boolean): void {
  pending.get(id)?.(ok)
  pending.delete(id)
}

function host(op: HostOp, arg: string): Promise<void> {
  const id = ++seq
  return new Promise<void>((resolve, reject) => {
    pending.set(id, (ok) => (ok ? resolve() : reject(new Error(`호스트 동작 실패: ${op}`))))
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

export const dialog = {
  /**
   * 확인 창이 없으므로 항상 취소로 답한다. handlers.ts 에서 response 0/1 은 "fetch 허용"·"SSH 연결"이라
   * 임의 값을 돌려주면 보안 가드가 열린다 — cancelId 가 없으면 취소 응답을 정할 수 없으니 던진다.
   */
  async showMessageBox(...a: unknown[]): Promise<{ response: number; checkboxChecked: boolean }> {
    const opts = (a.length > 1 ? a[1] : a[0]) as { cancelId?: unknown } | undefined
    if (typeof opts?.cancelId !== 'number') throw new Error('사이드카: cancelId 없는 확인 창은 지원하지 않습니다')
    return { response: opts.cancelId, checkboxChecked: false }
  },
  async showOpenDialog(..._a: unknown[]): Promise<{ canceled: boolean; filePaths: string[] }> {
    return { canceled: true, filePaths: [] }
  }
}

export const BrowserWindow = {
  fromWebContents(_wc: unknown): null {
    return null
  }
}

export const shell = {
  openExternal: (url: string): Promise<void> => host('openExternal', url),
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
