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
}

export interface AppConfig {
  version: 1
  sources: Source[]
  repos: Repo[]
  ui: UiPrefs
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

/** 사이드바용 ref 스냅샷 */
export interface RefsSnapshot {
  head: HeadInfo
  local: Branch[]
  remotes: RemoteGroup[]
  tags: Tag[]
  worktrees: Worktree[]
  stashes: Stash[]
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

export interface GhLabel {
  name: string
  /** 6자리 hex(# 없음) — GitHub 가 정한 라벨 색. 화면에서는 토큰과 섞어 은은하게 쓴다 */
  color: string
}

/** 로그인 상태. 토큰 값은 renderer 로 절대 넘기지 않는다 */
export interface GhAuthState {
  signedIn: boolean
  user?: GhUser
  /** 'pat' | 'device' */
  method?: 'pat' | 'device'
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
export type GhReviewState = 'approved' | 'changes_requested' | 'commented' | 'pending' | 'none'

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

export interface FileDiffRequest {
  hash: string
  path: string
  oldPath?: string
  /** 비교 기준 부모. 생략하면 첫 부모(루트 커밋이면 빈 트리) */
  parent?: string
  /** true 면 크기 상한을 무시하고 전체를 불러온다(지연 로딩의 "전체 보기") */
  full?: boolean
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
