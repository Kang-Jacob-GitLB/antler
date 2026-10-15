import { useEffect } from 'react'
import { useAppStore } from './appStore'
import { useDetailStore } from './detailStore'
import { useLogStore } from './logStore'
import { useWipStore, wipDirty, wipEntries } from './wipStore'

/** 상태 폴링 주기(ms). 원격(WSL·SSH)은 git 호출이 비싸 더 느리게 */
const POLL_LOCAL_MS = 5_000
const POLL_REMOTE_MS = 15_000

/**
 * 목록에서 WIP 행을 고르거나(on) 커밋을 골라 WIP 를 벗어난다(off).
 * 중앙의 diff 는 어느 쪽이든 이전 선택의 것이라 닫는다.
 */
export function selectWip(on: boolean): void {
  const wip = useWipStore.getState()
  if (wip.selected === on) return
  const detail = useDetailStore.getState()
  // WIP 를 고르면 열려 있던 커밋 diff 를, WIP 를 벗어나면 열려 있던 WIP diff 를 닫는다
  if (on || detail.wipArea) detail.closeFile()
  wip.select(on)
}

/**
 * 활성 저장소의 작업 트리 상태를 최신으로 유지한다(App 에서 한 번 호출).
 * 메인의 RepoWatcher 는 ref 변화만 알리고 파일 저장은 알리지 않으므로, 다음 때 상태를 다시 읽는다:
 * 저장소 선택 · 창 포커스 · repo:changed(App 이 refreshWip 호출) · 포커스된 동안 주기 폴링.
 * 읽는 중이면 건너뛰므로(wipStore.refresh) git 프로세스가 쌓이지 않는다.
 */
export function useWipSync(): void {
  const repoId = useAppStore((s) => s.activeRepoId)
  const kind = useAppStore((s) => {
    const repo = s.config?.repos.find((r) => r.id === s.activeRepoId)
    return s.config?.sources.find((x) => x.id === repo?.sourceId)?.kind ?? 'local'
  })

  useEffect(() => {
    const wip = useWipStore.getState()
    wip.reset(repoId)
    if (!repoId) return
    const every = kind === 'local' ? POLL_LOCAL_MS : POLL_REMOTE_MS
    let timer: ReturnType<typeof setTimeout> | undefined
    let stopped = false
    const tick = async (): Promise<void> => {
      // 창이 가려져 있으면 읽지 않는다(돌아오면 focus 이벤트로 바로 읽는다)
      if (document.hasFocus()) await useWipStore.getState().refresh()
      if (!stopped) timer = setTimeout(() => void tick(), every)
    }
    void wip.refresh()
    timer = setTimeout(() => void tick(), every)
    const onFocus = (): void => void useWipStore.getState().refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      stopped = true
      clearTimeout(timer)
      window.removeEventListener('focus', onFocus)
    }
  }, [repoId, kind])

  useEffect(() => {
    // 목록에서 커밋 선택이 바뀌면(클릭·사이드바 이동·검색) WIP 선택을 푼다
    const unLog = useLogStore.subscribe((s, prev) => {
      if (s.selected !== prev.selected || s.scrollRequest !== prev.scrollRequest) selectWip(false)
    })
    // 상태가 바뀌면: 변경이 모두 사라졌으면 선택을 풀고, 보던 파일이 사라졌으면 diff 를 닫는다
    const unWip = useWipStore.subscribe((s, prev) => {
      if (s.status === prev.status) return
      if (s.selected && !wipDirty(s.status)) selectWip(false)
      const d = useDetailStore.getState()
      if (!d.wipArea || !d.file) return
      const cur = d.file
      const area = d.wipArea
      // 열린 WIP diff 는 연 시점의 스냅샷이다(다시 누르면 갱신). 보던 파일이 목록에서 사라졌을 때만 닫는다
      if (!wipEntries(s.status).some((x) => x.area === area && x.file.path === cur.path)) d.closeFile()
    })
    return () => {
      unLog()
      unWip()
    }
  }, [])
}

/** 외부 변경 알림(repo:changed) 때 상태를 다시 읽는다 */
export function refreshWip(): void {
  void useWipStore.getState().refresh()
}
