import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { resolveGitflow } from '@shared/gitflow'
import type { GitflowConfig, GitflowInfo, RefsSnapshot } from '@shared/types'
import { api } from '../lib/api'
import { branchKey } from '../lib/laneColor'
import { useAppStore } from './appStore'

/** 활성 저장소의 LFS 사용 여부·gitflow 설정(읽기 전용 표시용). 저장소가 바뀌거나 HEAD 가 움직이면 다시 읽는다 */
interface ExtrasState {
  repoId: string | null
  lfs: boolean
  gitflow: GitflowConfig | null
}

export const useRepoExtrasStore = create<ExtrasState>(() => ({ repoId: null, lfs: false, gitflow: null }))

let loadedKey = ''
let seq = 0

async function load(repoId: string): Promise<void> {
  const mine = ++seq
  try {
    const x = await api.invoke('git:repoExtras', repoId)
    if (mine === seq) useRepoExtrasStore.setState({ repoId, lfs: x.lfs, gitflow: x.gitflow })
  } catch {
    // 부가 표시라 실패하면 없는 것으로 둔다
    if (mine === seq) useRepoExtrasStore.setState({ repoId, lfs: false, gitflow: null })
  }
}

useAppStore.subscribe((s) => {
  const id = s.activeRepoId
  if (!id || !s.refs) {
    if (loadedKey !== '') {
      loadedKey = ''
      seq++
      useRepoExtrasStore.setState({ repoId: null, lfs: false, gitflow: null })
    }
    return
  }
  const key = `${id}\0${s.refs.head.hash ?? ''}`
  if (key === loadedKey) return
  loadedKey = key
  void load(id)
})

/** 지금 저장소의 Gitflow 정보(설정 또는 기본 접두사 사용 감지). 아니면 null */
export function useGitflow(): GitflowInfo | null {
  const repoId = useAppStore((s) => s.activeRepoId)
  const refs = useAppStore((s) => s.refs)
  const extras = useRepoExtrasStore((s) => (s.repoId === repoId ? s.gitflow : null))
  return flowFor(refs, extras)
}

// 커밋 목록의 칩마다 호출되므로 마지막 입력에 대한 결과를 공유한다
let lastFlow: { refs: RefsSnapshot | null; extras: GitflowConfig | null; value: GitflowInfo | null } | null = null
function flowFor(refs: RefsSnapshot | null, extras: GitflowConfig | null): GitflowInfo | null {
  if (lastFlow && lastFlow.refs === refs && lastFlow.extras === extras) return lastFlow.value
  let value: GitflowInfo | null = null
  if (refs) {
    const names = [...refs.local.map((b) => b.name), ...refs.remotes.flatMap((r) => r.branches.map((b) => branchKey(b.name, true)))]
    value = resolveGitflow(extras, names)
  }
  lastFlow = { refs, extras, value }
  return value
}

export function useRepoUsesLfs(): boolean {
  const repoId = useAppStore((s) => s.activeRepoId)
  return useRepoExtrasStore((s) => s.repoId === repoId && s.lfs)
}

const EMPTY: ReadonlySet<string> = new Set()
const commitCache = new Map<string, ReadonlySet<string>>()
const CACHE_MAX = 60

/**
 * 파일 목록 중 LFS 로 추적되는 경로(filter=lfs 속성). rev 가 null 이면 작업 트리(WIP) 기준.
 * 저장소가 LFS 를 쓰지 않으면 git 을 부르지 않는다.
 */
export function useLfsPaths(repoId: string | null, rev: string | null, files: readonly { path: string }[]): ReadonlySet<string> {
  const enabled = useRepoUsesLfs()
  const [found, setFound] = useState<{ files: readonly { path: string }[]; rev: string | null; set: ReadonlySet<string> } | null>(null)
  useEffect(() => {
    if (!enabled || !repoId || files.length === 0) return
    const key = rev ? `${repoId}\0${rev}` : ''
    const hit = key ? commitCache.get(key) : undefined
    if (hit) {
      setFound({ files, rev, set: hit })
      return
    }
    let cancelled = false
    const paths = files.map((f) => f.path)
    api
      .invoke('git:lfsPaths', repoId, { ...(rev ? { rev } : {}), paths })
      .then((r) => {
        if (cancelled) return
        const set = new Set(r)
        if (key) {
          commitCache.set(key, set)
          if (commitCache.size > CACHE_MAX) commitCache.delete(commitCache.keys().next().value!)
        }
        setFound({ files, rev, set })
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [enabled, repoId, rev, files])
  return enabled && found && found.files === files && found.rev === rev ? found.set : EMPTY
}
