// Rust(src-tauri/src/sidecar.rs) ↔ Node 사이드카의 stdio JSON-lines 프로토콜. 줄 하나가 메시지 하나다.
// Rust 는 채널을 해석하지 않고 req 를 그대로 중계한다.
import type { IpcEnvelope } from '@shared/ipc'

/**
 * 인자 배열의 빈 자리(undefined) 표식. JSON 은 배열 속 undefined 를 null 로 바꾸므로 bridge 가 이 키 하나짜리 객체로 보내고
 * 사이드카가 undefined 로 되돌린다(src/renderer/tauri/bridge.ts 의 encodeArgs, src/sidecar/index.ts 의 decodeArgs).
 */
export const UNDEFINED_ARG = '__antlerUndefinedArg'

/**
 * Rust → Node. Rust 가 설정을 소유하는 실행(Phase 6b PR 10)에서는 `req`(update:* 만)와 `hostres` 뿐이다 — `focus` 는 사이드카가 감시·fetch 를 맡는 되돌리기(ANTLER_RUST_GIT=0)에서만 보낸다.
 * (`note`·`fetchApprove`·`githubBranch`·`repoq`·`repoqCancel`·`ghToken` 은 PR 10 에서 없앴다 — Rust 가 그 일을 직접 한다)
 */
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

/**
 * Rust 라우터가 저장소 경로·환경을 읽는 출처(미러). Rust 가 설정을 소유하면 `config::sync` 가 채우고, 소유하지 않는 되돌리기(ANTLER_RUST_GIT=0)에서는 사이드카가 이 모양으로 보낸다.
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
  /**
   * github 소스(GitHub 원격 전용 저장소)의 owner/repo 링크. Rust 가 `gh:repo` 를 git 없이 이 값으로 답한다(RepoService.ghRepo 와 같다).
   * 경로는 항상 앱 캐시 경로이고, Rust 는 캐시 모양(`<기준>/<owner>/<repo>.git`)을 통과한 경로에서만 토큰·lazy fetch 환경을 연다.
   * defaultBranch 는 레코드의 기본 브랜치(알면)다 — Rust fetch 가 HEAD 를 맞출 때 첫 후보로 쓴다
   */
  github?: { owner: string; repo: string; defaultBranch?: string }
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
