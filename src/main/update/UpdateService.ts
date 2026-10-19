// 앱 업데이트 오케스트레이션. electron 에 직접 의존하지 않고(주입) 상태 기계·정책만 가진다.
//  - auto (Windows NSIS · Linux AppImage): electron-updater 백엔드가 내려받고 quitAndInstall
//  - manual-dmg (macOS) / manual-file (Windows portable · Linux deb): manual.ts 가 GitHub Releases 에서 검증하며 내려받고 파일을 연다
import type { ReleaseNotes, UpdateDismissAction, UpdateMode, UpdateState } from '@shared/types'
import { findManualUpdate, downloadVerified, type ManualUpdate } from './manual'
import { fetchReleaseNotes } from './releaseNotes'
import { UpdatePolicy, detectInstallKind, isOfferable, modeFor, parseDismiss, releaseNotesUrl, sanitizeError, verifyFile, type InstallKind, type InstallProbe } from './logic'

/** electron-updater 를 감싼 최소 인터페이스(autoBackend.ts 가 구현, 테스트는 모의) */
export interface AutoBackend {
  /** 새 버전 문자열, 없으면 null */
  check(): Promise<string | null>
  download(onProgress: (percent: number) => void): Promise<void>
  cancel(): void
  /** 설치를 시작하고 앱을 끝낸다. 모듈을 지연 로드하므로 실패할 수 있다 */
  quitAndInstall(): void | Promise<void>
}

export interface UpdateDeps {
  currentVersion: string
  platform: string
  arch: string
  env: Record<string, string | undefined>
  isPackaged: boolean
  /** Linux 설치 형태 판별용(process.execPath, resources/package-type 내용) */
  probe?: InstallProbe
  /** 설정에 저장된 '건너뛴 버전' 읽기/쓰기 */
  getSkipped(): string | undefined
  setSkipped(version: string): void
  send(state: UpdateState): void
  /** auto 모드에서만 필요(지연 생성) */
  createAutoBackend?: () => AutoBackend
  fetch?: typeof fetch
  downloadsDir(): string
  /** 파일 열기(dmg·deb → 기본 앱). 실패 시 오류 문자열을 돌려주는 shell.openPath 규약: 빈 문자열 = 성공 */
  openPath(path: string): Promise<string>
  /** 탐색기/Finder 에서 파일 위치 보여 주기(portable) */
  showItem(path: string): void
}

const FIRST_CHECK_MS = 10_000
const INTERVAL_MS = 6 * 60 * 60 * 1000
const FAKE = new Set(['available', 'downloading', 'downloaded', 'error'])
const FAKE_MODES = new Set<UpdateMode>(['auto', 'manual-dmg', 'manual-file'])

export class UpdateService {
  readonly kind: InstallKind
  readonly mode: UpdateMode
  private readonly policy: UpdatePolicy
  private readonly enabled: boolean
  private readonly fake: string | null
  private state: UpdateState
  private backend: AutoBackend | undefined
  private pending: ManualUpdate | null = null
  private pendingAutoVersion: string | null = null
  private abort: AbortController | null = null
  private busy = false
  private openedPath: string | null = null
  private timers: NodeJS.Timeout[] = []

  constructor(private readonly d: UpdateDeps) {
    this.kind = detectInstallKind(d.platform, d.env, d.probe)
    const devOk = !d.isPackaged || d.env['ANTLER_UPDATE_DEV'] === '1'
    const fakeEnv = d.env['ANTLER_UPDATE_FAKE']
    this.fake = devOk && fakeEnv && FAKE.has(fakeEnv) ? fakeEnv : null
    const fakeMode = d.env['ANTLER_UPDATE_FAKE_MODE'] as UpdateMode | undefined
    this.mode = this.fake && fakeMode && FAKE_MODES.has(fakeMode) ? fakeMode : modeFor(this.kind)
    this.enabled = this.kind !== 'unsupported' && (d.isPackaged || d.env['ANTLER_UPDATE_DEV'] === '1')
    this.policy = new UpdatePolicy(d.getSkipped)
    this.state = this.base('idle')
    if (this.fake) this.state = this.fakeState(this.fake)
  }

  private base(status: UpdateState['status'], extra: Partial<UpdateState> = {}): UpdateState {
    return { status, mode: this.mode, currentVersion: this.d.currentVersion, ...extra }
  }

  private fakeState(kind: string): UpdateState {
    const version = '0.2.0'
    const common = { version, notesUrl: releaseNotesUrl(version) }
    switch (kind) {
      case 'available':
        return this.base('available', common)
      case 'downloading':
        return this.base('downloading', { ...common, progress: 42 })
      case 'downloaded':
        return this.base('downloaded', { ...common, ...(this.mode !== 'auto' ? { fileName: this.mode === 'manual-dmg' ? 'Antler-0.2.0-arm64.dmg' : 'Antler-Portable-0.2.0.exe' } : {}) })
      default:
        return this.base('error', { ...common, error: 'GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요' })
    }
  }

  get(): UpdateState {
    return this.state
  }

  private notes: { at: number; value: Promise<ReleaseNotes> } | null = null

  /** 현재 버전의 릴리스 노트. 성공한 결과는 10분 동안 재사용하고, 실패하면 다음 요청 때 다시 가져온다 */
  releaseNotes(): Promise<ReleaseNotes> {
    if (this.notes && Date.now() - this.notes.at < 10 * 60 * 1000) return this.notes.value
    const value = fetchReleaseNotes(this.d.fetch ?? fetch, this.d.currentVersion, AbortSignal.timeout(15_000))
    const entry = { at: Date.now(), value }
    this.notes = entry
    value.catch(() => {
      if (this.notes === entry) this.notes = null
    })
    return value
  }

  private set(s: UpdateState): UpdateState {
    this.state = s
    this.d.send(s)
    return s
  }

  /** 시작 10초 뒤 첫 확인, 이후 6시간마다 */
  start(): void {
    if (!this.enabled || this.fake) return
    const first = setTimeout(() => void this.check(false), FIRST_CHECK_MS)
    const every = setInterval(() => void this.check(false), INTERVAL_MS)
    first.unref()
    every.unref()
    this.timers.push(first, every)
  }

  dispose(): void {
    for (const t of this.timers) {
      clearTimeout(t)
      clearInterval(t)
    }
    this.timers = []
    this.abort?.abort()
  }

  async check(manual: boolean): Promise<UpdateState> {
    if (this.fake) return this.state
    if (this.busy || this.state.status === 'downloading') return this.state
    if (!this.enabled) {
      // 개발 빌드 등: 확인하지 않는다(수동 확인이면 최신으로 안내)
      return manual ? this.set(this.base('not-available')) : this.state
    }
    // 이미 받아 둔 업데이트가 있으면 다시 확인하지 않는다
    if (this.state.status === 'downloaded') return this.state
    this.busy = true
    if (manual) this.set(this.base('checking'))
    try {
      let version: string | null = null
      if (this.mode === 'auto') {
        this.backend ??= this.d.createAutoBackend?.()
        if (!this.backend) throw new Error('업데이트를 사용할 수 없습니다')
        version = await this.backend.check()
        this.pending = null
        this.pendingAutoVersion = version
      } else {
        const u = await findManualUpdate(this.d.fetch ?? fetch, { current: this.d.currentVersion, kind: this.kind, arch: this.d.arch })
        this.pending = u
        version = u?.version ?? null
      }
      if (!version || !isOfferable(this.d.currentVersion, version)) {
        this.pendingAutoVersion = null
        return this.set(this.base(manual ? 'not-available' : 'idle'))
      }
      if (!this.policy.shouldNotify(version, manual)) return this.set(this.base('idle'))
      return this.set(this.base('available', { version, notesUrl: releaseNotesUrl(version) }))
    } catch (e) {
      // 자동 확인의 실패(오프라인 등)는 조용히 넘기고, 수동 확인만 알린다
      return manual ? this.set(this.base('error', { error: sanitizeError(e) })) : this.set(this.base('idle'))
    } finally {
      this.busy = false
    }
  }

  async download(): Promise<UpdateState> {
    if (this.fake) return this.state
    const version = this.state.version
    if (this.busy || !version || !['available', 'error'].includes(this.state.status)) return this.state
    this.busy = true
    const notesUrl = releaseNotesUrl(version)
    this.set(this.base('downloading', { version, notesUrl, progress: 0 }))
    let last = -1
    const progress = (p: number): void => {
      const pct = Math.max(0, Math.min(100, Math.round(p)))
      if (pct === last || this.state.status !== 'downloading') return
      last = pct
      this.set(this.base('downloading', { version, notesUrl, progress: pct }))
    }
    try {
      if (this.mode === 'auto') {
        if (!this.backend || this.pendingAutoVersion !== version) throw new Error('다시 확인한 뒤 시도하세요')
        await this.backend.download(progress)
        return this.set(this.base('downloaded', { version, notesUrl }))
      }
      // 오류 후 재시도 등으로 pending 이 없으면 다시 찾는다
      this.pending ??= await findManualUpdate(this.d.fetch ?? fetch, { current: this.d.currentVersion, kind: this.kind, arch: this.d.arch })
      if (!this.pending || this.pending.version !== version) throw new Error('다시 확인한 뒤 시도하세요')
      this.abort = new AbortController()
      const file = await downloadVerified(this.d.fetch ?? fetch, this.pending, this.d.downloadsDir(), progress, this.abort.signal)
      this.openedPath = file
      await this.open(file)
      return this.set(this.base('downloaded', { version, notesUrl, fileName: this.pending.asset.name }))
    } catch (e) {
      if (this.abort?.signal.aborted || this.state.status !== 'downloading') {
        return this.state.status === 'downloading' ? this.set(this.base('available', { version, notesUrl })) : this.state
      }
      return this.set(this.base('error', { version, notesUrl, error: sanitizeError(e) }))
    } finally {
      this.abort = null
      this.busy = false
    }
  }

  /** 검증을 마친 파일만 연다. dmg·deb 는 기본 앱(마운트·소프트웨어 센터), portable 은 파일 위치를 보여 준다 */
  private async open(file: string): Promise<void> {
    if (this.kind === 'portable') return this.d.showItem(file)
    const err = await this.d.openPath(file)
    if (err) throw new Error('설치 파일을 열지 못했습니다. 다운로드 폴더에서 직접 여세요')
  }

  cancel(): void {
    if (this.fake || this.state.status !== 'downloading') return
    // 상태는 즉시 되돌리고(download() 의 catch 가 이미 바뀐 상태를 보고 넘어간다), 진행 중인 전송을 끊는다
    const { version, notesUrl } = this.state
    this.set(this.base('available', { ...(version ? { version } : {}), ...(notesUrl ? { notesUrl } : {}) }))
    this.abort?.abort()
    this.backend?.cancel()
  }

  async install(): Promise<void> {
    if (this.fake || this.state.status !== 'downloaded') return
    if (this.mode === 'auto') {
      try {
        await this.backend?.quitAndInstall()
      } catch (e) {
        // 설치를 시작하지 못했다 — 받아 둔 상태에서 멈춰 있지 않게 오류로 알린다
        this.set(this.base('error', { ...(this.state.version ? { version: this.state.version } : {}), error: sanitizeError(e) }))
      }
      return
    }
    if (!this.openedPath || !this.pending) return
    // 다시 열기 전에 해시를 한 번 더 확인한다(받은 뒤 파일이 바뀌었을 수 있다)
    const v = await verifyFile(this.openedPath, this.pending.expected)
    if (!v.ok) {
      this.set(this.base('error', { ...(this.state.version ? { version: this.state.version } : {}), error: `다시 열기 전 검증 실패: ${v.reason}` }))
      return
    }
    await this.open(this.openedPath)
  }

  dismiss(action: unknown, version: unknown): UpdateState {
    const p = parseDismiss(action, version)
    if (this.fake) return this.set(this.base('idle'))
    // 지금 알리고 있는 버전에 대해서만 받는다(renderer 가 임의 버전으로 알림을 영구히 끄지 못하게)
    if (!['available', 'error'].includes(this.state.status) || this.state.version !== p.version) {
      throw new Error('현재 알림 중인 버전이 아닙니다')
    }
    if (p.action === 'skip') {
      this.d.setSkipped(p.version)
    } else {
      this.policy.snooze(p.version)
    }
    // 내려받는 중이거나 받아 둔 업데이트는 그대로 두고, 알림 단계에서만 숨긴다
    return ['available', 'error', 'not-available'].includes(this.state.status) ? this.set(this.base('idle')) : this.state
  }
}

export type { UpdateDismissAction }
