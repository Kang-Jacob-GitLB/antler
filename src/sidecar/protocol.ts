// Rust(src-tauri/src/sidecar.rs) ↔ Node 사이드카의 stdio JSON-lines 프로토콜. 줄 하나가 메시지 하나다.
// Rust 는 채널을 해석하지 않고 req 를 그대로 중계한다.
import type { IpcEnvelope } from '@shared/ipc'

/**
 * 인자 배열의 빈 자리(undefined) 표식. JSON 은 배열 속 undefined 를 null 로 바꾸므로 bridge 가 이 키 하나짜리 객체로 보내고
 * 사이드카가 undefined 로 되돌린다(src/renderer/tauri/bridge.ts 의 encodeArgs, src/sidecar/index.ts 의 decodeArgs).
 */
export const UNDEFINED_ARG = '__antlerUndefinedArg'

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
