// Rust 가 설정을 소유하는 실행(Phase 6b PR 10)에서 사이드카가 등록하는 처리기 — 앱 업데이트 `update:*` 7개뿐이다.
// 처리기 본문은 src/main/ipc/handlers.ts 의 registerIpc 앞부분과 같다(Electron 판이 쓰는 파일이라 건드리지 않고 옮겼다 — tests/sidecarUpdateIpc.test.ts 가 소스를 대조한다).
import type { IpcEnvelope } from '@shared/ipc'
import type { UpdateService } from '../main/update/UpdateService'
import { ipcMain } from './electronShim'

type Register = (channel: string, fn: (e: { sender: object }, ...args: unknown[]) => unknown) => void

/** 결과를 IpcEnvelope 로 감싸 등록한다(handlers.ts 의 `handle` 과 같다 — update 채널의 오류는 모두 code 'ERROR') */
export function registerUpdateIpc(updates: UpdateService, register: Register = (ch, fn) => ipcMain.handle(ch, fn)): void {
  const handle = (channel: string, fn: (...args: never[]) => unknown): void =>
    register(channel, async (_e, ...args): Promise<IpcEnvelope<unknown>> => {
      try {
        return { ok: true, data: await (fn as (...a: unknown[]) => unknown)(...args) }
      } catch (err) {
        return { ok: false, error: err instanceof Error ? err.message : String(err), code: 'ERROR' }
      }
    })
  handle('update:get', () => updates.get())
  handle('update:check', () => updates.check(true))
  handle('update:download', () => updates.download())
  handle('update:cancel', () => updates.cancel())
  handle('update:install', () => updates.install())
  handle('update:dismiss', (action: Parameters<UpdateService['dismiss']>[0], version: string) => updates.dismiss(action, version))
  handle('update:releaseNotes', (version?: string) => updates.releaseNotes(version))
}
