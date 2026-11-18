// Node 사이드카(src/sidecar, Phase 6c PR 5 에서 제거)가 Rust 에 넘기던 값의 변환 규칙 — 골든(tests/golden/**)을 만드는 동결된 사양이다.
// Rust 가 같은 값을 만드는지는 Rust 시험(tests/golden.rs·config.rs·fetch*.rs)이 본다. 앱 코드가 아니라 시험 전용이다.
import type { AppConfig } from '@shared/types'
import { DEFAULT_AUTO_FETCH_MINUTES } from '../../src/main/services/FetchService'

/** Rust 라우터가 저장소 경로·환경을 읽는 출처(미러)의 한 항목 */
export interface MirrorRepo {
  id: string
  kind: 'local' | 'wsl' | 'ssh' | 'github'
  path: string
  name: string
  distro?: string
  sshHost?: string
  github?: { owner: string; repo: string; defaultBranch?: string }
}

/** 설정의 저장소를 미러 모양으로 바꾼다(wsl 은 distro, ssh 는 sshHost, github 는 owner/repo 링크와 기본 브랜치를 함께 담는다). 소스를 못 찾은 저장소는 뺀다 */
export function mirrorOf(config: AppConfig): MirrorRepo[] {
  const out: MirrorRepo[] = []
  for (const r of config.repos) {
    const s = config.sources.find((x) => x.id === r.sourceId)
    if (!s) continue
    out.push({
      id: r.id,
      kind: s.kind,
      path: r.path,
      name: r.name,
      ...(s.kind === 'wsl' && s.distro ? { distro: s.distro } : {}),
      ...(s.kind === 'ssh' && s.sshHost ? { sshHost: s.sshHost } : {}),
      ...(s.kind === 'github' && r.github
        ? { github: { owner: r.github.owner, repo: r.github.repo, ...(r.github.defaultBranch ? { defaultBranch: r.github.defaultBranch } : {}) } }
        : {})
    })
  }
  return out
}

type Prefs = { autoFetchMinutes: number; approvals: Record<string, string>; trusted: Record<string, string> }

/** 이 저장소 목록 안의 id 만, 키 순서를 고정해 담는다(저장소가 사라지면 그 승인도 함께 사라진다 — ConfigStore.removeRepo 와 같다) */
function onlyKnown(map: Record<string, string> | undefined, config: AppConfig): Record<string, string> {
  const out: Record<string, string> = {}
  if (!map) return out
  for (const id of config.repos.map((r) => r.id).sort()) {
    const v = map[id]
    if (typeof v === 'string') out[id] = v
  }
  return out
}

/** 설정에서 fetch 가 쓸 값만 뽑는다. 주기는 이미 기본값(5분)을 적용한 값이며, 음수·무한·NaN 은 0(끔)이다 */
export function fetchPrefsOf(config: AppConfig): Prefs {
  const m = config.ui.autoFetchMinutes ?? DEFAULT_AUTO_FETCH_MINUTES
  return {
    autoFetchMinutes: Number.isFinite(m) && m > 0 ? m : 0,
    approvals: onlyKnown(config.fetchApprovals, config),
    trusted: onlyKnown(config.trustedConfigs, config)
  }
}

/** 확인 창 인자(네이티브 대화상자). cancelId 버튼은 Rust 가 마지막(Esc·닫기와 같은 자리)에 둔다 — src-tauri/src/dialogs.rs 참고 */
export interface MessageBoxArg {
  kind: 'info' | 'warning' | 'error'
  title: string
  /** message 와 detail 을 이어 붙인 본문 */
  message: string
  /** 1~3개, 서로 달라야 한다(눌린 버튼을 문구로 구분한다) */
  buttons: string[]
  cancelId: number
}

type MessageBoxOpts = { type?: unknown; title?: unknown; message?: unknown; detail?: unknown; buttons?: unknown; cancelId?: unknown }

const MESSAGE_KINDS: Record<string, MessageBoxArg['kind']> = { warning: 'warning', error: 'error' }

/** Electron `dialog.showMessageBox` 옵션을 Rust 에 넘기던 MessageBoxArg 로 바꾼다. 잘못된 옵션(버튼 수·cancelId 범위·중복)은 null 이다(그때는 취소로 답했다) */
export function messageBoxArgOf(opts: MessageBoxOpts): MessageBoxArg | null {
  if (typeof opts.cancelId !== 'number') return null
  const buttons = opts.buttons
  if (!Array.isArray(buttons) || buttons.length < 1 || buttons.length > 3) return null
  if (!Number.isInteger(opts.cancelId) || opts.cancelId < 0 || opts.cancelId >= buttons.length) return null
  if (!buttons.every((b) => typeof b === 'string' && b !== '') || new Set(buttons).size !== buttons.length) return null
  const str = (v: unknown): string => (typeof v === 'string' ? v : '')
  return {
    kind: MESSAGE_KINDS[str(opts.type)] ?? 'info',
    title: str(opts.title),
    message: [str(opts.message), str(opts.detail)].filter(Boolean).join('\n\n'),
    buttons: buttons as string[],
    cancelId: opts.cancelId
  }
}
