// IPC 계약. 채널 이름과 인자·결과 타입을 한곳에서 정의해 main/preload/renderer 가 공유한다.
import type {
  AddRepoInput,
  AppConfig,
  CommitActivity,
  FetchStatus,
  GhAuthState,
  GhAvatarQuery,
  GhAvatarResult,
  GhCheckDetail,
  GhComment,
  GhCommitChecksResult,
  GhCommitSummary,
  GhDeviceCode,
  GhIssueDetail,
  GhIssueSummary,
  GhJob,
  GhLabel,
  GhLaunchpadResult,
  GhListQuery,
  GhListResult,
  GhPullDetail,
  GhPullFile,
  GhPullSummary,
  GhRun,
  GhRepoRef,
  GhUser,
  JiraAuthState,
  JiraComment,
  JiraConnectInput,
  JiraIssueDetail,
  JiraSearchQuery,
  JiraSearchResult,
  RemoteDirListing,
  RepoPatch,
  SshHostInfo,
  CommitDetail,
  CompareResult,
  ExternalOpenRequest,
  FileDiff,
  FileDiffRequest,
  FileHistoryPage,
  FileHistoryRequest,
  BlameRequest,
  BlameResult,
  LogPage,
  LogQuery,
  LfsPathsRequest,
  RepoExtras,
  LogScope,
  RefsDetails,
  RefsSnapshot,
  ReleaseNotes,
  Repo,
  SearchResult,
  UiPrefs,
  UpdateDismissAction,
  UpdateState,
  WipDiffRequest,
  WipStatus
} from './types'

export interface IpcContract {
  'config:get': { args: []; result: AppConfig }
  'config:setUi': { args: [patch: Partial<UiPrefs>]; result: AppConfig }
  /** 저장소(또는 그 안의 파일)를 파일 관리자·에디터·터미널로 연다. 경로·실행 파일은 main 이 저장소 설정과 사용자가 고른 에디터 설정으로만 정한다 */
  'external:open': { args: [repoId: string, req: ExternalOpenRequest]; result: void }
  /** 에디터 실행 파일 선택 창(JetBrains·사용자 지정). 고르면 저장하고 설정을 돌려준다, 취소하면 null */
  'external:pickEditorExe': { args: []; result: AppConfig | null }
  /** 폴더 선택 대화상자. 취소하면 null. defaultPath 로 시작 위치(\\wsl.localhost\… 등)를 줄 수 있다 */
  'dialog:pickFolder': { args: [defaultPath?: string]; result: string | null }
  'repos:remove': { args: [repoId: string]; result: AppConfig }
  /** 환경(로컬·WSL·SSH)과 경로로 저장소를 추가한다. 그 환경에서 git 최상위 경로를 확인한 뒤 저장한다 */
  'repos:add': { args: [input: AddRepoInput]; result: Repo }
  /** 이름·그룹 변경 */
  'repos:update': { args: [repoId: string, patch: RepoPatch]; result: AppConfig }
  /** 변경 감시 대상(활성 저장소). null 이면 감시 중단. 바뀌면 'repo:changed' 이벤트 */
  'repos:watch': { args: [repoId: string | null]; result: void }
  /** 설치된 WSL 배포판(docker-desktop 류 제외). Windows 가 아니면 빈 배열 */
  'env:wslDistros': { args: []; result: string[] }
  /** ~/.ssh/config 의 Host 목록(와일드카드 패턴 제외) */
  'env:sshHosts': { args: []; result: SshHostInfo[] }
  /** 연결 테스트: 그 환경에서 git 이 실행되는지 확인하고 git 버전 문자열을 돌려준다 */
  'env:test': { args: [input: Omit<AddRepoInput, 'path' | 'group'>]; result: string }
  /** SSH 원격 폴더 목록(저장소 추가 창의 폴더 탐색). path 가 비었거나 '~' 면 홈 */
  'env:listDir': { args: [input: { sshHost: string; path: string }]; result: RemoteDirListing }

  // ── GitHub (API 는 환경과 무관하게 main 에서 직접 호출, 토큰은 main 에만 있다) ──
  /** 저장소 remote 에서 GitHub owner/repo 추론. GitHub 저장소가 아니면 null */
  'gh:repo': { args: [repoId: string]; result: GhRepoRef | null }
  'gh:auth': { args: []; result: GhAuthState }
  /** PAT 로 로그인(검증 후 safeStorage 로 암호화 저장) */
  'gh:loginPat': { args: [token: string]; result: GhAuthState }
  /** GitHub CLI(gh)에 로그인된 토큰을 가져와 로그인(토큰은 main 안에서만 쓰고 돌려주지 않는다) */
  'gh:importCliToken': { args: []; result: GhAuthState }
  /** Device Flow 시작. 설정의 githubClientId 가 필요하다 */
  'gh:deviceStart': { args: []; result: GhDeviceCode }
  /** Device Flow 완료까지 폴링(사용자가 브라우저에서 승인). 취소는 req:cancel */
  'gh:devicePoll': { args: [requestId: string]; result: GhAuthState }
  'gh:logout': { args: []; result: GhAuthState }
  'gh:pulls': { args: [repoId: string, query: GhListQuery]; result: GhListResult<GhPullSummary> }
  'gh:pull': { args: [repoId: string, number: number]; result: GhPullDetail }
  'gh:pullFiles': { args: [repoId: string, number: number]; result: GhPullFile[] }
  'gh:pullCommits': { args: [repoId: string, number: number]; result: GhCommitSummary[] }
  'gh:issues': { args: [repoId: string, query: GhListQuery]; result: GhListResult<GhIssueSummary> }
  'gh:issue': { args: [repoId: string, number: number]; result: GhIssueDetail }
  /** PR·이슈 댓글(이슈 댓글 API — PR 대화도 여기에 있다) */
  'gh:comments': { args: [repoId: string, number: number]; result: GhComment[] }
  /** 필터용 라벨·담당 가능 사용자 */
  'gh:filters': { args: [repoId: string]; result: { labels: GhLabel[]; assignees: GhUser[] } }
  /** GitHub 웹 페이지를 기본 브라우저로(https://github.com/ 만 허용) */
  /** 작성자 아바타 해석(커밋 sha → GitHub 계정). 이메일은 GitHub 로 보내지 않는다. requestId 로 취소 */
  'gh:avatars': { args: [repoId: string, requestId: string, authors: GhAvatarQuery[]]; result: GhAvatarResult }
  /** 커밋 목록의 체크 요약(화면에 보이는 sha 만, 최대 50개). 로그인하지 않았으면 available=false */
  'gh:commitChecks': { args: [repoId: string, shas: string[]]; result: GhCommitChecksResult }
  /** 한 커밋의 체크 목록(이름·상태·소요 시간) */
  'gh:commitCheckRuns': { args: [repoId: string, sha: string]; result: GhCheckDetail[] }
  /** 최근 워크플로 실행 목록(읽기 전용) */
  'gh:runs': { args: [repoId: string, page: number]; result: GhListResult<GhRun> }
  /** 워크플로 실행 한 건의 잡 목록 */
  'gh:runJobs': { args: [repoId: string, runId: number]; result: GhJob[] }
  'gh:openExternal': { args: [url: string]; result: void }
  /**
   * Launchpad: 내 PR·리뷰 요청·할당 이슈를 사용자 기준 검색으로 가져와 등록된 저장소의 GitHub remote 와 맞춘다(main 이 저장소 목록을 안다).
   * 로그인하지 않았으면 needsLogin. fresh 면 저장소 remote 캐시도 새로 읽는다. requestId 로 취소
   */
  'gh:launchpad': { args: [requestId: string, fresh?: boolean]; result: GhLaunchpadResult }

  // ── Jira (읽기 전용, 자격 증명은 main 에만 있다) ──
  'jira:auth': { args: []; result: JiraAuthState }
  /** 자격 증명을 확인(/myself)한 뒤 safeStorage 로 암호화 저장 */
  'jira:connect': { args: [input: JiraConnectInput]; result: JiraAuthState }
  'jira:disconnect': { args: []; result: JiraAuthState }
  'jira:issue': { args: [key: string, requestId?: string]; result: JiraIssueDetail }
  /** 최근 댓글 몇 개(최신순) */
  'jira:comments': { args: [key: string, requestId?: string]; result: JiraComment[] }
  'jira:search': { args: [query: JiraSearchQuery, requestId?: string]; result: JiraSearchResult }
  /** Jira 사이트 호스트의 https 주소만 기본 브라우저로 연다 */
  'jira:openExternal': { args: [url: string]; result: void }
  /** 사이드바용 ref 스냅샷(HEAD·브랜치·리모트·태그·워크트리·스태시) */
  'git:refs': { args: [repoId: string]; result: RefsSnapshot }
  /** git:refs 에서 미룬 부가 정보(업스트림 ahead/behind·서브모듈). 첫 로그 페이지가 나간 뒤에 조회한다 */
  'git:refsDetails': { args: [repoId: string]; result: RefsDetails }
  'git:log': { args: [repoId: string, query: LogQuery, requestId?: string]; result: LogPage }
  /** 메시지·작성자·해시 검색. scope 를 주면 그 보기 범위(Solo/Hide)의 이력에서만 찾는다 */
  'git:search': { args: [repoId: string, query: string, scope?: LogScope, requestId?: string]; result: SearchResult }
  /** 커밋 활동 미니맵용 시간 구간별 커밋 수. scope 를 주면 그 보기 범위(그래프와 같은 범위) */
  'git:activity': { args: [repoId: string, scope?: LogScope, requestId?: string]; result: CommitActivity }
  /** 커밋 메타데이터 + 첫 부모 대비 변경 파일 목록(+/- 통계). 커밋 해시 기준 불변 캐시 */
  'git:commit': { args: [repoId: string, hash: string, requestId?: string]; result: CommitDetail }
  /** 두 커밋(해시) 사이의 변경 파일 목록(+/- 통계). from → to 방향, 읽기 전용. 해시 쌍 기준 불변 캐시 */
  'git:compare': { args: [repoId: string, from: string, to: string, requestId?: string]; result: CompareResult }
  /** 파일 하나의 diff. 크기 상한을 넘으면 앞부분만 담고 truncated=true(req.full 로 전체) */
  'git:fileDiff': { args: [repoId: string, req: FileDiffRequest, requestId?: string]; result: FileDiff }
  /** 파일 하나를 바꾼 커밋 목록(`git log --follow`, 이름 변경 추적). skip/count 로 페이지 단위 로딩 */
  'git:fileHistory': { args: [repoId: string, req: FileHistoryRequest, requestId?: string]; result: FileHistoryPage }
  /** 지정 커밋(생략하면 HEAD) 시점 파일의 줄별 blame. 읽기 전용 */
  'git:blame': { args: [repoId: string, req: BlameRequest, requestId?: string]; result: BlameResult }
  /** 저장소 부가 정보(읽기 전용): LFS 사용 여부(.gitattributes 의 filter=lfs)와 git config 의 gitflow.* 설정 */
  'git:repoExtras': { args: [repoId: string]; result: RepoExtras }
  /** 경로 중 filter=lfs 속성인 것(git check-attr). req.rev 가 있으면 그 커밋 기준 */
  'git:lfsPaths': { args: [repoId: string, req: LfsPathsRequest, requestId?: string]; result: string[] }
  /** 작업 트리 상태(WIP): 스테이지됨·변경됨·추적 안 됨 파일. 읽기 전용이며 캐시하지 않는다 */
  'git:wipStatus': { args: [repoId: string, requestId?: string]; result: WipStatus }
  /** 작업 트리 파일 하나의 diff(스테이지/작업 트리/추적 안 됨). 결과 형식은 git:fileDiff 와 같다 */
  'git:wipDiff': { args: [repoId: string, req: WipDiffRequest, requestId?: string]; result: FileDiff }
  /**
   * 원격 fetch(이 앱의 유일한 쓰기). manual 이면 사용자가 새로고침을 누른 것 — 저장소 설정이 위험하면 main 이 확인 창을 띄운다.
   * 끝나면(또는 막히면) 상태를 돌려준다. 같은 저장소의 진행 중 fetch 가 있으면 그 결과를 공유한다
   */
  'git:fetch': { args: [repoId: string, manual: boolean]; result: FetchStatus }
  /** 진행 중인 요청 취소 */
  'req:cancel': { args: [requestId: string]; result: void }
  /** 앱 업데이트. 상태 변화는 'update:state' 이벤트로도 온다 */
  'update:get': { args: []; result: UpdateState }
  /** 새 버전 확인(수동 확인 — 나중에·건너뛰기 설정과 무관하게 결과를 돌려준다) */
  'update:check': { args: []; result: UpdateState }
  /** 알려진 새 버전 내려받기 시작(진행은 update:state 이벤트). 설치 방식은 OS·설치 형태에 따라 다르다 */
  'update:download': { args: []; result: UpdateState }
  /** 내려받기 취소 */
  'update:cancel': { args: []; result: void }
  /** 내려받은 업데이트 설치(auto: 앱 재시작 후 설치, manual: 설치 파일을 다시 열기) */
  'update:install': { args: []; result: void }
  'update:dismiss': { args: [action: UpdateDismissAction, version: string]; result: UpdateState }
  /** 현재 버전의 릴리스 노트(그 버전 릴리스가 없으면 최신 릴리스) */
  'update:releaseNotes': { args: []; result: ReleaseNotes }
  /** 제목 표시줄 오버레이(Windows·Linux 창 컨트롤) 색을 현재 테마에 맞춘다. macOS 는 no-op */
  /** dim: 모달 배경막이 떠 있음(창 컨트롤도 같은 정도로 어둡게) */
  'win:setTitleBarTheme': { args: [theme: { dark: boolean; dim?: boolean }]; result: void }
}

export type IpcChannel = keyof IpcContract
export type IpcArgs<C extends IpcChannel> = IpcContract[C]['args']
export type IpcResult<C extends IpcChannel> = IpcContract[C]['result']

/** main 이 renderer 로 밀어주는 이벤트 */
export interface IpcEvents {
  'repo:changed': { repoId: string }
  'fetch:status': FetchStatus
  'theme:system': { dark: boolean }
  'update:state': UpdateState
}
export type IpcEvent = keyof IpcEvents

/** preload 에서 허용 목록으로 쓰는 런타임 채널 목록 */
export const IPC_CHANNELS = [
  'config:get',
  'config:setUi',
  'dialog:pickFolder',
  'external:open',
  'external:pickEditorExe',
  'repos:remove',
  'repos:add',
  'repos:update',
  'repos:watch',
  'env:wslDistros',
  'env:sshHosts',
  'env:test',
  'env:listDir',
  'gh:repo',
  'gh:auth',
  'gh:loginPat',
  'gh:importCliToken',
  'gh:deviceStart',
  'gh:devicePoll',
  'gh:logout',
  'gh:pulls',
  'gh:pull',
  'gh:pullFiles',
  'gh:pullCommits',
  'gh:issues',
  'gh:issue',
  'gh:comments',
  'gh:filters',
  'gh:avatars',
  'gh:commitChecks',
  'gh:commitCheckRuns',
  'gh:runs',
  'gh:runJobs',
  'gh:openExternal',
  'gh:launchpad',
  'jira:auth',
  'jira:connect',
  'jira:disconnect',
  'jira:issue',
  'jira:comments',
  'jira:search',
  'jira:openExternal',
  'git:refs',
  'git:refsDetails',
  'git:search',
  'git:log',
  'git:activity',
  'git:commit',
  'git:compare',
  'git:fileDiff',
  'git:fileHistory',
  'git:blame',
  'git:repoExtras',
  'git:lfsPaths',
  'git:wipStatus',
  'git:wipDiff',
  'git:fetch',
  'req:cancel',
  'update:get',
  'update:check',
  'update:download',
  'update:cancel',
  'update:install',
  'update:dismiss',
  'update:releaseNotes',
  'win:setTitleBarTheme'
] as const satisfies readonly IpcChannel[]

export const IPC_EVENTS = ['repo:changed', 'fetch:status', 'theme:system', 'update:state'] as const satisfies readonly IpcEvent[]

/** invoke 결과 래퍼. Electron 기본 에러 문자열("Error invoking remote method…")을 피하려고 쓴다. */
export type IpcEnvelope<T> = { ok: true; data: T } | { ok: false; error: string; code?: string }

/** window.antler 로 노출되는 API 형태 */
export interface AntlerApi {
  platform: 'win32' | 'darwin' | 'linux' | string
  invoke<C extends IpcChannel>(channel: C, ...args: IpcArgs<C>): Promise<IpcResult<C>>
  on<E extends IpcEvent>(event: E, listener: (payload: IpcEvents[E]) => void): () => void
}
