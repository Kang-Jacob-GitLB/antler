import { randomUUID } from 'node:crypto'
import { copyFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { LAUNCHPAD_KEY_RE, MAX_LAUNCHPAD_PINS, MAX_LAUNCHPAD_SNOOZES } from '@shared/launchpad'
import type { AppConfig, Profile, ProfileSaveInput, Repo, RepoPatch, Source, SourceKind, UiPrefs } from '@shared/types'
import {
  MAX_PROFILES,
  applySettings,
  checkEditor,
  checkProfileName,
  checkVisibleGroups,
  sameName,
  sanitizeProfiles,
  snapshotSettings,
  validateSettings
} from './profiles'

export const LOCAL_SOURCE_ID = 'local'

/** 환경 하나를 나타내는 소스. id 가 결정적이라 같은 배포판·호스트는 같은 소스로 모인다 */
export function sourceFor(kind: SourceKind, target?: string): Source {
  if (kind === 'local') return { id: LOCAL_SOURCE_ID, kind: 'local', label: 'Local' }
  if (!target) throw new Error(`${kind} 소스에는 ${kind === 'wsl' ? '배포판' : '호스트'} 이름이 필요합니다`)
  return kind === 'wsl'
    ? { id: `wsl:${target}`, kind, label: `WSL · ${target}`, distro: target }
    : { id: `ssh:${target}`, kind, label: `SSH · ${target}`, sshHost: target }
}

const isStr = (v: unknown, max: number): v is string => typeof v === 'string' && v.length <= max

/** renderer 가 저장하려는 Jira 설정(프로젝트 키·저장 필터·최근 이슈)의 모양과 크기를 확인한다 */
function checkJiraPrefs(patch: Partial<UiPrefs>): void {
  const bad = (): never => {
    throw new Error('잘못된 Jira 설정입니다')
  }
  const { jiraProjectKeys: keys, jiraSites: sites } = patch
  if (keys !== undefined && (!Array.isArray(keys) || keys.length > 50 || !keys.every((k) => typeof k === 'string' && /^[A-Z][A-Z0-9_]{1,19}$/.test(k)))) bad()
  if (sites === undefined) return
  if (!sites || typeof sites !== 'object' || Array.isArray(sites) || Object.keys(sites).length > 20) bad()
  for (const [site, d] of Object.entries(sites)) {
    if (site.length > 300 || !d || typeof d !== 'object') bad()
    const { filters, recent } = d
    if (filters !== undefined && (!Array.isArray(filters) || filters.length > 50 || !filters.every((f) => f && isStr(f.name, 100) && isStr(f.jql, 2000)))) bad()
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
    // 에디터 실행 파일·적용된 프로필은 main 만 정한다 — renderer 가 임의 프로그램을 가리키거나 프로필 표시를 위조하지 못하게 한다
    if ('editorExePath' in patch || 'activeProfileId' in patch) throw new Error('이 설정은 직접 바꿀 수 없습니다')
    if (patch.editor !== undefined) checkEditor(patch.editor)
    if (patch.visibleGroups !== undefined) checkVisibleGroups(patch.visibleGroups)
    checkJiraPrefs(patch)
    checkLaunchpadPrefs(patch)
    const ui = { ...this.config.ui, ...patch }
    // 보일 그룹이 빈 목록이면 제한 없음이다(패치로는 키를 지울 수 없어 빈 목록으로 알린다)
    if (ui.visibleGroups?.length === 0) delete ui.visibleGroups
    return this.update({ ...this.config, ui })
  }

  /** main 전용: 사용자가 파일 선택 창에서 고른 에디터 실행 파일(호출자가 checkChosenExe 로 검증한다). IPC 로 직접 노출하지 않는다 */
  setEditorExePath(path: string): AppConfig {
    if (typeof path !== 'string' || !path || path.length > 4096 || path.includes('\0')) throw new Error('잘못된 실행 파일 경로입니다')
    return this.update({ ...this.config, ui: { ...this.config.ui, editorExePath: path } })
  }

  // ── 프로필 ──

  /**
   * 새 프로필(id 없음) 또는 기존 프로필의 이름 변경·설정 덮어쓰기(id 있음). settings 를 주지 않으면 현재 설정에서 만든다.
   * 토큰은 프로필에 들어가지 않는다(PROFILE_KEYS 참고).
   */
  saveProfile(input: ProfileSaveInput): AppConfig {
    if (!input || typeof input !== 'object' || Object.keys(input).some((k) => !['id', 'name', 'settings'].includes(k))) {
      throw new Error('잘못된 프로필 입력입니다')
    }
    const profiles = this.config.profiles ?? []
    const settings = input.settings !== undefined ? validateSettings(input.settings) : undefined
    if (input.id === undefined) {
      if (profiles.length >= MAX_PROFILES) throw new Error(`프로필은 최대 ${MAX_PROFILES}개까지 만들 수 있습니다`)
      const name = checkProfileName(input.name)
      if (profiles.some((p) => sameName(p.name, name))) throw new Error('같은 이름의 프로필이 있습니다')
      const profile: Profile = { id: randomUUID(), name, settings: settings ?? snapshotSettings(this.config.ui) }
      // 현재 설정으로 만든 프로필은 곧바로 그 프로필을 쓰는 중이다
      const ui = settings ? this.config.ui : { ...this.config.ui, activeProfileId: profile.id }
      return this.update({ ...this.config, ui, profiles: [...profiles, profile] })
    }
    const cur = profiles.find((p) => p.id === input.id)
    if (!cur) throw new Error('프로필을 찾을 수 없습니다')
    const name = input.name !== undefined ? checkProfileName(input.name) : cur.name
    if (profiles.some((p) => p.id !== cur.id && sameName(p.name, name))) throw new Error('같은 이름의 프로필이 있습니다')
    const next: Profile = {
      id: cur.id,
      name,
      settings: settings ?? (input.name === undefined ? snapshotSettings(this.config.ui) : cur.settings)
    }
    return this.update({ ...this.config, profiles: profiles.map((p) => (p.id === cur.id ? next : p)) })
  }

  /** 지금 쓰는 프로필을 현재 설정으로 갱신해 두고, 선택한 프로필의 설정으로 바꾼다 */
  applyProfile(id: string): AppConfig {
    const profiles = this.config.profiles ?? []
    const target = profiles.find((p) => p.id === id)
    if (!target) throw new Error('프로필을 찾을 수 없습니다')
    const outgoingId = this.config.ui.activeProfileId
    const saved =
      outgoingId && outgoingId !== id
        ? profiles.map((p) => (p.id === outgoingId ? { ...p, settings: snapshotSettings(this.config.ui) } : p))
        : profiles
    const ui = { ...applySettings(this.config.ui, target.settings), activeProfileId: id }
    return this.update({ ...this.config, ui, profiles: saved })
  }

  duplicateProfile(id: string, name: string): AppConfig {
    const profiles = this.config.profiles ?? []
    const src = profiles.find((p) => p.id === id)
    if (!src) throw new Error('프로필을 찾을 수 없습니다')
    if (profiles.length >= MAX_PROFILES) throw new Error(`프로필은 최대 ${MAX_PROFILES}개까지 만들 수 있습니다`)
    const clean = checkProfileName(name)
    if (profiles.some((p) => sameName(p.name, clean))) throw new Error('같은 이름의 프로필이 있습니다')
    // 지금 쓰는 프로필을 복제하면 아직 저장 전인 변경도 담도록 현재 설정에서 만든다
    const settings = this.config.ui.activeProfileId === id ? snapshotSettings(this.config.ui) : structuredClone(src.settings)
    return this.update({ ...this.config, profiles: [...profiles, { id: randomUUID(), name: clean, settings }] })
  }

  deleteProfile(id: string): AppConfig {
    const profiles = this.config.profiles ?? []
    if (!profiles.some((p) => p.id === id)) throw new Error('프로필을 찾을 수 없습니다')
    const { activeProfileId, ...rest } = this.config.ui
    const ui = activeProfileId === id ? rest : this.config.ui
    return this.update({ ...this.config, ui, profiles: profiles.filter((p) => p.id !== id) })
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

  /** 저장소를 지우고, 더 이상 쓰는 저장소가 없는 WSL·SSH 소스도 함께 정리한다(로컬 소스는 항상 남긴다) */
  removeRepo(id: string): AppConfig {
    const ui = this.config.ui.lastRepoId === id ? { ...this.config.ui, lastRepoId: undefined } : this.config.ui
    const repos = this.config.repos.filter((r) => r.id !== id)
    const used = new Set(repos.map((r) => r.sourceId))
    const sources = this.config.sources.filter((s) => s.id === LOCAL_SOURCE_ID || used.has(s.id))
    const { [id]: _gone, ...fetchApprovals } = this.config.fetchApprovals ?? {}
    return this.update({ ...this.config, ui, repos, sources, fetchApprovals })
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
      const sources = raw.sources?.length ? raw.sources : base.sources
      if (!sources.some((s) => s.id === LOCAL_SOURCE_ID)) sources.unshift(base.sources[0]!)
      const profiles = sanitizeProfiles(raw.profiles)
      const ui: UiPrefs = { ...base.ui, ...raw.ui }
      // 없어진 프로필을 가리키는 표시는 지운다
      if (ui.activeProfileId !== undefined && !profiles.some((p) => p.id === ui.activeProfileId)) delete ui.activeProfileId
      return {
        version: 1,
        sources,
        repos: raw.repos ?? [],
        ui,
        ...(profiles.length ? { profiles } : {}),
        ...(raw.fetchApprovals ? { fetchApprovals: raw.fetchApprovals } : {})
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
