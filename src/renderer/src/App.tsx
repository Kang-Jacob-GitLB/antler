import { useEffect, useRef, useState } from 'react'
import { CommitList } from './components/CommitList'
import { DetailPanel } from './components/DetailPanel'
import { EmptyState } from './components/EmptyState'
import { RepoSwitcher } from './components/RepoSwitcher'
import { Sidebar } from './components/Sidebar'
import { Splitter, type SplitterHandlers } from './components/Splitter'
import { Toolbar } from './components/Toolbar'
import { useAppStore } from './stores/appStore'
import { useLogStore } from './stores/logStore'
import { useSearchStore } from './stores/searchStore'
import './App.css'

type Widths = [sidebar: number, detail: number]

const DEFAULT_WIDTHS: Widths = [248, 380]
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
  }, [])

  // 저장된 패널 폭은 설정을 처음 받았을 때 한 번만 반영한다
  useEffect(() => {
    if (config && !widthsLoaded.current) {
      widthsLoaded.current = true
      setWidths(config.ui.paneWidths)
    }
  }, [config])

  // 활성 저장소가 바뀌면 커밋 목록을 새로 연다
  useEffect(() => {
    useSearchStore.getState().close()
    useLogStore.getState().open(activeRepoId)
  }, [activeRepoId])

  // 그래프 필터가 바뀌면 검색 결과도 그 범위로 다시 구한다(옛 범위의 일치 표시가 남지 않게)
  const filterRev = useLogStore((s) => s.filter?.rev)
  useEffect(() => {
    const search = useSearchStore.getState()
    if (search.open && search.ranQuery) void search.run()
  }, [filterRev])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return
      const key = e.key.toLowerCase()
      if (key === 'p') {
        e.preventDefault()
        useAppStore.getState().setSwitcherOpen(true)
      } else if (key === 'f' && useAppStore.getState().activeRepoId) {
        e.preventDefault()
        useSearchStore.getState().show()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
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
        <Sidebar width={widths[0]} />
        <Splitter {...splitter(0)} />
        {activeRepoId ? (
          <>
            <CommitList />
            <Splitter {...splitter(1)} />
            <DetailPanel width={widths[1]} />
          </>
        ) : (
          <EmptyState />
        )}
      </div>
      {error && (
        <div className="toast" role="alert" onClick={() => useAppStore.getState().clearError()}>
          {error}
        </div>
      )}
      <RepoSwitcher />
    </div>
  )
}
