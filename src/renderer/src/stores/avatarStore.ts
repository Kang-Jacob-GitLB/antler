import { create } from 'zustand'
import type { GhAvatarBlocked, GhAvatarQuery } from '@shared/types'
import { avatarBaseUrl, parseNoreply } from '@shared/avatar'
import { api, isAbort, newRequestId } from '../lib/api'

/** 화면 범위가 멈춘 뒤 이만큼 기다렸다가 묻는다(스크롤 중에는 묻지 않는다) */
const DEBOUNCE_MS = 250
/** 한 번에 묻는 작성자 수와, 못 정했을 때(한도·미푸시 등) 같은 작성자를 다시 묻기까지의 간격 */
const MAX_PER_REQUEST = 60
const RETRY_MS = 60_000

interface AvatarState {
  /** 소문자 이메일 → 크기 없는 아바타 주소. null=GitHub 계정과 연결되지 않음. 키가 없으면 아직 모름 */
  urls: Record<string, string | null>
  /** 사진 조회가 막혀 있는 이유(막혀 있지 않으면 null) — 작성자 열 머리의 안내 점이 쓴다 */
  blocked: (GhAvatarBlocked & { repoId: string }) | null
  /** 로그인·로그아웃으로 조건이 바뀔 때마다 증가 — 목록이 다시 묻도록 하는 신호 */
  epoch: number
}

export const useAvatarStore = create<AvatarState>(() => ({ urls: {}, blocked: null, epoch: 0 }))

/** noreply 이메일은 이메일만으로 풀린다(결과를 캐시해 프레임마다 정규식을 돌리지 않는다) */
const noreplyCache = new Map<string, string | null>()
function noreplyBase(email: string): string | null {
  let v = noreplyCache.get(email)
  if (v === undefined) {
    if (noreplyCache.size > 5000) noreplyCache.clear()
    const n = parseNoreply(email)
    noreplyCache.set(email, (v = n ? avatarBaseUrl(n.login, n.id) : null))
  }
  return v
}

/** 이메일의 아바타 기본 주소. 없거나(미해석 포함) 계정이 없으면 null — 호출자는 이니셜로 대신한다 */
export function avatarBaseOf(email: string, urls: Record<string, string | null> = useAvatarStore.getState().urls): string | null {
  return noreplyBase(email) ?? urls[email.toLowerCase()] ?? null
}

/** React 에서: 이 이메일의 아바타 기본 주소(바뀌면 다시 그린다) */
export function useAvatarBase(email: string): string | null {
  return useAvatarStore((s) => avatarBaseOf(email, s.urls))
}

// ── 해석 요청(화면에 보이는 커밋만, debounce) ────────────

let timer: ReturnType<typeof setTimeout> | undefined
let wanted: { repoId: string; list: GhAvatarQuery[] } | null = null
let activeRepo: string | null = null
const retryAt = new Map<string, number>()
/** GitHub remote 가 없다고 확인된 저장소 — 더 묻지 않는다 */
const noGithub = new Set<string>()
const inflight = new Map<string, string>()

/**
 * 로그인·로그아웃 뒤: "나중에 다시"·"GitHub 저장소 아님" 표시와 진행 중 표시를 비우고 안내를 지운 뒤, epoch 를 올려 보이는 작성자를 다시 묻게 한다.
 * (확인된 사진 주소는 계정이 같으니 남긴다.) main 이 중단한 요청의 늦은 응답은 inflight 가 비어 있어 무시된다.
 */
export function resetAvatarSession(): void {
  clearTimeout(timer)
  wanted = null
  inflight.clear()
  retryAt.clear()
  noGithub.clear()
  useAvatarStore.setState((s) => ({ blocked: null, epoch: s.epoch + 1 }))
}

/** 지금 알 수 없는 작성자만 골라 (이메일, sha) 목록을 만든다. 같은 작성자의 가장 오래된(=미푸시일 가능성이 낮은) 커밋을 쓴다 */
export function pendingAuthors(commits: readonly { hash: string; authorEmail: string }[], first: number, last: number): GhAvatarQuery[] {
  const urls = useAvatarStore.getState().urls
  const now = Date.now()
  const byEmail = new Map<string, GhAvatarQuery>()
  for (let i = Math.max(0, first); i <= last && i < commits.length; i++) {
    const c = commits[i]!
    const email = c.authorEmail
    if (!email || email.length > 254) continue
    const key = email.toLowerCase()
    if (noreplyBase(email) || key in urls || inflight.has(key) || (retryAt.get(key) ?? 0) > now) continue
    // 목록은 최신순이라 뒤에 나온 것이 더 오래된 커밋이다
    byEmail.set(key, { email, sha: c.hash })
  }
  return [...byEmail.values()].slice(0, MAX_PER_REQUEST)
}

/** 화면 범위의 미해석 작성자를 묻는다(마지막 호출만 debounce 뒤에 실행) */
export function requestAvatars(repoId: string | null, list: GhAvatarQuery[]): void {
  if (!repoId || list.length === 0 || noGithub.has(repoId)) return
  wanted = { repoId, list }
  clearTimeout(timer)
  timer = setTimeout(flush, DEBOUNCE_MS)
}

function flush(): void {
  const w = wanted
  wanted = null
  if (!w || noGithub.has(w.repoId)) return
  // 저장소가 바뀌면 이전 저장소의 진행 중 요청은 더 필요 없다
  if (activeRepo && activeRepo !== w.repoId) {
    for (const id of new Set(inflight.values())) void api.invoke('req:cancel', id).catch(() => {})
    inflight.clear()
  }
  activeRepo = w.repoId
  const list = w.list.filter((a) => !inflight.has(a.email.toLowerCase()))
  if (list.length === 0) return
  const requestId = newRequestId('gh-avatars')
  for (const a of list) inflight.set(a.email.toLowerCase(), requestId)

  api.invoke('gh:avatars', w.repoId, requestId, list).then(
    (res) => {
      const next: Record<string, string | null> = {}
      for (const a of list) {
        const key = a.email.toLowerCase()
        if (inflight.get(key) !== requestId) continue
        inflight.delete(key)
        const hit = res.avatars[a.email]
        if (hit === undefined) retryAt.set(key, Date.now() + RETRY_MS)
        else next[key] = hit ? hit.url : null
      }
      if (!res.github && !Object.keys(res.avatars).length) noGithub.add(w.repoId)
      // 이 저장소가 지금 보는 저장소일 때만 안내를 갱신한다(응답이 모두 무시된 경우는 위에서 이미 걸러진다)
      const blocked = res.blocked ? { ...res.blocked, repoId: w.repoId } : null
      if (Object.keys(next).length || blocked?.reason !== useAvatarStore.getState().blocked?.reason || blocked?.resetAt !== useAvatarStore.getState().blocked?.resetAt || blocked?.repoId !== useAvatarStore.getState().blocked?.repoId) {
        useAvatarStore.setState((s) => ({ urls: { ...s.urls, ...next }, blocked }))
      }
    },
    (err) => {
      for (const a of list) {
        const key = a.email.toLowerCase()
        if (inflight.get(key) !== requestId) continue
        inflight.delete(key)
        if (!isAbort(err)) retryAt.set(key, Date.now() + RETRY_MS)
      }
    }
  )
}
