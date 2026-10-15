import { existsSync, watch, type FSWatcher } from 'node:fs'
import { join } from 'node:path'
import type { RepoService } from './RepoService'

/** 원격 환경 폴링 주기(ms). 창이 포커스를 잃으면 FOCUS_SLOWDOWN 배로 늦춘다 */
const POLL_MS = { wsl: 4_000, ssh: 30_000 } as const
const FOCUS_SLOWDOWN = 4
/** 로컬 .git 이벤트를 모아서 한 번만 확인하는 지연 */
const DEBOUNCE_MS = 400
/** 이벤트가 몰려도 시그니처 확인(git 실행) 사이에 두는 최소 간격 */
const MIN_CHECK_INTERVAL_MS = 2_000
/** 시그니처 확인 제한 시간(멈춘 wsl.exe·SSH 가 감시를 붙잡지 않게) */
const CHECK_TIMEOUT_MS = 20_000
/** 로컬에서도 watcher 가 놓친 변화(네트워크 드라이브 등)를 잡는 안전망 폴링 */
const LOCAL_SAFETY_POLL_MS = 60_000

/**
 * 활성 저장소 하나만 감시한다(비활성 저장소는 감시·폴링하지 않는다).
 * - 로컬: .git(워크트리면 공통 디렉터리까지) 재귀 fs.watch → 디바운스 → ref 시그니처 비교
 * - WSL: 9P 공유는 파일 이벤트가 믿을 만하지 않아 짧은 주기 폴링
 * - SSH: 저빈도 폴링(+ 사용자의 수동 새로고침)
 * 시그니처가 바뀐 경우에만 onChange 를 부른다 — .git 안의 잦은 변화(index, objects)는 걸러진다.
 */
export class RepoWatcher {
  private repoId: string | null = null
  private generation = 0
  private watchers: FSWatcher[] = []
  private timer: ReturnType<typeof setTimeout> | null = null
  private debounce: ReturnType<typeof setTimeout> | null = null
  private lastSig: string | null = null
  private focused = true
  /** 확인이 진행 중인 세대(없으면 null) */
  private checkingGen: number | null = null
  private lastCheck = 0
  private pendingSince = 0
  /** 기준 시그니처를 계산하는 중 — 이때 포커스로 확인을 또 돌리면 같은 for-each-ref 를 한 번 더 실행한다 */
  private baselining = false

  constructor(
    private readonly repos: RepoService,
    private readonly onChange: (repoId: string) => void
  ) {}

  /** 지금 감시 중인 저장소 */
  get current(): string | null {
    return this.repoId
  }

  async watch(repoId: string | null): Promise<void> {
    if (repoId === this.repoId) return
    this.stop()
    this.repoId = repoId
    const gen = ++this.generation
    if (!repoId) return
    // 기준 시그니처를 먼저 잡아 둔다(감시 시작 전 상태와 비교). 첫 화면이 나온 뒤에 시작한다
    this.baselining = true
    try {
      await this.repos.afterFirstPage(repoId)
      if (gen !== this.generation) return
      // signal 이 없는 호출은 제한 시간이 있는 공유 계산이다 — 멈춘 원격이어도 baselining 이 영영 남지 않는다
      this.lastSig = await this.repos.signature(repoId).catch(() => null)
      if (gen !== this.generation) return
    } finally {
      // 다른 저장소로 바뀌어 중단된 경우에는 새 감시의 상태를 건드리지 않는다
      if (gen === this.generation) this.baselining = false
    }
    const kind = this.repos.repoKind(repoId)
    if (kind === 'local') await this.watchLocal(repoId, gen)
    this.schedule(gen)
  }

  /** 창 포커스 변화. 다시 포커스를 얻으면 바로 한 번 확인한다(다른 도구에서 커밋하고 돌아온 경우) */
  setFocused(focused: boolean): void {
    this.focused = focused
    if (focused && this.repoId && !this.baselining) void this.check(this.generation)
  }

  dispose(): void {
    this.stop()
    this.repoId = null
  }

  private async watchLocal(repoId: string, gen: number): Promise<void> {
    const dirs = await this.repos.localGitDirs(repoId).catch(() => null)
    if (!dirs || gen !== this.generation) return
    for (const dir of dirs) {
      // .git 파일의 gitdir/commondir 은 저장소가 정한다 — 악성 저장소가 홈 전체나 네트워크 경로를 가리키면
      // 재귀 감시가 이벤트 폭주(DoS)가 되므로, 실제 git 디렉터리(HEAD 가 있는 곳)이고 네트워크 경로가 아닐 때만 감시한다
      if (/^[\\/]{2}/.test(dir) || !existsSync(join(dir, 'HEAD'))) continue
      try {
        const w = watch(dir, { recursive: true }, (_event, file) => {
          // 객체 파일은 fetch·gc 때 대량으로 바뀐다 — ref 가 함께 바뀌므로 그쪽 이벤트로 충분하다
          const name = typeof file === 'string' ? file.replace(/\\/g, '/') : ''
          if (name.startsWith('objects/')) return
          this.kick(gen)
        })
        w.on('error', () => {}) // 디렉터리가 사라지면(저장소 삭제) 폴링 안전망이 처리한다
        this.watchers.push(w)
      } catch {
        // 재귀 감시를 지원하지 않는 파일 시스템 — 안전망 폴링에 맡긴다
      }
    }
  }

  /** 이벤트가 쉬지 않고 와도 확인은 MIN_CHECK_INTERVAL_MS 에 한 번을 넘지 않는다(디바운스만으로는 계속 밀린다) */
  private kick(gen: number): void {
    const now = Date.now()
    if (this.debounce) {
      // 이미 오래 기다린 확인은 더 미루지 않는다 — 계속 미루면 이벤트가 끊이지 않는 동안 영영 확인하지 못한다
      if (now - this.pendingSince > MIN_CHECK_INTERVAL_MS) return
      clearTimeout(this.debounce)
    } else {
      this.pendingSince = now
    }
    const wait = Math.max(DEBOUNCE_MS, this.lastCheck + MIN_CHECK_INTERVAL_MS - now)
    this.debounce = setTimeout(() => {
      this.debounce = null
      void this.check(gen)
    }, wait)
  }

  private schedule(gen: number): void {
    if (this.timer) clearTimeout(this.timer)
    const kind = this.repoId ? this.repos.repoKind(this.repoId) : null
    if (!kind) return
    const base = kind === 'local' ? LOCAL_SAFETY_POLL_MS : POLL_MS[kind]
    this.timer = setTimeout(
      () => {
        void this.check(gen).finally(() => {
          if (gen === this.generation) this.schedule(gen)
        })
      },
      this.focused ? base : base * FOCUS_SLOWDOWN
    )
  }

  private async check(gen: number): Promise<void> {
    const repoId = this.repoId
    // 진행 중 표시는 세대별이다 — 이전 저장소의 확인이 멈춰 있어도 새 저장소 감시를 막지 않는다
    if (!repoId || gen !== this.generation || this.checkingGen === gen) return
    this.checkingGen = gen
    this.lastCheck = Date.now()
    try {
      const sig = await this.repos.signature(repoId, AbortSignal.timeout(CHECK_TIMEOUT_MS))
      if (gen !== this.generation) return
      if (this.lastSig !== null && sig !== this.lastSig) this.onChange(repoId)
      this.lastSig = sig
    } catch {
      // 일시적인 실패(SSH 끊김·시간 초과·잠금 경합)는 다음 주기에 다시 본다
    } finally {
      if (this.checkingGen === gen) this.checkingGen = null
    }
  }

  private stop(): void {
    this.generation++
    this.baselining = false
    for (const w of this.watchers) w.close()
    this.watchers = []
    if (this.timer) clearTimeout(this.timer)
    if (this.debounce) clearTimeout(this.debounce)
    this.timer = null
    this.debounce = null
    this.lastSig = null
  }
}
