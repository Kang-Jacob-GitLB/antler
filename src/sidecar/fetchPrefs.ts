// fetch 설정(자동 주기·승인 지문)을 Rust 에 알린다. 미러(mirror.ts)와 따로 보낸다 — 미러의 중복 제거·on_mirror_changed 와 무관하다. 프로토콜은 protocol.ts.
import type { AppConfig } from '@shared/types'
import { DEFAULT_AUTO_FETCH_MINUTES } from '../main/services/FetchService'
import type { FromSidecar } from './protocol'

export type FetchPrefsMessage = Extract<FromSidecar, { t: 'fetchPrefs' }>
type Prefs = Omit<FetchPrefsMessage, 't' | 'gen'>

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

/** 설정에서 Rust 가 쓸 값만 뽑는다. 주기는 이미 기본값(5분)을 적용한 값이며, 음수·무한·NaN 은 0(끔)이다 — 기본값을 정하는 곳은 여기 한 곳이다 */
export function fetchPrefsOf(config: AppConfig): Prefs {
  const m = config.ui.autoFetchMinutes ?? DEFAULT_AUTO_FETCH_MINUTES
  return {
    autoFetchMinutes: Number.isFinite(m) && m > 0 ? m : 0,
    approvals: onlyKnown(config.fetchApprovals, config),
    trusted: onlyKnown(config.trustedConfigs, config)
  }
}

/** 값이 바뀌었을 때만 `fetchPrefs` 줄을 보낸다(창 위치 같은 무관한 쓰기는 보내지 않고 gen 도 올리지 않는다) */
export class FetchPrefsSender {
  private gen = 0
  private last: string | null = null

  constructor(private readonly send: (msg: FetchPrefsMessage) => void) {}

  push(config: AppConfig): void {
    const prefs = fetchPrefsOf(config)
    const key = JSON.stringify(prefs)
    if (key === this.last) return
    this.last = key
    this.send({ t: 'fetchPrefs', gen: ++this.gen, ...prefs })
  }
}
