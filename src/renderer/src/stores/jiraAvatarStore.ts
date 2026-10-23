import { useEffect } from 'react'
import { create } from 'zustand'
import { api } from '../lib/api'
import { useJiraStore } from './jiraStore'

/** 기억해 두는 사진 수(넘으면 오래 받은 것부터 버린다) */
const MAX_ENTRIES = 300

interface JiraAvatarState {
  /** Jira avatarUrls 주소 → data URL. null=받을 수 없음. 키가 없으면 아직 묻지 않음 */
  urls: Record<string, string | null>
}

export const useJiraAvatarStore = create<JiraAvatarState>(() => ({ urls: {} }))

/** 받지 못한 사진을 다시 묻기까지의 간격(main 의 실패 캐시와 같다 — 일시적인 실패가 끝까지 이니셜로 남지 않게) */
const RETRY_MS = 10 * 60_000

const inflight = new Set<string>()
/** 받지 못한 주소 → 실패한 시각 */
const failedAt = new Map<string, number>()
/** 연결이 바뀔 때마다 증가 — 옛 연결에서 묻던 응답을 버린다 */
let gen = 0

/** 사진을 main 에 한 번만 묻는다(인증이 필요한 사진도 main 이 받아 data URL 로 넘긴다). 실패한 주소는 RETRY_MS 뒤에 다시 묻는다 */
function request(url: string): void {
  const known = useJiraAvatarStore.getState().urls[url]
  if (inflight.has(url) || known || (known === null && Date.now() - (failedAt.get(url) ?? 0) < RETRY_MS)) return
  inflight.add(url)
  const g = gen
  void api
    .invoke('jira:avatar', url)
    .catch(() => null)
    .then((data) => {
      if (g !== gen) return
      inflight.delete(url)
      if (data) failedAt.delete(url)
      else failedAt.set(url, Date.now())
      useJiraAvatarStore.setState((s) => {
        // 다시 넣어 순서를 최근으로 옮긴다(오래 받은 것부터 버린다)
        const urls = { ...s.urls }
        delete urls[url]
        urls[url] = data
        const keys = Object.keys(urls)
        for (const k of keys.slice(0, Math.max(0, keys.length - MAX_ENTRIES))) {
          delete urls[k]
          failedAt.delete(k)
        }
        return { urls }
      })
    })
}

/** React 에서: 이 사진 주소의 data URL(없거나 아직 받는 중이면 null — 호출자는 이니셜로 대신한다) */
export function useJiraAvatar(url: string | undefined): string | null {
  const data = useJiraAvatarStore((s) => (url ? s.urls[url] : undefined))
  // 상한으로 버려져 다시 모르게 된 사진(화면에 떠 있는 중이라도)도 다시 묻는다
  const unknown = data === undefined
  useEffect(() => {
    if (url) request(url)
  }, [url, unknown])
  return data ?? null
}

// 연결이 바뀌거나 끊기면 비운다(다른 계정·사이트로 받은 사진·실패를 다시 쓰지 않게)
let connKey = ''
useJiraStore.subscribe((s) => {
  const key = s.auth.connected ? `${s.auth.kind}|${s.auth.siteUrl}|${s.auth.email ?? ''}` : ''
  if (key === connKey) return
  connKey = key
  gen++
  inflight.clear()
  failedAt.clear()
  useJiraAvatarStore.setState({ urls: {} })
})
