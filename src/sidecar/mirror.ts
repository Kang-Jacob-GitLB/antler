// 저장소 미러(Rust 라우터용). 설정에서 저장소별 {id, kind, path, distro?} 만 뽑아 바뀔 때만 보낸다. 프로토콜은 protocol.ts.
import type { AppConfig } from '@shared/types'
import type { FromSidecar, MirrorRepo } from './protocol'

/** 설정의 저장소를 미러 모양으로 바꾼다. 소스를 못 찾은 저장소는 라우터가 쓸 수 없으므로 뺀다 */
export function mirrorOf(config: AppConfig): MirrorRepo[] {
  const out: MirrorRepo[] = []
  for (const r of config.repos) {
    const s = config.sources.find((x) => x.id === r.sourceId)
    if (!s) continue
    out.push({ id: r.id, kind: s.kind, path: r.path, ...(s.kind === 'wsl' && s.distro ? { distro: s.distro } : {}) })
  }
  return out
}

/**
 * 미러가 바뀌었을 때만 `repos` 줄을 보낸다. 창 위치·UI 설정처럼 저장소와 무관한 쓰기도 ConfigStore.update 를 거치므로
 * 직전에 보낸 것과 같으면 보내지 않는다(gen 도 올리지 않는다).
 */
export class MirrorSender {
  private gen = 0
  private last: string | null = null

  constructor(private readonly send: (msg: Extract<FromSidecar, { t: 'repos' }>) => void) {}

  push(config: AppConfig): void {
    const repos = mirrorOf(config)
    const key = JSON.stringify(repos)
    if (key === this.last) return
    this.last = key
    this.send({ t: 'repos', gen: ++this.gen, repos })
  }
}
