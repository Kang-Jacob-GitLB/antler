import { useEffect, useRef, useState } from 'react'
import { CommitList } from './components/CommitList'
import { DetailPanel } from './components/DetailPanel'
import { AddRepoDialog } from './components/AddRepoDialog'
import { DiffView } from './components/DiffView'
import { EmptyState } from './components/EmptyState'
import { RepoSwitcher } from './components/RepoSwitcher'
import { Sidebar } from './components/Sidebar'
import { Splitter, type SplitterHandlers } from './components/Splitter'
import { StatusBar } from './components/StatusBar'
import { Toolbar } from './components/Toolbar'
import { useAppStore } from './stores/appStore'
import { useDetailStore } from './stores/detailStore'
import { useLogStore } from './stores/logStore'
import { useFetchStore } from './stores/fetchStore'
import { useSearchStore } from './stores/searchStore'
import { refreshWip, useWipSync } from './stores/wipActions'
import { useGithubStore } from './stores/githubStore'
import { GitHubView } from './components/github/GitHubView'
import { UpdateSnackbar } from './components/UpdateSnackbar'
import { CommandPalette } from './components/CommandPalette'
import { ShortcutsDialog } from './components/ShortcutsDialog'
import { runShortcut } from './lib/commands'
import { isOverlayOpen } from './stores/paletteStore'
import { syncGroupColors } from './lib/groupColor'
import './App.css'

type Widths = [sidebar: number, detail: number]

const DEFAULT_WIDTHS: Widths = [248, 380]
const DETAIL_DEBOUNCE_MS = 60
const LIMITS: [min: number, max: number][] = [
  [180, 480],
  [280, 720]
]
const clamp = (v: number, [lo, hi]: [number, number]): number => Math.min(hi, Math.max(lo, v))

export function App(): React.JSX.Element | null {
  const config = useAppStore((s) => s.config)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const error = useAppStore((s) => s.error)
  const [widths, setWidths] = useState<Widths>(DEFAULT_WIDTHS)
  const widthsRef = useRef(widths)
  widthsRef.current = widths
  const dragBase = useRef(widths)
  const widthsLoaded = useRef(false)

  useEffect(() => {
    void useAppStore.getState().init()
    // 자동·수동 fetch 상태(진행 중·마지막 성공·오류·차단)를 main 이 알려 준다
    const offFetch = window.antler.on('fetch:status', (s) => useFetchStore.getState().set(s))
    // 외부(터미널·IDE)에서 커밋·체크아웃·fetch 하면 main 감시자가 알려 준다 — 선택은 유지한 채 다시 읽는다
    const offChanged = window.antler.on('repo:changed', ({ repoId }) => {
      if (repoId !== useAppStore.getState().activeRepoId) return
      void useAppStore.getState().refreshRefs()
      void useLogStore.getState().softReload()
      refreshWip()
    })
    return () => {
      offFetch()
      offChanged()
    }
  }, [])

  // 작업 트리(WIP) 변경 상태: 저장소 선택·창 포커스·주기 폴링으로 최신으로 유지한다
  useWipSync()

  // 저장된 패널 폭은 설정을 처음 받았을 때 한 번만 반영한다
  useEffect(() => {
    if (config && !widthsLoaded.current) {
      widthsLoaded.current = true
      setWidths(config.ui.paneWidths)
    }
  }, [config])

  // 저장소 그룹 칩 색: 새로 생긴 그룹에 겹치지 않는 색을 주고, 없어진 그룹의 색은 지운다
  const groupKey = config ? [...new Set(config.repos.map((r) => r.group).filter((g): g is string => !!g))].sort().join('\0') : null
  useEffect(() => {
    const cfg = useAppStore.getState().config
    if (!cfg) return
    const groups = [...new Set(cfg.repos.map((r) => r.group).filter((g): g is string => !!g))]
    const next = syncGroupColors(groups, cfg.ui.groupColors)
    if (next) void useAppStore.getState().setUi({ groupColors: next })
  }, [groupKey])

  // 활성 저장소가 바뀌면 커밋 목록을 새로 연다
  useEffect(() => {
    useSearchStore.getState().close()
    useDetailStore.getState().reset()
    void useGithubStore.getState().open(activeRepoId)
    useLogStore.getState().open(activeRepoId)
  }, [activeRepoId])

  // 선택한 커밋의 상세를 불러온다. ↑↓ 를 누르고 있을 때 요청이 쏟아지지 않도록 잠깐 기다린다
  const selectedHash = useLogStore((s) => (s.selected >= 0 ? s.commits[s.selected]?.hash : undefined))
  useEffect(() => {
    const repoId = useLogStore.getState().repoId
    const timer = setTimeout(() => void useDetailStore.getState().load(repoId, selectedHash ?? null), DETAIL_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [selectedHash])
  const diffOpen = useDetailStore((s) => s.file !== null)
  const sidebarShown = useAppStore((s) => s.config?.ui.sidebarHidden !== true)
  const detailShown = useAppStore((s) => s.config?.ui.detailHidden !== true)
  // 중앙 화면 우선순위: 파일 diff → GitHub 화면 → 커밋 그래프(그래프는 항상 마운트해 스크롤·페이지 유지)
  const ghOpen = useGithubStore((s) => s.view !== null)

  // 그래프 필터가 바뀌면 검색 결과도 그 범위로 다시 구한다(옛 범위의 일치 표시가 남지 않게)
  const filterRev = useLogStore((s) => s.filter?.rev)
  useEffect(() => {
    const search = useSearchStore.getState()
    if (search.open && search.ranQuery) void search.run()
  }, [filterRev])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      // 팔레트·단축키 목록·저장소 전환·저장소 추가가 열려 있으면 그쪽이 자기 키를 처리한다
      if (isOverlayOpen()) return
      const detail = useDetailStore.getState()
      // diff 보기: Esc 닫기, Alt+↑/↓ 이전/다음 파일 (팝업·검색 입력 중에는 그쪽이 Esc 를 쓴다)
      if (detail.file && !useAppStore.getState().switcherOpen) {
        const typing = (e.target as HTMLElement | null)?.tagName === 'INPUT'
        if (e.key === 'Escape' && !typing) {
          e.preventDefault()
          detail.closeFile()
          return
        }
        if (e.altKey && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
          e.preventDefault()
          detail.stepFile(e.key === 'ArrowDown' ? 1 : -1)
          return
        }
      }
      // GitHub 화면: Esc 로 상세 → 목록 → 그래프 순으로 물러난다
      const gh = useGithubStore.getState()
      if (gh.view && !detail.file && e.key === 'Escape' && !useAppStore.getState().switcherOpen) {
        const tag = (e.target as HTMLElement | null)?.tagName
        if (tag !== 'INPUT' && tag !== 'SELECT') {
          e.preventDefault()
          gh.show(gh.view.kind === 'pull' ? { kind: 'pulls' } : gh.view.kind === 'issue' ? { kind: 'issues' } : null)
          return
        }
      }
      // 명령 레지스트리에 단축키가 정의된 명령(Ctrl+B/Ctrl+Alt+B 패널, Ctrl+P 저장소 전환, Ctrl+F 검색,
      // Ctrl+Shift+P 팔레트, Ctrl+/ 단축키 목록)은 팔레트와 같은 함수를 실행한다. 입력 중에도 동작한다
      if (runShortcut(e)) e.preventDefault()
    }
    // 마우스 뒤로가기 버튼(X1, button 3): diff 를 보고 있으면 커밋 목록으로 돌아간다
    const onMouseBack = (e: MouseEvent): void => {
      if (e.button !== 3 || isOverlayOpen()) return
      const detail = useDetailStore.getState()
      if (!detail.file) return
      e.preventDefault()
      detail.closeFile()
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('mouseup', onMouseBack)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('mouseup', onMouseBack)
    }
  }, [])

  if (!config) return null

  const persist = (w: Widths): void => void useAppStore.getState().setUi({ paneWidths: w })

  /** side 0 = 사이드바(오른쪽으로 끌면 넓어짐), 1 = 상세 패널(왼쪽으로 끌면 넓어짐) */
  const splitter = (side: 0 | 1): SplitterHandlers => ({
    onDragStart: () => {
      dragBase.current = widthsRef.current
    },
    onDrag: (dx) => {
      const next: Widths = [...widthsRef.current]
      next[side] = clamp(dragBase.current[side] + (side === 0 ? dx : -dx), LIMITS[side]!)
      setWidths(next)
    },
    onDragEnd: () => persist(widthsRef.current),
    onReset: () => {
      const next: Widths = [...widthsRef.current]
      next[side] = DEFAULT_WIDTHS[side]
      setWidths(next)
      persist(next)
    }
  })

  return (
    <div className="app">
      <Toolbar />
      <div className="app__body">
        {sidebarShown && (
          <>
            <Sidebar width={widths[0]} />
            <Splitter {...splitter(0)} />
          </>
        )}
        {activeRepoId ? (
          <>
            {/* diff 를 여는 동안에도 그래프는 마운트해 둔다(스크롤 위치·불러온 페이지 유지) */}
            <div className={`app__center${diffOpen || ghOpen ? ' is-hidden' : ''}`}>
              <CommitList />
            </div>
            {diffOpen ? <DiffView /> : ghOpen ? <GitHubView /> : null}
            {detailShown && (
              <>
                <Splitter {...splitter(1)} />
                <DetailPanel width={widths[1]} />
              </>
            )}
          </>
        ) : (
          <EmptyState />
        )}
      </div>
      <StatusBar />
      {error && (
        <div className="toast" role="alert" onClick={() => useAppStore.getState().clearError()}>
          {error}
        </div>
      )}
      <RepoSwitcher />
      <CommandPalette />
      <ShortcutsDialog />
      <AddRepoDialog />
      <UpdateSnackbar />
    </div>
  )
}
