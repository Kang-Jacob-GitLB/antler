import { randomUUID } from 'node:crypto'
import { copyFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
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
