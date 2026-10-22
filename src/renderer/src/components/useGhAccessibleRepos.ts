import { useCallback, useEffect, useRef, useState } from 'react'
import type { GhAccessibleReposResult } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useGithubStore } from '../stores/githubStore'

export interface GhAccessibleState {
  result: GhAccessibleReposResult | null
  loading: boolean
  error: string | null
}

const INITIAL: GhAccessibleState = { result: null, loading: false, error: null }

/**
 * 로그인한 사용자가 접근할 수 있는 GitHub 저장소 목록(gh:accessibleRepos)을 받는다.
 * - open: 저장소 추가 창이 열려 있는지. 창을 열 때마다 새 세션이라 이전 응답은 버린다(닫으면 진행 중 요청을 취소한다)
 * - enabled: GitHub 탭이 보이는지. 처음 보일 때 한 번 받는다
 * - load(true): 새로고침(main 의 캐시를 무시)
 */
export function useGhAccessibleRepos(open: boolean, enabled: boolean): GhAccessibleState & { load(fresh: boolean): void } {
  const [state, setState] = useState<GhAccessibleState>(INITIAL)
  const reqId = useRef<string | null>(null)
  const seq = useRef(0)
  const epoch = useRef(0)
  const loadedEpoch = useRef(-1)
  const signedIn = useGithubStore((s) => s.auth.signedIn)

  const cancel = useCallback((): void => {
    if (reqId.current) void api.invoke('req:cancel', reqId.current).catch(() => {})
    reqId.current = null
  }, [])

  const load = useCallback(
    (fresh: boolean): void => {
      cancel()
      const my = ++seq.current
      const id = newRequestId('gh-repos')
      reqId.current = id
      setState((s) => ({ ...s, loading: true, error: null }))
      api.invoke('gh:accessibleRepos', id, fresh).then(
        (result) => {
          if (my !== seq.current) return
          reqId.current = null
          setState({ result, loading: false, error: null })
        },
        (err) => {
          if (my !== seq.current || isAbort(err)) return
          reqId.current = null
          setState((s) => ({ ...s, loading: false, error: errorMessage(err) }))
        }
      )
    },
    [cancel]
  )

  // 열 때마다 새 세션: 이전 결과·응답을 버린다. 닫으면 진행 중 요청을 취소한다
  useEffect(() => {
    seq.current++
    if (!open) {
      cancel()
      return
    }
    epoch.current++
    setState(INITIAL)
    return cancel
  }, [open, cancel])

  // GitHub 탭이 처음 보일 때(세션마다 한 번) 받는다
  useEffect(() => {
    if (!open || !enabled || loadedEpoch.current === epoch.current) return
    loadedEpoch.current = epoch.current
    load(false)
  }, [open, enabled, load])

  // 로그인이 필요했다가 로그인하면 바로 다시 받는다
  const needsLogin = state.result?.needsLogin === true
  useEffect(() => {
    if (open && enabled && needsLogin && signedIn) load(true)
  }, [open, enabled, needsLogin, signedIn, load])

  return { ...state, load }
}
