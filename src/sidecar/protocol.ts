// Rust(src-tauri/src/sidecar.rs) ↔ Node 사이드카의 stdio JSON-lines 프로토콜. 줄 하나가 메시지 하나다.
// Rust 는 채널을 해석하지 않고 req 를 그대로 중계한다.
import type { IpcEnvelope } from '@shared/ipc'

/** Rust → Node */
export type ToSidecar =
  | { t: 'req'; id: number; ch: string; args: unknown[] }
  | { t: 'focus'; focused: boolean }
  | { t: 'hostres'; id: number; ok: boolean }

/** Node 가 Rust 에 부탁하는 OS 동작 */
export type HostOp = 'openExternal' | 'clipboard'

/** Node → Rust */
export type FromSidecar =
  | { t: 'res'; id: number; env: IpcEnvelope<unknown> }
  | { t: 'evt'; name: string; payload: unknown }
  | { t: 'host'; id: number; op: HostOp; arg: string }
  | { t: 'ready' }
