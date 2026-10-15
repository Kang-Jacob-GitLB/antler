import { randomUUID } from 'node:crypto'
import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import type { AppConfig, Repo, Source, UiPrefs } from '@shared/types'

export const LOCAL_SOURCE_ID = 'local'

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
    return this.update({ ...this.config, ui: { ...this.config.ui, ...patch } })
  }

  /** 같은 소스·경로가 이미 있으면 기존 항목을 돌려준다. */
  addRepo(input: Omit<Repo, 'id'>): Repo {
    const existing = this.config.repos.find((r) => r.sourceId === input.sourceId && r.path === input.path)
    if (existing) return existing
    const repo: Repo = { ...input, id: randomUUID() }
    this.update({ ...this.config, repos: [...this.config.repos, repo] })
    return repo
  }

  removeRepo(id: string): AppConfig {
    const ui = this.config.ui.lastRepoId === id ? { ...this.config.ui, lastRepoId: undefined } : this.config.ui
    return this.update({ ...this.config, ui, repos: this.config.repos.filter((r) => r.id !== id) })
  }

  private update(next: AppConfig): AppConfig {
    this.config = next
    this.save()
    return next
  }

  private load(): AppConfig {
    const base = defaults()
    try {
      const raw = JSON.parse(readFileSync(this.file, 'utf8')) as Partial<AppConfig>
      const sources = raw.sources?.length ? raw.sources : base.sources
      if (!sources.some((s) => s.id === LOCAL_SOURCE_ID)) sources.unshift(base.sources[0]!)
      return { version: 1, sources, repos: raw.repos ?? [], ui: { ...base.ui, ...raw.ui } }
    } catch {
      return base
    }
  }

  private save(): void {
    mkdirSync(dirname(this.file), { recursive: true })
    const tmp = `${this.file}.tmp`
    writeFileSync(tmp, JSON.stringify(this.config, null, 2), 'utf8')
    renameSync(tmp, this.file)
  }
}
