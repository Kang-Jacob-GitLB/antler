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
  | { t: 'hostres'; id: number; ok: boolean; value?: unknown }

/**
 * Node 가 Rust 에 부탁하는 OS 동작. arg·value 모양:
 * - openExternal: arg 문자열(주소) → value 없음
 * - clipboard: arg 문자열(텍스트) → value 없음
 * - messageBox: arg {@link MessageBoxArg} → value `{ response: number }`(눌린 buttons 인덱스)
 * - pickPath: arg {@link PickPathArg} → value `{ filePaths: string[] }`(취소면 빈 배열)
 * ok:false 는 실패(창 없음·대화상자 오류 포함)이며, 호출부가 취소로 취급한다.
 */
export type HostOp = 'openExternal' | 'clipboard' | 'messageBox' | 'pickPath'

/** Node → Rust */
export type FromSidecar =
  | { t: 'res'; id: number; env: IpcEnvelope<unknown> }
  | { t: 'evt'; name: string; payload: unknown }
  | { t: 'host'; id: number; op: HostOp; arg: unknown }
  | { t: 'ready' }

/**
 * messageBox 요청. 네이티브 대화상자는 체크박스와 기본 버튼 지정을 지원하지 않는다.
 * cancelId 버튼은 Rust 가 마지막(Esc·닫기와 같은 자리)에 둔다 — src-tauri/src/dialogs.rs 참고
 */
export interface MessageBoxArg {
  kind: 'info' | 'warning' | 'error'
  title: string
  /** message 와 detail 을 이어 붙인 본문 */
  message: string
  /** 1~3개, 서로 달라야 한다(눌린 버튼을 문구로 구분한다) */
  buttons: string[]
  cancelId: number
}

/** pickPath 요청 */
export interface PickPathArg {
  mode: 'folder' | 'file'
  title: string
  defaultPath?: string
  filters?: { name: string; extensions: string[] }[]
}
