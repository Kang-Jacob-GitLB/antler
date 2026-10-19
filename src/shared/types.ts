// main ↔ renderer 공용 데이터 타입. 런타임 코드 없이 타입만 둔다.

export type SourceKind = 'local' | 'wsl' | 'ssh'

/** 저장소가 존재하는 실행 환경. git 은 항상 이 환경 안에서 실행된다. */
export interface Source {
  id: string
  kind: SourceKind
  label: string
  /** kind === 'wsl' 일 때 배포판 이름 */
  distro?: string
  /** kind === 'ssh' 일 때 ~/.ssh/config 의 Host 별칭 */
  sshHost?: string
}

/** 저장소. path 는 환경의 원래 형식 그대로 저장한다(Windows 경로 / Linux 경로). */
export interface Repo {
  id: string
  sourceId: string
  path: string
  name: string
  group?: string
}

export type ThemePref = 'system' | 'dark' | 'light'
export type Locale = 'ko' | 'en'

export interface UiPrefs {
  theme: ThemePref
  locale: Locale
  /** 3분할 패널 폭(px): [사이드바, 상세] */
  paneWidths: [number, number]
  /** 접힌 사이드바 섹션·폴더 키 */
  collapsed?: string[]
  /** diff 보기 방식 */
  diffMode?: 'unified' | 'split'
  /** GitHub OAuth App client ID(Device Flow 로그인용). 비우면 PAT 로만 로그인 */
  githubClientId?: string
  lastRepoId?: string
  /** 왼쪽 사이드바·오른쪽 상세 패널을 숨겼는지 */
  sidebarHidden?: boolean
  detailHidden?: boolean
  /** 저장소 그룹 이름 → 칩 색(hue). 그룹이 처음 생길 때 겹치지 않는 색을 골라 저장한다 */
  groupColors?: Record<string, number>
  /** 상세 패널에서 커밋 내용(머리 부분)의 높이(px). 없으면 내용에 맞추되 패널의 46% 까지 */
  detailMetaHeight?: number
  /** 커밋 활동 미니맵을 접었는지 */
  minimapCollapsed?: boolean
  /** 그래프 노드에 작성자 이니셜을 그릴지(없으면 켜짐) */
  graphInitials?: boolean
  /** 사용자가 '이 버전 건너뛰기'를 고른 업데이트 버전(그 버전은 자동 알림에서 제외, 더 새 버전은 알림) */
  skippedUpdateVersion?: string
  /** 활성 저장소를 자동으로 fetch 하는 주기(분). 0 이면 끔, 없으면 5분 */
  autoFetchMinutes?: number
}

/** 저장소 fetch 상태(main 이 관리하고 renderer 로 밀어준다) */
export interface FetchStatus {
  repoId: string
  fetching: boolean
  /** 마지막으로 성공한 시각(epoch ms) */
  lastOkAt: number | null
  /** 마지막 시도의 오류 메시지 */
  error: string | null
  /** 저장소 설정 때문에 fetch 를 막았다면 그 항목들. confirmable 이면 수동 새로고침에서 사용자가 허용할 수 있다 */
  blocked: { keys: string[]; confirmable: boolean } | null
  /** 인증 실패로 자동 fetch 를 멈춘 상태(수동 새로고침으로 재개) */
  authPaused: boolean
  /** 자동 fetch 가 인증에 실패했는데 이 remote 는 아직 수동 fetch 로 승인되지 않았다 — 새로고침을 한 번 누르면 이후 자동으로 가져온다 */
  needsManual: boolean
  /** remote 가 하나도 없어 fetch 대상이 아님 */
  noRemote: boolean
}

/**
 * 앱 업데이트 상태(main → renderer).
 * mode: auto = 앱 안에서 내려받아 설치, manual-dmg = macOS 설치 이미지(dmg)를 열어 줌, manual-file = portable·deb 파일을 열어/보여 줌
 */
export type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'downloaded' | 'error' | 'not-available'
export type UpdateMode = 'auto' | 'manual-dmg' | 'manual-file'
export interface UpdateState {
  status: UpdateStatus
  mode: UpdateMode
  currentVersion: string
  /** 새 버전(available·downloading·downloaded) */
  version?: string
  /** 릴리스 노트 페이지(https://github.com/… 만) */
  notesUrl?: string
  /** downloading 중 0~100 */
  progress?: number
  /** error 메시지(토큰·경로 등 민감 정보 없음) */
  error?: string
  /** manual 모드에서 열어 준 파일 이름 */
  fileName?: string
}
/** later = 앱을 다시 시작할 때까지 이 버전 알림 숨김, skip = 이 버전 건너뛰기(설정에 저장) */
export type UpdateDismissAction = 'later' | 'skip'

/** 커밋 활동 미니맵 데이터: 시간 구간별 커밋 수 */
export interface CommitActivity {
  /** 시간 오름차순, 구간 시작 시각(epoch 초)과 개수. 빈 구간도 0 으로 채운다 */
  buckets: { t: number; count: number }[]
  /** 가장 오래된·가장 최근 커밋 시각(epoch 초) */
  first: number
  last: number
  /** 보기 범위의 전체 커밋 수 */
  total: number
  /** 구간 길이(초) */
  bucketSec: number
}

export interface AppConfig {
  version: 1
  sources: Source[]
  repos: Repo[]
  ui: UiPrefs
  /**
   * main 전용: 사용자가 수동 fetch 로 한 번 성공시킨 저장소별 remote(이름+실제 URL) 지문. 이 지문이 같을 때만
   * 자동 fetch 가 자격 증명 도우미를 쓴다. renderer 는 바꿀 수 없다(config:setUi·repos:update 는 이 키를 받지 않는다)
   */
  fetchApprovals?: Record<string, string>
}

export type RefKind = 'head' | 'local' | 'remote' | 'tag'

/**
 * 커밋에 붙은 ref 라벨. name 은 짧은 이름(main, origin/main, v1.0).
 * kind === 'head' 의 name 은 HEAD 가 가리키는 브랜치 이름이고, 분리된 HEAD 면 'HEAD' 다.
 */
export interface RefLabel {
  kind: RefKind
  name: string
}

export interface Commit {
  hash: string
  parents: string[]
  authorName: string
  authorEmail: string
  /** epoch seconds */
  authorDate: number
  /** epoch seconds */
  commitDate: number
  subject: string
  refs: RefLabel[]
}

export interface LogQuery {
  skip: number
  count: number
  /** 특정 ref 만 보기. 없으면 모든 브랜치·리모트·태그 + HEAD */
  ref?: string
}

export interface LogPage {
  commits: Commit[]
  /** 다음 페이지가 존재할 수 있는지 */
  hasMore: boolean
}

export interface HeadInfo {
  /** 현재 브랜치. detached 면 null */
  branch: string | null
  hash: string | null
}

export interface Branch {
  /** 짧은 이름. 로컬: feature/x, 리모트: origin/feature/x */
  name: string
  /** refs/heads/feature/x, refs/remotes/origin/feature/x */
  fullName: string
  hash: string
  /** 로컬 브랜치의 업스트림 짧은 이름(origin/feature/x) */
  upstream?: string
  ahead?: number
  behind?: number
  /** 업스트림이 설정됐지만 원격에서 사라짐 */
  upstreamGone?: boolean
  isHead: boolean
}

export interface RemoteGroup {
  /** 리모트 이름(origin) */
  name: string
  /** 이 리모트의 브랜치들. name 은 'origin/feature/x' 형식, origin/HEAD 는 제외 */
  branches: Branch[]
}

export interface Tag {
  name: string
  fullName: string
  /** 태그가 가리키는 커밋(annotated 태그는 peel 한 커밋 해시) */
  hash: string
  annotated: boolean
  /** 최종 대상이 커밋인지(트리·블롭·중첩 태그면 false — 그래프에서 찾아갈 수 없다) */
  targetsCommit: boolean
}

export interface Worktree {
  path: string
  /** HEAD 커밋. 아직 커밋이 없으면 null */
  head: string | null
  /** 체크아웃된 브랜치 짧은 이름. detached 면 null */
  branch: string | null
  detached: boolean
  bare: boolean
  locked: boolean
  prunable: boolean
  /** 목록 첫 항목(주 작업 트리) */
  isMain: boolean
}

export interface Stash {
  index: number
  /** stash@{0} */
  ref: string
  hash: string
  message: string
  /** epoch seconds */
  date: number
}

/**
 * 첫 화면 뒤에 따로 받는 ref 부가 정보(비싸서 RefsSnapshot 에서 미룬다).
 * tracking: 로컬 브랜치 전체 이름 → 업스트림 ahead/behind/gone(변화 없는 브랜치는 없음)
 */
export interface RefsDetails {
  tracking: Record<string, { ahead?: number; behind?: number; upstreamGone?: boolean }>
  submodules: SubmoduleInfo[]
}

/** 사이드바용 ref 스냅샷 */
export interface RefsSnapshot {
  head: HeadInfo
  local: Branch[]
  remotes: RemoteGroup[]
  tags: Tag[]
  worktrees: Worktree[]
  stashes: Stash[]
  /** 최상위 서브모듈(중첩은 보지 않는다). 없으면 빈 배열. git:refs 는 비워 두고 git:refsDetails 가 채운다 */
  submodules: SubmoduleInfo[]
  /** 저장소의 기본 브랜치와 그 tip(그래프에서 굵게 그릴 줄기). 정할 수 없으면 없음 */
  defaultBranch?: DefaultBranch
}

/** uninitialized: 작업 트리에 체크아웃되지 않음 · clean: HEAD 가 기록된 커밋과 같음 · modified: 다른 커밋 · unknown: 상한 초과 등으로 확인하지 않음 */
export type SubmoduleStatus = 'uninitialized' | 'clean' | 'modified' | 'unknown'

export interface SubmoduleInfo {
  /** .gitmodules 의 이름(없으면 경로) */
  name: string
  /** 상위 저장소 기준 상대 경로(POSIX 구분자) */
  path: string
  /** .gitmodules 의 URL — 표시 전용 텍스트 */
  url?: string
  branch?: string
  /** 상위 저장소가 기록한 커밋 */
  recorded: string
  /** 체크아웃된 HEAD(초기화되지 않았거나 알 수 없으면 없음) */
  head?: string
  status: SubmoduleStatus
}

export interface DefaultBranch {
  /** 짧은 이름(origin/main, main, master) */
  name: string
  hash: string
}

export interface SearchResult {
  /** 그래프와 같은 순서(--date-order)의 일치 커밋 해시 */
  hashes: string[]
  /** 상한에 걸려 잘렸는지 */
  truncated: boolean
}

// ── M5: GitHub ───────────────────────────────────────

/** 저장소 remote URL 에서 추론한 GitHub 저장소 */
export interface GhRepoRef {
  owner: string
  repo: string
  /** 추론에 쓴 remote 이름(origin 우선) */
  remote: string
}

export interface GhUser {
  login: string
  avatarUrl: string
}

/** 작성자 아바타 조회 요청 한 건: 이메일은 GitHub 로 보내지 않고, 그 작성자의 커밋 sha 로만 조회한다 */
export interface GhAvatarQuery {
  email: string
  sha: string
}

/** 해석된 작성자. url 은 크기(`s=`)를 붙이기 전의 avatars.githubusercontent.com 주소 */
export interface GhAuthorAvatar {
  login: string
  url: string
}

export interface GhAvatarResult {
  /** 이 저장소에 GitHub remote 가 있는지(없으면 renderer 가 더 묻지 않는다) */
  github: boolean
  /** 이메일(요청에 쓴 문자열) → 해석 결과. null=GitHub 계정과 연결되지 않음. 빠진 키=지금은 못 정함(한도·네트워크·미푸시), 나중에 다시 */
  avatars: Record<string, GhAuthorAvatar | null>
  /** 지금 사진 조회가 막혀 있는 이유(없으면 막히지 않음). 화면은 작성자 열 머리에 안내로만 드러낸다 */
  blocked?: GhAvatarBlocked
}

/** rate-limited: 비로그인/로그인 API 한도 소진(resetAt 이후 재시도), needs-login: 비공개 저장소라 비로그인 조회가 404, session-cap: 이번 실행의 조회 상한 */
export interface GhAvatarBlocked {
  reason: 'rate-limited' | 'needs-login' | 'session-cap'
  /** epoch seconds (rate-limited 일 때) */
  resetAt?: number
  /** 이 상태가 로그인으로 풀리는지 */
  signedIn: boolean
}

export interface GhLabel {
  name: string
  /** 6자리 hex(# 없음) — GitHub 가 정한 라벨 색. 화면에서는 토큰과 섞어 은은하게 쓴다 */
  color: string
}

/** 로그인 상태. 토큰 값은 renderer 로 절대 넘기지 않는다 */
export interface GhAuthState {
  signedIn: boolean
  user?: GhUser
  /** 'pat' | 'device' | 'gh-cli' */
  method?: 'pat' | 'device' | 'gh-cli'
  /** GitHub CLI(gh)가 설치되어 있어 로그인 가져오기를 쓸 수 있는지(아직 모르면 생략) */
  cliAvailable?: boolean
  /** 남은 API 호출 수(마지막 응답 기준) */
  rateRemaining?: number
  rateLimit?: number
  /** epoch seconds */
  rateReset?: number
  /** safeStorage 를 쓸 수 없어 토큰을 저장하지 않는 환경(Linux 키링 없음 등) */
  storageUnavailable?: boolean
}

/** Device Flow 시작 결과 — 사용자가 브라우저에서 user_code 를 입력한다 */
export interface GhDeviceCode {
  userCode: string
  verificationUri: string
  /** 만료까지 초 */
  expiresIn: number
  /** 폴링 간격(초) */
  interval: number
}

export type GhCheckState = 'success' | 'failure' | 'pending' | 'neutral' | 'none'
export type GhReviewState = 'approved' | 'changes_requested' | 'commented' | 'pending' | 'dismissed' | 'none'

export interface GhPullSummary {
  number: number
  title: string
  state: 'open' | 'closed' | 'merged'
  draft: boolean
  author: GhUser
  labels: GhLabel[]
  assignees: GhUser[]
  headRef: string
  baseRef: string
  headSha: string
  /** epoch seconds */
  updatedAt: number
  createdAt: number
  comments: number
  /** 목록 보강(로그인했을 때만). 없으면 아직 모름 */
  review?: GhReviewState
  checks?: GhCheckState
  htmlUrl: string
}

export interface GhCheckRun {
  name: string
  state: GhCheckState
  /** 상세 링크 */
  url?: string
}

export interface GhReview {
  author: GhUser
  state: GhReviewState
  /** epoch seconds */
  submittedAt: number
}

export interface GhComment {
  author: GhUser
  /** GitHub 가 렌더링·정화한 HTML(renderer 에서 한 번 더 정화한다) */
  bodyHtml: string
  createdAt: number
  htmlUrl: string
}

export interface GhCommitSummary {
  sha: string
  message: string
  authorName: string
  author?: GhUser
  date: number
}

export interface GhPullFile {
  path: string
  oldPath?: string
  status: 'added' | 'removed' | 'modified' | 'renamed' | 'copied' | 'changed' | 'unchanged'
  additions: number
  deletions: number
  /** GitHub 가 준 unified diff 조각. 큰 파일·바이너리는 없다 */
  patch?: string
}

export interface GhPullDetail extends GhPullSummary {
  bodyHtml: string
  mergeable: boolean | null
  additions: number
  deletions: number
  changedFiles: number
  reviews: GhReview[]
  checkRuns: GhCheckRun[]
  requestedReviewers: GhUser[]
}

export interface GhIssueSummary {
  number: number
  title: string
  state: 'open' | 'closed'
  author: GhUser
  labels: GhLabel[]
  assignees: GhUser[]
  comments: number
  updatedAt: number
  createdAt: number
  htmlUrl: string
}

export interface GhIssueDetail extends GhIssueSummary {
  bodyHtml: string
  closedAt?: number
}

export interface GhListQuery {
  state: 'open' | 'closed' | 'all'
  /** 라벨 이름(하나) */
  label?: string
  /** 담당자 login, 'none' 은 담당자 없음 */
  assignee?: string
  page?: number
}

export interface GhListResult<T> {
  items: T[]
  hasMore: boolean
  /**
   * 필터링으로 빈 쪽을 건너뛰며 여러 쪽을 읽었을 때만 있다. 있으면 "더 보기"에서 이 쪽을 요청하고,
   * 없으면 요청한 page + 1 을 요청한다
   */
  nextPage?: number
}

// ── M4: 원격 환경 ─────────────────────────────────────

/** 저장소 추가 입력. path 는 그 환경의 원래 경로 형식(WSL·SSH 는 /home/…) */
export interface AddRepoInput {
  kind: SourceKind
  /** kind === 'wsl' */
  distro?: string
  /** kind === 'ssh' — ~/.ssh/config 의 Host 별칭 또는 user@host[:port] */
  sshHost?: string
  path: string
  group?: string
}

/** 원격 폴더 탐색 결과(SSH 저장소 추가 창). path 는 원격의 절대 경로 */
export interface RemoteDirListing {
  path: string
  /** 루트(/)면 null */
  parent: string | null
  /** 이 폴더 자체가 git 작업 트리인가(.git 이 있음) */
  isRepo: boolean
  /** 숨김 폴더를 뺀 하위 폴더. isRepo 는 앞쪽 일부만 확인한다(undefined = 확인 안 함) */
  dirs: { name: string; isRepo?: boolean }[]
  /** 항목이 너무 많아 일부만 실었다 */
  truncated: boolean
}

/** ~/.ssh/config 에서 읽은 접속 대상(비밀번호는 다루지 않는다 — 키/agent 인증만) */
export interface SshHostInfo {
  alias: string
  hostName: string
  user?: string
  port: number
  proxyJump?: string
}

export interface RepoPatch {
  /** 빈 문자열 또는 null 이면 그룹 해제 */
  group?: string | null
  name?: string
}

// ── M3: 커밋 상세·diff ─────────────────────────────────

/** A 추가 · M 수정 · D 삭제 · R 이름 변경 · C 복사 · T 타입 변경(파일↔심볼릭 링크 등) · U 병합 충돌 */
export type FileStatus = 'A' | 'M' | 'D' | 'R' | 'C' | 'T' | 'U'

export interface FileChange {
  status: FileStatus
  /** 변경 후 경로(삭제면 삭제된 경로) */
  path: string
  /** 이름 변경·복사의 원래 경로 */
  oldPath?: string
  /** 이름 변경·복사 유사도(0~100) */
  similarity?: number
  /** 바이너리면 null */
  additions: number | null
  deletions: number | null
  binary: boolean
}

export interface CommitDetail {
  hash: string
  parents: string[]
  authorName: string
  authorEmail: string
  /** epoch seconds */
  authorDate: number
  committerName: string
  committerEmail: string
  /** epoch seconds */
  commitDate: number
  subject: string
  /** 제목 줄을 뺀 본문(앞뒤 공백 제거) */
  body: string
  /** 비교 기준 부모(merge 는 첫 부모, 루트 커밋은 null) */
  baseParent: string | null
  files: FileChange[]
  additions: number
  deletions: number
  /** 파일 수 상한에 걸려 목록이 잘렸는지 */
  truncated: boolean
}

/** 두 커밋 비교 결과: from → to 로 바뀐 파일 목록(+/- 통계). 파일 diff 는 FileDiffRequest 의 hash=to, parent=from 으로 연다 */
export interface CompareResult {
  from: string
  to: string
  files: FileChange[]
  additions: number
  deletions: number
  /** 파일 수 상한에 걸려 목록이 잘렸는지 */
  truncated: boolean
}

export interface FileDiffRequest {
  hash: string
  path: string
  oldPath?: string
  /** 비교 기준 부모. 생략하면 첫 부모(루트 커밋이면 빈 트리) */
  parent?: string
  /** true 면 크기 상한을 무시하고 전체를 불러온다(지연 로딩의 "전체 보기") */
  full?: boolean
}

/** 파일 히스토리 요청(`git log --follow` 상당). rev 를 생략하면 HEAD 부터 거슬러 올라간다 */
export interface FileHistoryRequest {
  path: string
  /** 이 커밋부터 거슬러 올라간다(커밋 해시만). 생략하면 HEAD */
  rev?: string
  skip: number
  count: number
}

/** 파일을 바꾼 커밋 하나. path/oldPath 는 그 커밋에서의 이름(이름 변경을 따라간 결과) */
export interface FileHistoryEntry {
  hash: string
  parents: string[]
  authorName: string
  authorEmail: string
  /** epoch seconds */
  authorDate: number
  subject: string
  /** name-status 첫 글자: A 추가 · M 수정 · D 삭제 · R 이름 변경 · C 복사 · T 타입 변경 */
  status: string
  path: string
  oldPath?: string
}

export interface FileHistoryPage {
  entries: FileHistoryEntry[]
  hasMore: boolean
  /** 이 페이지를 읽은 시작 커밋(요청에 rev 가 없었다면 해석한 HEAD). 이후 페이지는 이 해시로 요청한다 */
  rev?: string
}

/** Blame 요청. rev 를 생략하면 HEAD 시점의 파일 */
export interface BlameRequest {
  path: string
  rev?: string
}

export interface BlameCommit {
  hash: string
  authorName: string
  authorEmail: string
  /** epoch seconds */
  authorDate: number
  summary: string
  /** 이력의 끝(루트)이라 더 거슬러 갈 수 없는 커밋 */
  boundary: boolean
}

/** 같은 커밋이 연속으로 만든 줄 묶음. start 는 lines 의 0 기반 위치 */
export interface BlameGroup {
  hash: string
  start: number
  count: number
}

export interface BlameResult {
  /** 실제로 blame 한 커밋(요청에 rev 가 없었다면 해석한 HEAD) */
  rev: string
  path: string
  /** 바이너리라 줄 단위로 보여 주지 않는다 */
  binary: boolean
  /** 크기 상한을 넘어 blame 하지 않았다 */
  tooLarge: boolean
  /** 그 시점에 경로가 없거나 파일이 아니라(서브모듈) 볼 내용이 없다 */
  missing: boolean
  /** 그 시점 파일 크기(바이트) */
  size: number
  commits: Record<string, BlameCommit>
  groups: BlameGroup[]
  lines: string[]
}

/** diff 한 줄: ' ' 문맥 · '+' 추가 · '-' 삭제 · '\\' "No newline at end of file" */
export interface DiffLine {
  t: ' ' | '+' | '-' | '\\'
  text: string
  /** 원래 파일 줄 번호(추가 줄은 없음) */
  o?: number
  /** 새 파일 줄 번호(삭제 줄은 없음) */
  n?: number
}

export interface DiffHunk {
  oldStart: number
  oldLines: number
  newStart: number
  newLines: number
  /** @@ 뒤의 함수 문맥 등 */
  header: string
  lines: DiffLine[]
}

export interface FileDiff {
  path: string
  oldPath?: string
  binary: boolean
  hunks: DiffHunk[]
  /** 크기 상한으로 일부만 담았는지(full 요청으로 전체를 받을 수 있다) */
  truncated: boolean
  /** 전체 diff 줄 수(잘렸을 때 안내용, 모르면 담긴 줄 수) */
  totalLines: number
  /** 세기 한계에 걸려 totalLines 가 하한값인지("N+ 줄") */
  totalIsLowerBound?: boolean
  /** 모드 변경 등 내용 외 변경 안내("new file mode 100755" 등) */
  meta: string[]
}

// ── WIP: 작업 트리 변경 ──────────────────────────────────

/** WIP 파일이 속한 영역: 스테이지됨 · 스테이지 안 됨(추적 중인 파일의 작업 트리 변경) · 추적 안 됨 */
export type WipArea = 'staged' | 'unstaged' | 'untracked'

/** 작업 트리 상태(`git status`). 읽기 전용 — 스테이지·커밋은 하지 않는다 */
export interface WipStatus {
  /** 인덱스(스테이지) 변경. additions/deletions 는 계산하지 않아 null 이다 */
  staged: FileChange[]
  /** 작업 트리 변경(충돌 파일 포함, status 'U') */
  unstaged: FileChange[]
  /** 추적 안 되는 파일 경로(상한까지만 담는다) */
  untracked: string[]
  /** 상한 때문에 목록에 담지 못한 추적 안 되는 파일 수 */
  untrackedMore: number
  /** 충돌(병합 미해결) 파일 수 — unstaged 에도 'U' 로 들어 있다 */
  conflicted: number
  counts: {
    /** 기존 파일의 변경(수정·삭제·이름 변경 등). 스테이지/작업 트리를 합쳐 경로 기준으로 중복을 뺀다 */
    modified: number
    /** 새 파일(스테이지된 추가 + 추적 안 됨, 상한 밖 포함) */
    added: number
    staged: number
    unstaged: number
    untracked: number
  }
}

export interface WipDiffRequest {
  area: WipArea
  path: string
  /** 스테이지된 이름 변경의 원래 경로 */
  oldPath?: string
  /** true 면 크기 상한을 무시하고 전체를 불러온다 */
  full?: boolean
}
