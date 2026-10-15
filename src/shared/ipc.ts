// IPC 계약. 채널 이름과 인자·결과 타입을 한곳에서 정의해 main/preload/renderer 가 공유한다.
import type { AppConfig, LogPage, LogQuery, RefsSnapshot, Repo, SearchResult, UiPrefs } from './types'

export interface IpcContract {
  'config:get': { args: []; result: AppConfig }
  'config:setUi': { args: [patch: Partial<UiPrefs>]; result: AppConfig }
  /** 폴더 선택 대화상자를 띄워 로컬 저장소를 추가한다. 취소하면 null */
  'repos:pickLocal': { args: []; result: Repo | null }
  'repos:remove': { args: [repoId: string]; result: AppConfig }
  /** 사이드바용 ref 스냅샷(HEAD·브랜치·리모트·태그·워크트리·스태시) */
  'git:refs': { args: [repoId: string]; result: RefsSnapshot }
  'git:log': { args: [repoId: string, query: LogQuery, requestId?: string]; result: LogPage }
  /** 메시지·작성자·해시 검색. ref 를 주면 그 ref 기준 이력에서만 찾는다 */
  'git:search': { args: [repoId: string, query: string, ref?: string]; result: SearchResult }
  /** 진행 중인 요청 취소 */
  'req:cancel': { args: [requestId: string]; result: void }
}

export type IpcChannel = keyof IpcContract
export type IpcArgs<C extends IpcChannel> = IpcContract[C]['args']
export type IpcResult<C extends IpcChannel> = IpcContract[C]['result']

/** main 이 renderer 로 밀어주는 이벤트 */
export interface IpcEvents {
  'repo:changed': { repoId: string }
  'theme:system': { dark: boolean }
}
export type IpcEvent = keyof IpcEvents

/** preload 에서 허용 목록으로 쓰는 런타임 채널 목록 */
export const IPC_CHANNELS = [
  'config:get',
  'config:setUi',
  'repos:pickLocal',
  'repos:remove',
  'git:refs',
  'git:search',
  'git:log',
  'req:cancel'
] as const satisfies readonly IpcChannel[]

export const IPC_EVENTS = ['repo:changed', 'theme:system'] as const satisfies readonly IpcEvent[]

/** invoke 결과 래퍼. Electron 기본 에러 문자열("Error invoking remote method…")을 피하려고 쓴다. */
export type IpcEnvelope<T> = { ok: true; data: T } | { ok: false; error: string; code?: string }

/** window.antler 로 노출되는 API 형태 */
export interface AntlerApi {
  platform: 'win32' | 'darwin' | 'linux' | string
  invoke<C extends IpcChannel>(channel: C, ...args: IpcArgs<C>): Promise<IpcResult<C>>
  on<E extends IpcEvent>(event: E, listener: (payload: IpcEvents[E]) => void): () => void
}
