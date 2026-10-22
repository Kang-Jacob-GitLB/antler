import { randomUUID } from 'node:crypto'
import { copyFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { LAUNCHPAD_KEY_RE, MAX_LAUNCHPAD_PINS, MAX_LAUNCHPAD_SNOOZES } from '@shared/launchpad'
import type { AppConfig, EditorKind, Repo, RepoPatch, Source, SourceKind, UiPrefs, WindowState } from '@shared/types'
import { GITHUB_SOURCE_ID, isSafeBranch, sanitizeGithubLink } from '../git/githubCache'
import { sanitizeWindowState } from '../windowState'

export const LOCAL_SOURCE_ID = 'local'

/** 환경 하나를 나타내는 소스. id 가 결정적이라 같은 배포판·호스트는 같은 소스로 모인다 */
export function sourceFor(kind: SourceKind, target?: string): Source {
  if (kind === 'local') return { id: LOCAL_SOURCE_ID, kind: 'local', label: 'Local' }
  // GitHub 원격 전용 저장소는 환경이 하나뿐이다(앱 캐시). 저장소가 하나라도 있을 때만 소스가 존재한다
  if (kind === 'github') return { id: GITHUB_SOURCE_ID, kind: 'github', label: 'GitHub' }
  if (!target) throw new Error(`${kind} 소스에는 ${kind === 'wsl' ? '배포판' : '호스트'} 이름이 필요합니다`)
  return kind === 'wsl'
    ? { id: `wsl:${target}`, kind, label: `WSL · ${target}`, distro: target }
    : { id: `ssh:${target}`, kind, label: `SSH · ${target}`, sshHost: target }
}

const isStr = (v: unknown, max: number): v is string => typeof v === 'string' && v.length <= max

const EDITORS: readonly EditorKind[] = ['vscode', 'cursor', 'jetbrains', 'custom']

/** 보일 저장소 그룹 목록의 모양 검사(config:setUi 가 쓴다) */
export function checkVisibleGroups(v: unknown): string[] {
  const bad = (what: string): never => {
    throw new Error(`잘못된 ${what}입니다`)
  }
  if (!Array.isArray(v) || v.length > 50) return bad('그룹 목록')
  const out: string[] = []
  for (const g of v) {
    if (typeof g !== 'string' || !g.trim() || g.length > 200 || g.includes('\0')) return bad('그룹 이름')
    if (!out.includes(g)) out.push(g)
  }
  return out
}

export function checkEditor(v: unknown): EditorKind {
  if (!EDITORS.includes(v as EditorKind)) throw new Error('잘못된 에디터입니다')
  return v as EditorKind
}

/** renderer 가 저장하려는 Jira 설정(프로젝트 키·저장 필터·최근 이슈)의 모양과 크기를 확인한다 */
function checkJiraPrefs(patch: Partial<UiPrefs>): void {
  const bad = (): never => {
    throw new Error('잘못된 Jira 설정입니다')
  }
  const { jiraProjectKeys: keys, jiraSites: sites, jiraLayout: layout, jiraHiddenCategories: hidden } = patch
  if (keys !== undefined && (!Array.isArray(keys) || keys.length > 50 || !keys.every((k) => typeof k === 'string' && /^[A-Z][A-Z0-9_]{1,19}$/.test(k)))) bad()
  if (layout !== undefined && layout !== 'list' && layout !== 'board') bad()
  if (hidden !== undefined && (!Array.isArray(hidden) || !hidden.every((c) => c === 'new' || c === 'indeterminate' || c === 'done') || new Set(hidden).size !== hidden.length)) bad()
  // 워크로그 하루 표시 시각: 한쪽만 바꿔도 저장된 다른 쪽과 어긋나지 않게 호출부(setUi)가 합친 값을 다시 확인한다
  const { worklogDayStart: ds, worklogDayEnd: de } = patch
  if (ds !== undefined && (!Number.isInteger(ds) || ds < 0 || ds > 23)) bad()
  if (de !== undefined && (!Number.isInteger(de) || de < 1 || de > 24)) bad()
  if (ds !== undefined && de !== undefined && ds >= de) bad()
  const wm = patch.worklogMode
  if (wm !== undefined && wm !== 'day' && wm !== 'week' && wm !== 'month') bad()
  if (sites === undefined) return
  if (!sites || typeof sites !== 'object' || Array.isArray(sites) || Object.keys(sites).length > 20) bad()
  for (const [site, d] of Object.entries(sites)) {
    if (site.length > 300 || !d || typeof d !== 'object') bad()
    const { filters, recent, team } = d
    if (filters !== undefined && (!Array.isArray(filters) || filters.length > 50 || !filters.every((f) => f && isStr(f.name, 100) && isStr(f.jql, 2000)))) bad()
    if (team !== undefined && (!Array.isArray(team) || team.length > 30 || !team.every((u) => u && typeof u.id === 'string' && /^[A-Za-z0-9._@:+-]{1,128}$/.test(u.id) && isStr(u.name, 200)))) bad()
    if (recent !== undefined && (!Array.isArray(recent) || recent.length > 30 || !recent.every((r) => r && typeof r.key === 'string' && /^[A-Z][A-Z0-9_]{1,19}-\d{1,9}$/.test(r.key) && isStr(r.summary, 500)))) bad()
  }
}

/** renderer 가 저장하려는 Launchpad 핀·스누즈의 모양과 크기를 확인한다 */
function checkLaunchpadPrefs(patch: Partial<UiPrefs>): void {
  const bad = (): never => {
    throw new Error('잘못된 Launchpad 설정입니다')
  }
  const { launchpadPins: pins, launchpadSnoozes: snoozes } = patch
  if (pins !== undefined && (!Array.isArray(pins) || pins.length > MAX_LAUNCHPAD_PINS || !pins.every((k) => typeof k === 'string' && LAUNCHPAD_KEY_RE.test(k)))) bad()
  if (snoozes === undefined) return
  if (!snoozes || typeof snoozes !== 'object' || Array.isArray(snoozes)) bad()
  const entries = Object.entries(snoozes)
  if (entries.length > MAX_LAUNCHPAD_SNOOZES || !entries.every(([k, until]) => LAUNCHPAD_KEY_RE.test(k) && Number.isSafeInteger(until) && until > 0)) bad()
}

/** renderer 가 저장하려는 저장소 목록 순서·색(그룹 칩·환경 배지)의 모양과 크기를 확인한다 */
function checkRepoListPrefs(patch: Partial<UiPrefs>): void {
  const bad = (): never => {
    throw new Error('잘못된 저장소 목록 설정입니다')
  }
  const isName = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 200 && !v.includes('\0')
  const isList = (v: unknown, max: number): boolean => Array.isArray(v) && v.length <= max && v.every(isName) && new Set(v).size === v.length
  const isMap = (v: unknown, max: number): v is Record<string, unknown> =>
    !!v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length <= max && Object.keys(v).every(isName)
  const isHue = (v: unknown): boolean => Number.isInteger(v) && (v as number) >= 0 && (v as number) < 360
  const { groupColors, sourceColors, sourceOrder, groupOrder, repoOrder } = patch
  for (const colors of [groupColors, sourceColors]) {
    if (colors !== undefined && (!isMap(colors, 500) || !Object.values(colors).every(isHue))) bad()
  }
  if (sourceOrder !== undefined && !isList(sourceOrder, 500)) bad()
  if (repoOrder !== undefined && !isList(repoOrder, 5000)) bad()
  if (groupOrder !== undefined && (!isMap(groupOrder, 500) || !Object.values(groupOrder).every((g) => isList(g, 500)))) bad()
}

/**
 * 저장된 저장소 목록의 GitHub 링크를 검증한다. github 소스의 저장소인데 링크가 틀렸으면 fetch 할 대상도 URL 을 확인할 기준도
 * 없으므로 그 항목을 버리고, github 소스가 아닌 저장소에 붙은 링크는 필드만 버린다.
 */
function sanitizeRepos(repos: Repo[]): Repo[] {
  const out: Repo[] = []
  for (const r of repos) {
    if (r.sourceId === GITHUB_SOURCE_ID) {
      const link = sanitizeGithubLink(r.github)
      if (link) out.push({ ...r, github: link })
    } else if (r.github !== undefined) {
      const { github: _stray, ...rest } = r
      out.push(rest)
    } else {
      out.push(r)
    }
  }
  return out
}

function defaults(): AppConfig {
  return {
    version: 1,
    sources: [{ id: LOCAL_SOURCE_ID, kind: 'local', label: 'Local' }],
    repos: [],
    ui: { theme: 'system', locale: 'ko', paneWidths: [248, 380] }
  }
}

/** 소스·저장소 목록과 UI 설정을 JSON 파일 하나에 저장한다. 쓰기는 임시 파일 → rename 으로 원자적으로 한다. */
export class ConfigStore {
  private config: AppConfig

  constructor(private readonly file: string) {
    this.config = this.load()
  }

  get(): AppConfig {
    return this.config
  }

  source(id: string): Source | undefined {
    return this.config.sources.find((s) => s.id === id)
  }

  repo(id: string): Repo | undefined {
    return this.config.repos.find((r) => r.id === id)
  }

  setUi(patch: Partial<UiPrefs>): AppConfig {
    // renderer 가 보낸 값이다 — main 이 그대로 쓰는 값은 형식을 확인한다
    const cid = patch.githubClientId
    if (cid !== undefined && (typeof cid !== 'string' || !/^[A-Za-z0-9._-]{1,100}$/.test(cid))) {
      throw new Error('잘못된 OAuth App client ID 입니다(영문·숫자·._- 100자 이내)')
    }
    // 건너뛴 업데이트 버전은 main(UpdateService)만 쓴다 — renderer 가 알림을 영구히 끄지 못하게 막는다
    if ('skippedUpdateVersion' in patch) throw new Error('이 설정은 직접 바꿀 수 없습니다')
    const af = patch.autoFetchMinutes
    if (af !== undefined && (!Number.isInteger(af) || af < 0 || af > 1440)) throw new Error('잘못된 자동 fetch 주기입니다(0~1440분)')
    // 에디터 실행 파일은 main 만 정한다 — renderer 가 임의 프로그램을 가리키지 못하게 한다
    if ('editorExePath' in patch) throw new Error('이 설정은 직접 바꿀 수 없습니다')
    if (patch.editor !== undefined) checkEditor(patch.editor)
    if (patch.visibleGroups !== undefined) checkVisibleGroups(patch.visibleGroups)
    checkJiraPrefs(patch)
    checkLaunchpadPrefs(patch)
    checkRepoListPrefs(patch)
    const ui = { ...this.config.ui, ...patch }
    if (('worklogDayStart' in patch || 'worklogDayEnd' in patch) && (ui.worklogDayStart ?? 7) >= (ui.worklogDayEnd ?? 21)) throw new Error('잘못된 Jira 설정입니다')
    // 보일 그룹이 빈 목록이면 제한 없음이다(패치로는 키를 지울 수 없어 빈 목록으로 알린다)
    if (ui.visibleGroups?.length === 0) delete ui.visibleGroups
    return this.update({ ...this.config, ui })
  }

  /** main 전용: 사용자가 파일 선택 창에서 고른 에디터 실행 파일(호출자가 checkChosenExe 로 검증한다). IPC 로 직접 노출하지 않는다 */
  setEditorExePath(path: string): AppConfig {
    if (typeof path !== 'string' || !path || path.length > 4096 || path.includes('\0')) throw new Error('잘못된 실행 파일 경로입니다')
    return this.update({ ...this.config, ui: { ...this.config.ui, editorExePath: path } })
  }

  /** main 전용: 창 위치·크기(IPC 로 노출하지 않는다). 같은 값이면 쓰지 않는다 */
  setWindowState(state: WindowState): void {
    const next = sanitizeWindowState(state)
    if (!next) return
    const cur = this.config.window
    if (cur && cur.x === next.x && cur.y === next.y && cur.width === next.width && cur.height === next.height && !!cur.maximized === !!next.maximized) return
    this.update({ ...this.config, window: next })
  }

  /** main 전용: 사용자가 '이 버전 건너뛰기'를 고른 업데이트 버전(IPC 로 노출하지 않는다) */
  setSkippedUpdateVersion(version: string): AppConfig {
    if (typeof version !== 'string' || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]{1,40})?$/.test(version)) throw new Error('잘못된 버전입니다')
    return this.update({ ...this.config, ui: { ...this.config.ui, skippedUpdateVersion: version } })
  }

  /**
   * 같은 소스·경로가 이미 있으면 기존 항목을 돌려준다.
   * source 를 함께 주면(처음 쓰는 WSL 배포판·SSH 호스트) 저장소와 같은 쓰기로 소스도 추가한다.
   */
  addRepo(input: Omit<Repo, 'id'>, source?: Source): Repo {
    const existing = this.config.repos.find((r) => r.sourceId === input.sourceId && r.path === input.path)
    if (existing) return existing
    const repo: Repo = { ...input, id: randomUUID() }
    const sources = source && !this.source(source.id) ? [...this.config.sources, source] : this.config.sources
    this.update({ ...this.config, sources, repos: [...this.config.repos, repo] })
    return repo
  }

  updateRepo(id: string, patch: RepoPatch): AppConfig {
    const repos = this.config.repos.map((r) => {
      if (r.id !== id) return r
      const next: Repo = { ...r }
      if (patch.name !== undefined && patch.name.trim()) next.name = patch.name.trim()
      if (patch.group !== undefined) {
        const g = patch.group?.trim()
        if (g) next.group = g
        else delete next.group
      }
      return next
    })
    return this.update({ ...this.config, repos })
  }

  /** main 전용: 첫 fetch 뒤 알아낸 GitHub 저장소의 기본 브랜치를 레코드에 남긴다 */
  setGithubDefaultBranch(id: string, branch: string): void {
    const cur = this.repo(id)
    if (!cur?.github || cur.github.defaultBranch === branch || !isSafeBranch(branch)) return
    const repos = this.config.repos.map((r) => (r.id === id ? { ...r, github: { ...cur.github!, defaultBranch: branch } } : r))
    this.update({ ...this.config, repos })
  }

  /** 저장소를 지우고, 더 이상 쓰는 저장소가 없는 WSL·SSH·GitHub 소스도 함께 정리한다(로컬 소스는 항상 남긴다) */
  removeRepo(id: string): AppConfig {
    const ui = this.config.ui.lastRepoId === id ? { ...this.config.ui, lastRepoId: undefined } : this.config.ui
    const repos = this.config.repos.filter((r) => r.id !== id)
    const used = new Set(repos.map((r) => r.sourceId))
    const sources = this.config.sources.filter((s) => s.id === LOCAL_SOURCE_ID || used.has(s.id))
    const { [id]: _gone, ...fetchApprovals } = this.config.fetchApprovals ?? {}
    const { [id]: _untrusted, ...trustedConfigs } = this.config.trustedConfigs ?? {}
    return this.update({ ...this.config, ui, repos, sources, fetchApprovals, trustedConfigs })
  }

  trustedConfig(repoId: string): string | undefined {
    return this.config.trustedConfigs?.[repoId]
  }

  /** fetch 확인 창에서 항상 허용한 저장소 설정 지문을 기록한다. main 에서만 호출한다 */
  setTrustedConfig(repoId: string, fingerprint: string): void {
    if (this.config.trustedConfigs?.[repoId] === fingerprint) return
    this.update({ ...this.config, trustedConfigs: { ...this.config.trustedConfigs, [repoId]: fingerprint } })
  }

  fetchApproval(repoId: string): string | undefined {
    return this.config.fetchApprovals?.[repoId]
  }

  /** 수동 fetch 가 성공한 remote 지문을 기록한다(null 이면 지운다). main 에서만 호출한다 */
  setFetchApproval(repoId: string, fingerprint: string | null): void {
    if (this.config.fetchApprovals?.[repoId] === (fingerprint ?? undefined)) return
    const { [repoId]: _old, ...rest } = this.config.fetchApprovals ?? {}
    this.update({ ...this.config, fetchApprovals: fingerprint ? { ...rest, [repoId]: fingerprint } : rest })
  }

  /** 디스크에 먼저 쓰고, 성공했을 때만 메모리 상태를 바꾼다 */
  private update(next: AppConfig): AppConfig {
    this.save(next)
    this.config = next
    return next
  }

  private load(): AppConfig {
    const base = defaults()
    let text: string
    try {
      text = readFileSync(this.file, 'utf8')
    } catch {
      return base // 첫 실행
    }
    try {
      const raw = JSON.parse(text) as Partial<AppConfig>
      const repos = sanitizeRepos(raw.repos ?? [])
      // github 소스는 정해진 모양 하나뿐이고, 저장소가 있을 때만 존재한다(파일에서 바뀐 값은 믿지 않고 다시 만든다)
      const hasGithub = repos.some((r) => r.sourceId === GITHUB_SOURCE_ID)
      const sources = (raw.sources?.length ? raw.sources : base.sources).filter((s) => s.id !== GITHUB_SOURCE_ID && s.kind !== 'github')
      if (hasGithub) sources.push(sourceFor('github'))
      if (!sources.some((s) => s.id === LOCAL_SOURCE_ID)) sources.unshift(base.sources[0]!)
      const ui: UiPrefs = { ...base.ui, ...raw.ui }
      // 예전 버전이 남긴 프로필 표시는 버린다(다음 저장 때 파일에서도 사라진다. raw.profiles 도 옮기지 않는다)
      delete (ui as { activeProfileId?: unknown }).activeProfileId
      // 손상된 하루 표시 시각은 두 키 모두 버려 기본값으로 돌린다(그대로 두면 이후 setUi 가 계속 실패한다)
      const { worklogDayStart: ds, worklogDayEnd: de } = ui
      if (ds !== undefined || de !== undefined) {
        const ok = Number.isInteger(ds) && Number.isInteger(de) && ds! >= 0 && de! <= 24 && ds! < de!
        if (!ok) {
          delete ui.worklogDayStart
          delete ui.worklogDayEnd
        }
      }
      // 알 수 없는 워크로그 보기 값은 버린다(기본 월 보기)
      const wm: unknown = ui.worklogMode
      if (wm !== undefined && wm !== 'day' && wm !== 'week' && wm !== 'month') delete ui.worklogMode
      const win = sanitizeWindowState(raw.window)
      return {
        version: 1,
        sources,
        repos,
        ui,
        ...(raw.fetchApprovals ? { fetchApprovals: raw.fetchApprovals } : {}),
        ...(raw.trustedConfigs ? { trustedConfigs: raw.trustedConfigs } : {}),
        ...(win ? { window: win } : {})
      }
    } catch {
      // 손상된 설정은 덮어쓰기 전에 보존해 둔다(사용자가 저장소 목록을 복구할 수 있게)
      try {
        copyFileSync(this.file, `${this.file}.corrupt-${Date.now()}.bak`)
      } catch {
        /* 보존 실패는 무시하고 기본값으로 시작한다 */
      }
      return base
    }
  }

  private save(config: AppConfig): void {
    mkdirSync(dirname(this.file), { recursive: true })
    const tmp = `${this.file}.tmp`
    writeFileSync(tmp, JSON.stringify(config, null, 2), 'utf8')
    // Windows 에서 백신·인덱서가 파일을 잠깐 잡으면 rename 이 EPERM 으로 실패한다 — 몇 번 재시도한다
    for (let attempt = 0; ; attempt++) {
      try {
        renameSync(tmp, this.file)
        return
      } catch (err) {
        if (attempt >= 4 || (err as NodeJS.ErrnoException).code !== 'EPERM') throw err
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 30 * (attempt + 1))
      }
    }
  }
}
