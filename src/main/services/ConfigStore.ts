import { randomUUID } from 'node:crypto'
import { copyFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import type { AppConfig, Repo, RepoPatch, Source, SourceKind, UiPrefs } from '@shared/types'

export const LOCAL_SOURCE_ID = 'local'

/** 환경 하나를 나타내는 소스. id 가 결정적이라 같은 배포판·호스트는 같은 소스로 모인다 */
export function sourceFor(kind: SourceKind, target?: string): Source {
  if (kind === 'local') return { id: LOCAL_SOURCE_ID, kind: 'local', label: 'Local' }
  if (!target) throw new Error(`${kind} 소스에는 ${kind === 'wsl' ? '배포판' : '호스트'} 이름이 필요합니다`)
  return kind === 'wsl'
    ? { id: `wsl:${target}`, kind, label: `WSL · ${target}`, distro: target }
    : { id: `ssh:${target}`, kind, label: `SSH · ${target}`, sshHost: target }
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
    return this.update({ ...this.config, ui: { ...this.config.ui, ...patch } })
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
    return this.update({ ...this.config, ui, repos, sources })
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
      return { version: 1, sources, repos: raw.repos ?? [], ui: { ...base.ui, ...raw.ui } }
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
