// 사이드카의 FetchService 가 맡는 저장소 판정. Rust 가 fetch 를 맡는 환경(local·wsl, 그리고 Rust 가 ssh 를 풀 때는 ssh)은 사이드카가 fetch 도 자동 예약도 하지 않는다(이중 fetch 방지).
// Rust 는 라우터가 켜져 있을 때만(ANTLER_RUST_GIT 이 0 이 아닐 때) 사이드카를 env ANTLER_RUST_FETCH_KINDS=local,wsl[,ssh] 로 띄운다 — ssh 는 ANTLER_RUST_SSH 가 켜져 SSH 풀이 있을 때만 들어간다
// (src-tauri/src/router.rs 의 rust_fetch_kinds). 값이 없으면 사이드카가 전부 맡는다(되돌리기).
import type { AppConfig } from '@shared/types'

/** Rust 가 맡을 수 있는 소스 종류. 그 밖의 토큰(github·오타)은 무시한다 — 알 수 없는 값이 사이드카의 fetch 를 꺼서 아무도 맡지 않는 일이 없게 */
const RUST_FETCH_KINDS = ['local', 'wsl', 'ssh'] as const
export type RustFetchKind = (typeof RUST_FETCH_KINDS)[number]

export function parseRustFetchKinds(value: string | undefined): ReadonlySet<RustFetchKind> {
  const out = new Set<RustFetchKind>()
  for (const token of (value ?? '').split(',')) {
    const t = token.trim()
    if ((RUST_FETCH_KINDS as readonly string[]).includes(t)) out.add(t as RustFetchKind)
  }
  return out
}

/** `FetchService` 의 `owns`. 소스 종류는 `mirrorOf` 와 같은 출처(저장소의 소스)다. 설정에 없는 저장소는 맡는다(fetch 가 알 수 없는 저장소 오류를 낸다) */
export function fetchOwner(getConfig: () => AppConfig, rustKinds: ReadonlySet<RustFetchKind>): (repoId: string) => boolean {
  if (rustKinds.size === 0) return () => true
  return (repoId) => {
    const config = getConfig()
    const repo = config.repos.find((r) => r.id === repoId)
    const source = repo && config.sources.find((s) => s.id === repo.sourceId)
    return !source || !(rustKinds as ReadonlySet<string>).has(source.kind)
  }
}
