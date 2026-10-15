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

export interface RepoStatus {
  repoId: string
  ok: boolean
  error?: string
}
