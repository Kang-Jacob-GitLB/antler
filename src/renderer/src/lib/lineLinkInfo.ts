// 줄 링크에 필요한 저장소 정보(GitHub 원격 · push 여부)를 main 에 묻고 캐시한다
import type { GhRepoRef } from '@shared/types'
import { api } from './api'

const ghCache = new Map<string, Promise<GhRepoRef | null>>()
const pushed = new Set<string>()

/** 저장소의 GitHub origin(없으면 null). 저장소마다 한 번만 묻는다(실패하면 다음에 다시 묻는다) */
export function githubRepoOf(repoId: string): Promise<GhRepoRef | null> {
  let p = ghCache.get(repoId)
  if (!p) {
    p = api.invoke('gh:repo', repoId).catch(() => {
      ghCache.delete(repoId)
      return null
    })
    ghCache.set(repoId, p)
  }
  return p
}

/** 커밋이 원격에 있는지. true 만 캐시한다(fetch·push 뒤 false → true 로 바뀔 수 있다). 확인 실패는 true(경고하지 않음) */
export async function isCommitOnRemote(repoId: string, rev: string): Promise<boolean> {
  const key = `${repoId}:${rev}`
  if (pushed.has(key)) return true
  try {
    const ok = await api.invoke('git:onRemote', repoId, rev)
    if (ok) pushed.add(key)
    return ok
  } catch {
    return true
  }
}
