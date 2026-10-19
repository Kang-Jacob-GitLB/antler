import { useEffect } from 'react'
import { create } from 'zustand'
import type { GhCheckState } from '@shared/types'
import { checkKey, isFresh, scopeOf, ttlOf } from '@shared/checkTtl'
import { api, isAbort } from '../lib/api'

export { checkKey, isFresh, scopeOf }

/** 화면 범위가 멈춘 뒤 이만큼 기다렸다가 묻는다(스크롤 중에는 묻지 않는다) */
const DEBOUNCE_MS = 300
/** 한 번에 묻는 커밋 수(main 의 GraphQL 한 번 분량) */
const BATCH = 50
/** 실패(한도·네트워크)한 sha 를 다시 묻기까지의 간격 */
const RETRY_MS = 60_000
/** 진행 중으로 계속 남는 상태(EXPECTED 등)를 이 횟수까지만 다시 묻는다 */
const PENDING_RECHECK_MAX = 20
/** 저장된 항목 수가 이를 넘으면 현재 저장소 것만 남긴다 */
const MAX_ENTRIES = 3000

interface Entry {
  state: GhCheckState
  /** 받은 시각(epoch ms) */
  at: number
}

interface ChecksState {
  /** `scope|sha` → 상태. 키가 없으면 아직 모름 */
  states: Record<string, Entry>
  /** 로그인하지 않아 조회하지 않는 중이면 false — 커밋 목록은 아이콘을 그리지 않는다 */
  available: boolean
}

export const useChecksStore = create<ChecksState>(() => ({ states: {}, available: true }))

let timer: ReturnType<typeof setTimeout> | undefined
let recheck: ReturnType<typeof setTimeout> | undefined
let visible: { repoId: string; scope: string; shas: string[] } | null = null
const inflight = new Set<string>()
const retryAt = new Map<string, number>()
/** 진행 중 상태를 다시 물은 횟수 */
const pendingRounds = new Map<string, number>()
/** 로그인·로그아웃마다 증가 — 그 전에 보낸 요청의 늦은 응답을 버린다 */
let gen = 0

/** 로그인·로그아웃 뒤: 상태를 비우고 보이는 범위를 다시 묻게 한다 */
export function resetChecksSession(): void {
  clearTimeout(timer)
  clearTimeout(recheck)
  recheck = undefined
  gen++
  inflight.clear()
  retryAt.clear()
  pendingRounds.clear()
  useChecksStore.setState({ states: {}, available: true })
  if (visible) requestChecks(visible.repoId, visible.scope, visible.shas)
}

/** 화면에서 벗어났다(다른 저장소·GitHub 아님·목록 사라짐) — 예약된 재확인을 멈춘다 */
function stopVisible(): void {
  visible = null
  clearTimeout(timer)
  clearTimeout(recheck)
  recheck = undefined
}

/** 화면 범위의 커밋 중 모르거나 수명이 지난 것을 묻는다(마지막 호출만 debounce 뒤에 실행) */
export function requestChecks(repoId: string | null, scope: string | null, shas: string[]): void {
  if (!repoId || !scope) return stopVisible()
  if (visible && visible.scope !== scope) prune(scope)
  visible = { repoId, scope, shas }
  if (!useChecksStore.getState().available) return
  const { states } = useChecksStore.getState()
  const now = Date.now()
  const list = shas.filter((s) => {
    const k = checkKey(scope, s)
    return !isFresh(states[k], now) && !inflight.has(k) && (retryAt.get(k) ?? 0) <= now
  })
  if (list.length > 0) {
    clearTimeout(timer)
    timer = setTimeout(() => flush(repoId, scope, list), DEBOUNCE_MS)
  }
  scheduleRecheck()
}

/** 다른 저장소로 옮겨 가면 이전 저장소의 항목을 버린다(전환 직후에도 새 저장소 것은 유지) */
function prune(keepScope: string): void {
  const next: Record<string, Entry> = {}
  const prefix = `${keepScope}|`
  for (const [k, v] of Object.entries(useChecksStore.getState().states)) if (k.startsWith(prefix)) next[k] = v
  useChecksStore.setState({ states: next })
  for (const k of [...retryAt.keys()]) if (!k.startsWith(prefix)) retryAt.delete(k)
  for (const k of [...pendingRounds.keys()]) if (!k.startsWith(prefix)) pendingRounds.delete(k)
}

/** 모든 커밋을 BATCH 개씩 나눠 보낸다 — 화면에 BATCH 개보다 많이 보여도 전부 묻는다 */
function flush(repoId: string, scope: string, shas: string[]): void {
  if (visible?.scope !== scope) return
  const g = gen
  const todo = shas.filter((s) => !inflight.has(checkKey(scope, s)))
  for (let i = 0; i < todo.length; i += BATCH) {
    const chunk = todo.slice(i, i + BATCH)
    for (const s of chunk) inflight.add(checkKey(scope, s))
    api.invoke('gh:commitChecks', repoId, chunk).then(
      (res) => {
        if (g !== gen) return
        for (const s of chunk) inflight.delete(checkKey(scope, s))
        // 그사이 저장소를 옮겼으면 버린다
        if (visible?.scope !== scope) return
        if (!res.available) {
          useChecksStore.setState({ available: false })
          return
        }
        const at = Date.now()
        const next: Record<string, Entry> = {}
        for (const s of chunk) {
          const state = res.states[s]
          const k = checkKey(scope, s)
          if (!state) {
            retryAt.set(k, at + RETRY_MS)
            continue
          }
          next[k] = { state, at }
          pendingRounds.set(k, state === 'pending' ? (pendingRounds.get(k) ?? 0) + 1 : 0)
        }
        if (Object.keys(next).length) {
          useChecksStore.setState((st) => {
            const merged = { ...st.states, ...next }
            if (Object.keys(merged).length > MAX_ENTRIES) {
              const keep: Record<string, Entry> = {}
              for (const [k, v] of Object.entries(merged)) if (k.startsWith(`${scope}|`)) keep[k] = v
              return { states: keep }
            }
            return { states: merged }
          })
        }
        scheduleRecheck()
      },
      (err) => {
        if (g !== gen) return
        for (const s of chunk) {
          const k = checkKey(scope, s)
          inflight.delete(k)
          // 한도·권한 문제는 조용히 넘기고 잠시 뒤에 다시 시도한다
          if (!isAbort(err)) retryAt.set(k, Date.now() + RETRY_MS)
        }
      }
    )
  }
}

/** 보이는 커밋 중 가장 먼저 수명이 끝나는 항목 시점에 다시 묻는다(다시 물을 때는 기존 아이콘을 유지한 채 갱신한다) */
function scheduleRecheck(): void {
  clearTimeout(recheck)
  recheck = undefined
  if (!visible || !useChecksStore.getState().available) return
  const { repoId, scope, shas } = visible
  const { states } = useChecksStore.getState()
  const now = Date.now()
  let wait = Infinity
  for (const s of shas) {
    const k = checkKey(scope, s)
    const e = states[k]
    if (!e) continue
    // 진행 중이 계속 남는 상태는 일정 횟수까지만 다시 묻는다
    if (e.state === 'pending' && (pendingRounds.get(k) ?? 0) >= PENDING_RECHECK_MAX) continue
    wait = Math.min(wait, Math.max(0, e.at + ttlOf(e.state) - now))
  }
  if (!Number.isFinite(wait)) return
  recheck = setTimeout(() => {
    recheck = undefined
    if (visible && visible.scope === scope) requestChecks(repoId, scope, shas)
  }, wait + 50)
}

/** 커밋 목록: 화면에 보이는 범위의 커밋에 대해 체크 상태를 요청한다. 목록이 사라지거나 저장소를 옮기면 재확인을 멈춘다 */
export function useVisibleChecks(repoId: string | null, scope: string | null, enabled: boolean, commits: readonly { hash: string }[], first: number, last: number): void {
  useEffect(() => {
    if (!repoId || !scope || !enabled) return stopVisible()
    const shas: string[] = []
    for (let i = Math.max(0, first); i <= last && i < commits.length; i++) shas.push(commits[i]!.hash.toLowerCase())
    requestChecks(repoId, scope, shas)
  }, [repoId, scope, enabled, commits, first, last])
  // 언마운트 때만 멈춘다(의존값이 바뀔 때마다 멈추면 예약이 매번 끊긴다)
  useEffect(() => stopVisible, [])
}
