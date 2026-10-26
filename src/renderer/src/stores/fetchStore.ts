import { create } from 'zustand'
import type { FetchStatus } from '@shared/types'
import { api, errorMessage } from '../lib/api'
import { useAppStore } from './appStore'
import { useGithubStore } from './githubStore'
import { useLogStore } from './logStore'

interface FetchState {
  /** 저장소별 fetch 상태(main 이 자동·수동 fetch 마다 밀어준다) */
  status: Record<string, FetchStatus>
  set(status: FetchStatus): void
  /** 새로고침 버튼: 활성 저장소를 fetch 한 뒤 ref·커밋 목록을 다시 읽는다 */
  refresh(): Promise<void>
}

export const useFetchStore = create<FetchState>((set) => ({
  status: {},

  set(status) {
    const was = useFetchStore.getState().status[status.repoId]
    set((s) => ({ status: { ...s.status, [status.repoId]: status } }))
    // fetch 가 막 끝났으면 GitHub PR·이슈 목록도 다시 받는다 — 새로 열리거나 바뀐 PR 이 저장소를 다시 열지 않아도 보이게
    if (was?.fetching && !status.fetching) useGithubStore.getState().refreshLists(status.repoId)
  },

  async refresh() {
    const app = useAppStore.getState()
    const id = app.activeRepoId
    if (!id) return
    try {
      // 진행 중 상태는 main 이 'fetch:status' 로 알려 준다. 막힘·실패도 상태로 돌아오므로 예외는 통신 오류뿐이다
      useFetchStore.getState().set(await api.invoke('git:fetch', id, true))
    } catch (err) {
      useAppStore.setState({ error: errorMessage(err) })
    }
    // fetch 가 실패하거나 막혀도 로컬 상태는 새로 읽는다
    if (useAppStore.getState().activeRepoId !== id) return
    useLogStore.getState().reload()
    void useAppStore.getState().refreshRefs()
  }
}))
