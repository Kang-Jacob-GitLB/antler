// Rust(src-tauri/src/sidecar.rs) ↔ Node 사이드카의 stdio JSON-lines 프로토콜. 줄 하나가 메시지 하나다.
// Rust 는 채널을 해석하지 않고 req 를 그대로 중계한다.
import type { IpcEnvelope } from '@shared/ipc'

/**
 * 인자 배열의 빈 자리(undefined) 표식. JSON 은 배열 속 undefined 를 null 로 바꾸므로 bridge 가 이 키 하나짜리 객체로 보내고
 * 사이드카가 undefined 로 되돌린다(src/renderer/tauri/bridge.ts 의 encodeArgs, src/sidecar/index.ts 의 decodeArgs).
 */
export const UNDEFINED_ARG = '__antlerUndefinedArg'

/**
 * Rust 가 `git:log`·fetch 를 직접 처리할 때 사이드카의 RepoService 에 알리는 일(`RepoService.noteLog`). fetchChanged 는 fetch 가 ref 를 바꿨다 —
 * 기억한 시그니처를 버린다(Rust 는 이 note 를 먼저 보내고 그다음 `repo:changed` 를 낸다).
 * 이 메시지는 Rust 만 만든다 — renderer 의 값은 항상 `req` 의 `args` 안에만 들어가고 `t` 는 Rust 가 정한다.
 */
export const NOTE_OPS = ['logFirstPage', 'logStale', 'fetchChanged'] as const
export type NoteOp = (typeof NOTE_OPS)[number]

/**
 * Rust 의 fetch 가 설정 파일에 남겨 달라고 부탁하는 승인의 종류.
 * - approval: 수동 fetch 가 성공한 remote(이름+URL 지문) → `ConfigStore.setFetchApproval`
 * - trusted: 확인 창에서 "항상 허용" 한 위험 설정 지문 → `ConfigStore.setTrustedConfig`
 */
export const FETCH_APPROVE_KINDS = ['approval', 'trusted'] as const
export type FetchApproveKind = (typeof FETCH_APPROVE_KINDS)[number]

/**
 * Rust 가 사이드카에 묻는 일(GitHub 서비스가 저장소 정보를 얻는다 — `repoq`). 닫힌 목록이고 사이드카가 다시 검사한다(src/sidecar/repoQuery.ts).
 * Rust 의 `repoq::Op` 과 같아야 한다(Rust 테스트가 이 줄을 읽어 대조한다).
 */
export const REPOQ_OPS = ['ghRepo', 'ghRemotes', 'pushedShas', 'invalidateGhRemotes', 'githubClientId', 'repoIds'] as const
export type RepoqOp = (typeof REPOQ_OPS)[number]

/** Rust → Node */
export type ToSidecar =
  | { t: 'req'; id: number; ch: string; args: unknown[] }
  | { t: 'note'; op: NoteOp; repoId: string }
  /**
   * 승인 저장 부탁. 승인을 쓰는 **유일한** 길이다 — renderer 의 `req` 경로(채널)로는 받지 않는다. 사이드카가 kind 허용 목록·저장소 존재·지문 모양(소문자 hex 64자)을 다시 검사한다.
   * 보내는 곳은 Rust fetch 서비스의 두 지점(수동 fetch 성공 뒤, "항상 허용" 응답 뒤)뿐이다
   */
  | { t: 'fetchApprove'; kind: FetchApproveKind; repoId: string; fingerprint: string }
  | { t: 'focus'; focused: boolean }
  /**
   * 저장소 정보 질의. `id` 가 있으면 `res`(`{t:'res',id,env}`)로 답하고 없으면 알림이다(invalidateGhRemotes). 매개변수는 `p` 안에만 있다.
   * ghRepo `{repoId}` → GhRepoRef|null, ghRemotes `{repoId}` → GhRepoRef[], pushedShas `{repoId,email,limit}` → sha[], githubClientId → string|null, repoIds → string[]
   */
  | { t: 'repoq'; id?: number; op: RepoqOp; p: Record<string, unknown> }
  /** 응답을 기다리다 버린(시간 초과·취소) 질의 — 사이드카가 그 질의의 AbortSignal 을 끊는다 */
  | { t: 'repoqCancel'; id: number }
  /**
   * GitHub 토큰의 메모리 사본(없으면 null). Rust 가 GitHub 를 처리할 때(env ANTLER_RUST_API=1)만, 시작 때 한 번과 로그인 상태가 바뀔 때마다 보낸다.
   * 사이드카에 남는 일(원격 전용 저장소의 git fetch 인증)이 쓴다. 로그·오류·env 에 싣지 않는다
   */
  | { t: 'ghToken'; token: string | null }
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

/**
 * Rust 라우터가 저장소 경로·환경을 읽는 유일한 출처(미러). 설정을 쓰는 쪽은 사이드카(main 의 ConfigStore)뿐이고,
 * Rust 는 renderer 가 보낸 경로를 쓰지 않는다. 토큰 등 비밀은 담지 않는다.
 * kind 는 저장소가 속한 소스의 종류, distro 는 wsl 소스의 배포판 이름, sshHost 는 ssh 소스의 호스트(`~/.ssh/config` 별칭 또는 `[user@]host[:port]`)다.
 * Rust 는 sshHost 를 쓰기 전에 `handlers.ts` 의 SSH 호스트 규칙으로 다시 검사한다(비었거나 255자 초과·공백·NUL·제어 문자·`-` 시작이면 거부).
 */
export interface MirrorRepo {
  id: string
  kind: 'local' | 'wsl' | 'ssh' | 'github'
  path: string
  /** 사용자가 붙인 이름. Rust 는 fetch 확인 창 제목에만 쓴다(경로·kind 판정에는 쓰지 않는다) */
  name: string
  distro?: string
  sshHost?: string
}

/** Node → Rust */
export type FromSidecar =
  | { t: 'res'; id: number; env: IpcEnvelope<unknown> }
  /** 저장소 미러 전체. gen 은 1 부터 1씩 늘고, Rust 는 이전 것보다 작거나 같은 gen 을 버린다. 시작 때 ready 전에, 이후 목록이 바뀔 때마다 보낸다 */
  | { t: 'repos'; gen: number; repos: MirrorRepo[] }
  /**
   * fetch 설정 스냅샷. 미러와 gen 을 따로 센다(1 부터, Rust 는 이미 받은 것 이하를 버린다). 시작 때 ready 전에, 값이 바뀔 때마다 `repos` 다음에 보낸다.
   * autoFetchMinutes 는 기본값(5)이 적용된 값이고 0 이면 끈다. approvals·trusted 는 저장소 id → 지문(sha256 hex)이며 설정에 있는 저장소만 담는다 — 비밀이 아니다
   */
  | { t: 'fetchPrefs'; gen: number; autoFetchMinutes: number; approvals: Record<string, string>; trusted: Record<string, string> }
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
