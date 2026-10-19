import { useEffect, useMemo, useState } from 'react'
import { create } from 'zustand'
import { resolveGitflow } from '@shared/gitflow'
import type { GitflowConfig, GitflowInfo, RefsSnapshot } from '@shared/types'
import { api, newRequestId } from '../lib/api'
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
  // 배열이 새로 만들어져도 경로 목록이 같으면 다시 묻지 않는다
  const pathsKey = useMemo(() => files.map((f) => f.path).join('\0'), [files])
  const scope = `${repoId}\0${rev ?? ''}`
  const [found, setFound] = useState<{ scope: string; set: ReadonlySet<string> } | null>(null)
  useEffect(() => {
    if (!enabled || !repoId || pathsKey === '') return
    const key = rev ? `${scope}\0${pathsKey.length}\0${hashText(pathsKey)}` : ''
    const hit = key ? commitCache.get(key) : undefined
    if (hit) {
      setFound({ scope, set: hit })
      return
    }
    let done = false
    const requestId = newRequestId('lfs')
    api
      .invoke('git:lfsPaths', repoId, { ...(rev ? { rev } : {}), paths: pathsKey.split('\0') }, requestId)
      .then((r) => {
        done = true
        const set = new Set(r)
        if (key) {
          commitCache.set(key, set)
          if (commitCache.size > CACHE_MAX) commitCache.delete(commitCache.keys().next().value!)
        }
        // 같은 범위(저장소·커밋)에서 목록만 바뀐 경우 이전 결과를 유지하다 새 결과로 바꾼다
        setFound({ scope, set })
      })
      .catch(() => {
        done = true
      })
    return () => {
      // 화면 전환·목록 변경이면 진행 중인 조회를 취소한다
      if (!done) void api.invoke('req:cancel', requestId).catch(() => {})
    }
  }, [enabled, repoId, rev, scope, pathsKey])
  return enabled && found && found.scope === scope ? found.set : EMPTY
}

function hashText(s: string): number {
  let h = 5381
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0
  return h
}
