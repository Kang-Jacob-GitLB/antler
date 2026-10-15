import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { Commit, Locale, RefLabel } from '@shared/types'
import { graphEngine } from '../graph/graphEngine'
import { useT } from '../i18n'
import { formatDate, initials } from '../lib/format'
import { branchKey, fnv1a, laneIndexFor, laneVar, LANE_COUNT } from '../lib/laneColor'
import { useAppStore } from '../stores/appStore'
import { PREFETCH, useLogStore } from '../stores/logStore'
import { useSearchStore } from '../stores/searchStore'
import { GraphCanvas, graphWidth, useGraphVersion } from './GraphCanvas'
import { IconCloud, IconMonitor, IconTag } from './icons'
import { SearchBar } from './SearchBar'
import './CommitList.css'

const ROW_H = 28

/** GitKraken 처럼 같은 이름의 로컬·리모트 브랜치를 하나의 칩으로 합친다 */
interface Chip {
  key: string
  label: string
  kind: 'branch' | 'tag'
  local: boolean
  remote: boolean
  head: boolean
  lane: number
}

function toChips(refs: RefLabel[]): Chip[] {
  const headBranch = refs.find((r) => r.kind === 'head')?.name
  const map = new Map<string, Chip>()
  for (const r of refs) {
    if (r.kind === 'head') continue
    if (r.kind === 'tag') {
      map.set(`t:${r.name}`, { key: `t:${r.name}`, label: r.name, kind: 'tag', local: false, remote: false, head: false, lane: -1 })
      continue
    }
    const name = branchKey(r.name, r.kind === 'remote')
    const key = `b:${name}`
    const chip = map.get(key) ?? {
      key,
      label: name,
      kind: 'branch' as const,
      local: false,
      remote: false,
      head: false,
      lane: laneIndexFor(name)
    }
    if (r.kind === 'local') {
      chip.local = true
      chip.head = r.name === headBranch
    } else chip.remote = true
    map.set(key, chip)
  }
  // HEAD 브랜치 → 로컬 → 리모트 → 태그 순
  return [...map.values()].sort((a, b) => rank(a) - rank(b))
}
const rank = (c: Chip): number => (c.head ? 0 : c.kind === 'tag' ? 3 : c.local ? 1 : 2)

const RefChip = ({ chip }: { chip: Chip }): React.JSX.Element => (
  <span
    className={`ref-chip ref-chip--${chip.kind}${chip.head ? ' is-head' : ''}`}
    style={chip.kind === 'branch' ? ({ '--chip': laneVar(chip.lane) } as React.CSSProperties) : undefined}
    title={chip.label}
  >
    {chip.kind === 'tag' ? (
      <IconTag size={12} className="ref-chip__icon" />
    ) : (
      <>
        {chip.local && <IconMonitor size={12} className="ref-chip__icon" />}
        {chip.remote && <IconCloud size={12} className="ref-chip__icon" />}
      </>
    )}
    <span className="ref-chip__label">{chip.label}</span>
  </span>
)

/** 검색 중 행 표시: 일치(match) / 불일치(dim) / 검색 아님(none) */
type SearchMark = 'match' | 'current' | 'dim' | 'none'

const Row = memo(function Row({
  commit,
  index,
  selected,
  locale,
  mark,
  onSelect
}: {
  commit: Commit
  index: number
  selected: boolean
  locale: Locale
  mark: SearchMark
  onSelect(i: number): void
}) {
  const chips = useMemo(() => toChips(commit.refs), [commit.refs])
  const avatarLane = fnv1a(commit.authorEmail) % LANE_COUNT
  const firstChip = chips[0]
  return (
    <div
      className={`commit-row${selected ? ' is-selected' : ''}${mark === 'none' ? '' : ` is-${mark}`}`}
      onMouseDown={() => onSelect(index)}
      role="row"
      aria-selected={selected}
    >
      <div className="cell cell--refs">
        {firstChip && <RefChip chip={firstChip} />}
        {chips.length > 1 && (
          <span className="ref-more" title={chips.slice(1).map((c) => c.label).join('\n')}>
            +{chips.length - 1}
          </span>
        )}
      </div>
      {/* 그래프는 GraphCanvas 가 이 컬럼 위에 겹쳐 그린다 */}
      <div className="cell cell--graph" />
      <div className="cell cell--message">
        <span className="commit-subject">{commit.subject}</span>
      </div>
      <div className="cell cell--author">
        <span className="avatar" style={{ '--avatar': laneVar(avatarLane) } as React.CSSProperties}>
          {initials(commit.authorName)}
        </span>
        <span className="cell__text">{commit.authorName}</span>
      </div>
      <div className="cell cell--date">{formatDate(commit.authorDate, locale)}</div>
      <div className="cell cell--sha mono">{commit.hash.slice(0, 7)}</div>
    </div>
  )
})

export function CommitList(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { commits, hasMore, loading, error, selected, scrollRequest } = useLogStore()
  const { loadMore, select } = useLogStore.getState()
  const search = useSearchStore()
  const [scrollEl, setScrollEl] = useState<HTMLDivElement | null>(null)
  const headGraphRef = useRef<HTMLDivElement>(null)
  const [graphBox, setGraphBox] = useState({ left: 0, width: 0 })
  useGraphVersion()
  const colGraph = graphWidth(graphEngine.maxWidth)

  const virtualizer = useVirtualizer({
    count: commits.length + (hasMore ? 1 : 0),
    getScrollElement: () => scrollEl,
    estimateSize: () => ROW_H,
    overscan: 20
  })
  const items = virtualizer.getVirtualItems()
  const lastIndex = items.at(-1)?.index ?? 0

  useEffect(() => {
    if (hasMore && !loading && lastIndex >= commits.length - PREFETCH) void loadMore()
  }, [lastIndex, commits.length, hasMore, loading, loadMore])

  useEffect(() => {
    if (selected >= 0) virtualizer.scrollToIndex(selected, { align: 'auto' })
  }, [selected, virtualizer])

  // 사이드바·검색에서 찾아간 커밋은 화면 가운데로
  useEffect(() => {
    if (scrollRequest) virtualizer.scrollToIndex(scrollRequest.index, { align: 'center' })
  }, [scrollRequest, virtualizer])

  // 그래프 canvas 를 헤더의 그래프 컬럼 위치에 맞춘다(컬럼 폭은 container query 로도 바뀐다)
  useLayoutEffect(() => {
    const el = headGraphRef.current
    if (!el) return
    // 헤더 행 기준 x 좌표(스크롤 컨테이너와 헤더는 같은 가로 위치에서 시작한다)
    const measure = (): void =>
      setGraphBox({ left: el.getBoundingClientRect().left - el.parentElement!.getBoundingClientRect().left, width: el.offsetWidth })
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    ro.observe(el.parentElement!)
    return () => ro.disconnect()
  }, [])

  const searching = search.open && search.hitSet.size > 0
  const current = search.cursor >= 0 ? search.hits[search.cursor] : undefined
  const markOf = (c: Commit): SearchMark =>
    !searching ? 'none' : c.hash === current ? 'current' : search.hitSet.has(c.hash) ? 'match' : 'dim'

  const onKeyDown = (e: React.KeyboardEvent): void => {
    const page = Math.max(1, Math.floor((scrollEl?.clientHeight ?? ROW_H * 10) / ROW_H) - 1)
    const moves: Record<string, number> = {
      ArrowDown: selected + 1,
      ArrowUp: selected - 1,
      PageDown: selected + page,
      PageUp: selected - page,
      Home: 0,
      End: commits.length - 1
    }
    const next = moves[e.key]
    if (next === undefined) return
    e.preventDefault()
    select(next)
  }

  const showEmpty = !loading && !hasMore && commits.length === 0

  return (
    <section className="commit-list" style={{ '--col-graph': `${colGraph}px` } as React.CSSProperties}>
      <SearchBar />
      <div className="commit-list__head" role="row">
        <div className="cell cell--refs">{t('col.refs')}</div>
        <div className="cell cell--graph" ref={headGraphRef}>
          {t('col.graph')}
        </div>
        <div className="cell cell--message">{t('col.message')}</div>
        <div className="cell cell--author">{t('col.author')}</div>
        <div className="cell cell--date">{t('col.date')}</div>
        <div className="cell cell--sha">{t('col.sha')}</div>
      </div>

      <div
        ref={setScrollEl}
        className="commit-list__scroll"
        tabIndex={0}
        role="grid"
        aria-rowcount={commits.length}
        onKeyDown={onKeyDown}
      >
        <div className="graph-layer">
          <GraphCanvas scrollEl={scrollEl} rowHeight={ROW_H} left={graphBox.left} width={graphBox.width} commits={commits} />
        </div>
        {(error || showEmpty) && (
          <div className={`commit-list__notice${error ? ' is-error' : ''}`}>
            {error ? t('error.generic', { msg: error }) : t('list.noCommits')}
          </div>
        )}
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {items.map((vi) => {
            const commit = commits[vi.index]
            return (
              <div
                key={vi.key}
                className="commit-list__slot"
                style={{ transform: `translateY(${vi.start}px)`, height: ROW_H }}
              >
                {commit ? (
                  <Row
                    commit={commit}
                    index={vi.index}
                    selected={vi.index === selected}
                    locale={locale}
                    mark={markOf(commit)}
                    onSelect={select}
                  />
                ) : (
                  <div className="commit-row commit-row--loading">{t('list.loading')}</div>
                )}
              </div>
            )
          })}
        </div>
      </div>

      <footer className="commit-list__foot">
        {t('list.count', { n: commits.length.toLocaleString(locale), more: hasMore ? '+' : '' })}
      </footer>
    </section>
  )
}
