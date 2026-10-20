import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { Commit, Locale, RefLabel } from '@shared/types'
import { parsePrNumber } from '@shared/prNumber'
import { graphEngine } from '../graph/graphEngine'
import { useT } from '../i18n'
import { DateText } from './DateText'
import { branchKey, laneIndexFor, laneVar } from '../lib/laneColor'
import { isHidden } from '../lib/refView'
import { useAppStore } from '../stores/appStore'
import { useGithubStore } from '../stores/githubStore'
import { scopeOf, useVisibleChecks } from '../stores/checksStore'
import { pendingAuthors, requestAvatars, useAvatarStore } from '../stores/avatarStore'
import { PREFETCH, useLogStore } from '../stores/logStore'
import { useSearchStore } from '../stores/searchStore'
import { selectWip } from '../stores/wipActions'
import { useWipStore, wipDirty } from '../stores/wipStore'
import { GraphCanvas, graphWidth, useGraphVersion } from './GraphCanvas'
import { AuthorAvatar } from './AuthorAvatar'
import { AvatarBlockNotice } from './AvatarBlockNotice'
import { CommitCheck } from './github/CommitCheck'
import { CommitMinimap } from './CommitMinimap'
import { ChipFlowDot } from './FlowBadge'
import { IconCloud, IconMonitor, IconTag } from './icons'
import { SearchBar } from './SearchBar'
import { WipGraph } from './WipGraph'
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
  /** 더블클릭하면 이 rev 로 그래프를 거른다(로컬 브랜치가 있으면 그것, 없으면 리모트, 태그는 태그) */
  rev: string
  /** 이 칩이 합친 모든 ref(로컬·리모트). 숨김 표시는 이 전부를 본다 */
  revs: string[]
}

function toChips(refs: RefLabel[]): Chip[] {
  const headBranch = refs.find((r) => r.kind === 'head')?.name
  const map = new Map<string, Chip>()
  for (const r of refs) {
    if (r.kind === 'head') continue
    if (r.kind === 'tag') {
      map.set(`t:${r.name}`, { key: `t:${r.name}`, label: r.name, kind: 'tag', local: false, remote: false, head: false, lane: -1, rev: `refs/tags/${r.name}`, revs: [`refs/tags/${r.name}`] })
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
      lane: laneIndexFor(name),
      rev: '',
      revs: []
    }
    if (r.kind === 'local') {
      chip.local = true
      chip.head = r.name === headBranch
      chip.rev = `refs/heads/${r.name}`
      chip.revs.push(chip.rev)
    } else {
      chip.remote = true
      chip.revs.push(`refs/remotes/${r.name}`)
      if (!chip.local && !chip.rev) chip.rev = `refs/remotes/${r.name}`
    }
    map.set(key, chip)
  }
  // HEAD 브랜치 → 로컬 → 리모트 → 태그 순
  return [...map.values()].sort((a, b) => rank(a) - rank(b))
}
const rank = (c: Chip): number => (c.head ? 0 : c.kind === 'tag' ? 3 : c.local ? 1 : 2)

/** 칩을 더블클릭하면 그 브랜치·태그만 solo 한다(사이드바 더블클릭과 같다) */
function filterByChip(chip: Chip): void {
  const label = chip.kind === 'branch' && !chip.local ? chip.rev.replace(/^refs\/remotes\//, '') : chip.label
  const kind = chip.kind === 'tag' ? 'tag' : chip.local ? 'branch' : 'remote'
  useLogStore.getState().setFilter({ id: chip.rev, label, kind })
}

const RefChip = ({ chip, filterTitle }: { chip: Chip; filterTitle: string }): React.JSX.Element => {
  // 숨긴 ref 는 흐리게 보인다(Hide 는 solo 가 없을 때만 적용된다)
  // 로컬·리모트를 합친 칩은 둘 다 숨겼을 때만 완전히 흐리고, 하나만 숨겼으면 부분 표시(툴팁에 어느 쪽인지)
  const hiddenCount = useLogStore((s) => (s.solo.length === 0 ? chip.revs.filter((r) => isHidden(s.hide, r)).length : 0))
  const t = useT()
  const hidden = hiddenCount > 0 && hiddenCount === chip.revs.length
  const partial = hiddenCount > 0 && !hidden
  const partNames = partial ? chip.revs.filter((r) => isHidden(useLogStore.getState().hide, r)).map((r) => r.replace(/^refs\/(heads|remotes|tags)\//, '')).join(', ') : ''
  return (
  <span
    className={`ref-chip ref-chip--${chip.kind}${chip.head ? ' is-head' : ''}${hidden ? ' is-hidden' : ''}${partial ? ' is-hidden-part' : ''}`}
    style={chip.kind === 'branch' ? ({ '--chip': laneVar(chip.lane) } as React.CSSProperties) : undefined}
    title={`${chip.label}\n${filterTitle}${partial ? `\n${t('list.chipPartHidden', { names: partNames })}` : ''}`}
    onDoubleClick={(e) => {
      e.stopPropagation()
      filterByChip(chip)
    }}
  >
    {chip.kind === 'tag' ? (
      <IconTag size={12} className="ref-chip__icon" />
    ) : (
      <>
        {chip.local && <IconMonitor size={12} className="ref-chip__icon" />}
        {chip.remote && <IconCloud size={12} className="ref-chip__icon" />}
      </>
    )}
    {chip.kind === 'branch' && <ChipFlowDot label={chip.label} />}
    <span className="ref-chip__label">{chip.label}</span>
  </span>
  )
}

/** 검색 중 행 표시: 일치(match) / 불일치(dim) / 검색 아님(none) */
type SearchMark = 'match' | 'current' | 'dim' | 'none'

/** PR 열: 제목에서 읽은 번호. GitHub remote 가 있으면 앱 안의 PR 화면을 여는 칩(GitHub 로 가는 버튼은 그 화면에 있다), 없으면 그냥 글자 */
function PrCell({ n, linkable, label }: { n: number | null; linkable: boolean; label: string }): React.JSX.Element {
  return (
    <div className="cell cell--pr">
      {n !== null &&
        (linkable ? (
          <button
            type="button"
            className="pr-chip mono"
            title={label}
            aria-label={label}
            // 행 선택(mousedown)으로 번지지 않게 하고, 키보드로도 누를 수 있게 버튼으로 둔다
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => useGithubStore.getState().show({ kind: 'pull', number: n, tab: 'conversation' })}
          >
            #{n}
          </button>
        ) : (
          <span className="pr-text mono">#{n}</span>
        ))}
    </div>
  )
}

const Row = memo(function Row({
  commit,
  index,
  selected,
  locale,
  mark,
  compared,
  showPr,
  prLinkable,
  onSelect
}: {
  commit: Commit
  index: number
  selected: boolean
  locale: Locale
  mark: SearchMark
  /** 두 커밋 비교에서 선택 행과 짝이 되는 행 */
  compared: boolean
  showPr: boolean
  /** GitHub remote 가 있어 PR 화면을 열 수 있는지 */
  prLinkable: boolean
  onSelect(i: number, e: React.MouseEvent): void
}) {
  const t = useT()
  const chips = useMemo(() => toChips(commit.refs), [commit.refs])
  const pr = useMemo(() => (showPr ? parsePrNumber(commit.subject) : null), [showPr, commit.subject])
  const firstChip = chips[0]
  return (
    <div
      className={`commit-row${selected ? ' is-selected' : ''}${compared ? ' is-compared' : ''}${mark === 'none' ? '' : ` is-${mark}`}`}
      onMouseDown={(e) => onSelect(index, e)}
      role="row"
      aria-selected={selected || compared}
    >
      <div className="cell cell--refs">
        {firstChip && <RefChip chip={firstChip} filterTitle={t('list.chipFilter')} />}
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
        <CommitCheck hash={commit.hash} />
      </div>
      {showPr && <PrCell n={pr} linkable={prLinkable} label={pr === null ? '' : t('pr.open', { n: pr })} />}
      <div className="cell cell--author">
        <AuthorAvatar email={commit.authorEmail} name={commit.authorName} size={18} />
        <span className="cell__text">{commit.authorName}</span>
      </div>
      <div className="cell cell--date">
        <DateText epochSec={commit.authorDate} locale={locale} />
      </div>
      <div className="cell cell--sha mono">{commit.hash.slice(0, 7)}</div>
    </div>
  )
})

/**
 * 커밋 행 선택: WIP 선택을 풀고 그 커밋을 고른다.
 * Ctrl(⌘)·Shift+클릭은 이미 선택한 커밋이 있을 때 그 커밋과의 두 커밋 비교 모드로 연다.
 */
function selectCommit(i: number, e?: React.MouseEvent): void {
  const log = useLogStore.getState()
  if (e && (e.ctrlKey || e.metaKey || e.shiftKey) && log.selected >= 0 && !useWipStore.getState().selected) {
    // Shift+클릭이 글자 범위 선택으로 번지지 않게 한다
    e.preventDefault()
    log.compareWith(i)
    return
  }
  selectWip(false)
  log.select(i)
}

/** WIP 행 선택: 비교 모드는 끝낸다 */
function selectWipRow(): void {
  useLogStore.getState().clearCompare()
  selectWip(true)
}

/** 맨 위 "// WIP" 행 — 커밋하지 않은 작업 트리 변경(수정 ✎ n · 추가 + n). 그래프의 점선 노드는 WipGraph 가 그린다 */
function WipRow({ selected, showPr }: { selected: boolean; showPr: boolean }): React.JSX.Element | null {
  const t = useT()
  const counts = useWipStore((s) => s.status?.counts)
  const conflicted = useWipStore((s) => s.status?.conflicted ?? 0)
  if (!counts) return null
  const total = counts.staged + counts.unstaged + counts.untracked
  return (
    <div
      className={`commit-row commit-row--wip${selected ? ' is-selected' : ''}`}
      onMouseDown={selectWipRow}
      role="row"
      aria-selected={selected}
      title={t('wip.rowTitle', { n: total.toLocaleString() })}
    >
      <div className="cell cell--refs" />
      <div className="cell cell--graph" />
      <div className="cell cell--message">
        <span className="wip-label">{t('wip.label')}</span>
        {counts.modified > 0 && (
          <span className="wip-badge wip-badge--mod" title={t('wip.badgeModified', { n: counts.modified.toLocaleString() })}>
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M2 10l.6-2.6L8.2 1.8a1.1 1.1 0 0 1 1.6 0l.4.4a1.1 1.1 0 0 1 0 1.6L4.6 9.4 2 10z" />
            </svg>
            {counts.modified.toLocaleString()}
          </span>
        )}
        {counts.added > 0 && (
          <span className="wip-badge wip-badge--add" title={t('wip.badgeAdded', { n: counts.added.toLocaleString() })}>
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
              <path d="M6 2.5v7M2.5 6h7" />
            </svg>
            {counts.added.toLocaleString()}
          </span>
        )}
        {conflicted > 0 && <span className="wip-badge wip-badge--conflict">{t('wip.conflicts', { n: conflicted })}</span>}
      </div>
      {showPr && <div className="cell cell--pr" />}
      <div className="cell cell--author" />
      <div className="cell cell--date" />
      <div className="cell cell--sha" />
    </div>
  )
}

export function CommitList(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { commits, hasMore, loading, error, selected, scrollRequest, solo, repoId: logRepoId, compareHash } = useLogStore()
  const { loadMore } = useLogStore.getState()
  const search = useSearchStore()
  // WIP 행: 변경이 있고 전체 그래프(Solo 없음 — Hide 는 기본 보기 기준이라 WIP 를 둔다)를 볼 때 맨 위에 둔다. 커밋 행은 그만큼 아래로 밀린다
  const wipDirtyNow = useWipStore((s) => wipDirty(s.status))
  const wipSelected = useWipStore((s) => s.selected)
  // PR 열: GitHub remote 가 있거나(번호가 있는 커밋만 칩이 보인다), remote 가 없어도 제목에 PR 번호가 보이는 커밋이 하나라도 있을 때
  const ghRef = useGithubStore((s) => s.ref)
  const anyPr = useMemo(() => commits.some((c) => parsePrNumber(c.subject) !== null), [commits])
  const showPr = !!ghRef || anyPr
  const showWip = wipDirtyNow && solo.length === 0 && commits.length > 0
  const wipOff = showWip ? ROW_H : 0
  const wipOn = showWip && wipSelected
  const [scrollEl, setScrollEl] = useState<HTMLDivElement | null>(null)
  const headGraphRef = useRef<HTMLDivElement>(null)
  const [graphBox, setGraphBox] = useState({ left: 0, width: 0 })
  useGraphVersion()
  const avatars = useAppStore((s) => s.config?.ui.graphInitials !== false)
  const colGraph = graphWidth(graphEngine.maxWidth, avatars)

  const virtualizer = useVirtualizer({
    count: commits.length + (hasMore ? 1 : 0),
    getScrollElement: () => scrollEl,
    estimateSize: () => ROW_H,
    overscan: 20,
    // WIP 행이 가상 목록 위에 흐름대로 놓여 있어 목록의 시작이 그만큼 아래다
    scrollMargin: wipOff
  })
  const items = virtualizer.getVirtualItems()
  const lastIndex = items.at(-1)?.index ?? 0

  useEffect(() => {
    if (hasMore && !loading && lastIndex >= commits.length - PREFETCH) void loadMore()
  }, [lastIndex, commits.length, hasMore, loading, loadMore])

  // 화면에 보이는 행의 작성자 중 아직 모르는 사람의 프로필 사진을 묻는다(debounce 는 스토어가 한다)
  const firstIndex = items[0]?.index ?? 0
  // 로그인·로그아웃으로 조건이 바뀌면(epoch) 보이는 작성자를 다시 묻는다
  const avatarEpoch = useAvatarStore((s) => s.epoch)
  useEffect(() => {
    if (!logRepoId) return
    requestAvatars(logRepoId, pendingAuthors(commits, firstIndex, lastIndex))
  }, [logRepoId, commits, firstIndex, lastIndex, avatarEpoch])

  // GitHub 저장소면 보이는 커밋의 체크(Actions) 상태도 일괄로 묻는다(로그인했을 때만 표시된다)
  useVisibleChecks(logRepoId, scopeOf(ghRef), !!ghRef, commits, firstIndex, lastIndex)

  useEffect(() => {
    if (selected >= 0) virtualizer.scrollToIndex(selected, { align: 'auto' })
  }, [selected, virtualizer])

  // WIP 행을 고르면 맨 위로(키보드로 올라온 경우 보이게)
  useEffect(() => {
    if (wipOn) scrollEl?.scrollTo({ top: 0 })
  }, [wipOn, scrollEl])

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
    // WIP 행은 커밋 0번 바로 위(-1)에 있는 것처럼 움직인다
    const at = wipOn ? -1 : selected
    const moves: Record<string, number> = {
      ArrowDown: at + 1,
      ArrowUp: at - 1,
      PageDown: at + page,
      PageUp: at - page,
      Home: 0,
      End: commits.length - 1
    }
    if (e.key === 'Escape' && compareHash) {
      e.preventDefault()
      useLogStore.getState().clearCompare()
      return
    }
    const next = moves[e.key]
    if (next === undefined) return
    e.preventDefault()
    if (showWip && next < 0 && (e.key === 'ArrowUp' || e.key === 'PageUp')) selectWipRow()
    else selectCommit(next)
  }

  const showEmpty = !loading && !hasMore && commits.length === 0

  return (
    <section className={`commit-list${showPr ? ' has-pr' : ''}`} style={{ '--col-graph': `${colGraph}px` } as React.CSSProperties}>
      <CommitMinimap scrollEl={scrollEl} rowHeight={ROW_H} topOffset={wipOff} />
      <SearchBar />
      <div className="commit-list__head" role="row">
        <div className="cell cell--refs">{t('col.refs')}</div>
        <div className="cell cell--graph" ref={headGraphRef}>
          {t('col.graph')}
        </div>
        <div className="cell cell--message">{t('col.message')}</div>
        {showPr && <div className="cell cell--pr">{t('col.pr')}</div>}
        <div className="cell cell--author">
          {t('col.author')}
          <AvatarBlockNotice locale={locale} />
        </div>
        <div className="cell cell--date">{t('col.date')}</div>
        <div className="cell cell--sha">{t('col.sha')}</div>
      </div>

      <div
        ref={setScrollEl}
        className="commit-list__scroll"
        tabIndex={0}
        role="grid"
        aria-rowcount={commits.length + (showWip ? 1 : 0)}
        onKeyDown={onKeyDown}
      >
        <div className="graph-layer">
          {/* WIP 점선 노드·연결선이 아래, 레인 그래프(커밋 노드)가 위 */}
          {showWip && <WipGraph scrollEl={scrollEl} rowHeight={ROW_H} left={graphBox.left} width={graphBox.width} topOffset={wipOff} />}
          <GraphCanvas scrollEl={scrollEl} rowHeight={ROW_H} left={graphBox.left} width={graphBox.width} commits={commits} topOffset={wipOff} />
        </div>
        {(error || showEmpty) && (
          <div className={`commit-list__notice${error ? ' is-error' : ''}`}>
            {error ? t('error.generic', { msg: error }) : t('list.noCommits')}
          </div>
        )}
        {showWip && (
          <div className="commit-list__wip" style={{ height: ROW_H }}>
            <WipRow selected={wipOn} showPr={showPr} />
          </div>
        )}
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {items.map((vi) => {
            const commit = commits[vi.index]
            return (
              <div
                key={vi.key}
                className="commit-list__slot"
                style={{ transform: `translateY(${vi.start - wipOff}px)`, height: ROW_H }}
              >
                {commit ? (
                  <Row
                    commit={commit}
                    index={vi.index}
                    selected={!wipOn && vi.index === selected}
                    locale={locale}
                    mark={markOf(commit)}
                    compared={!!compareHash && commit.hash === compareHash}
                    showPr={showPr}
                    prLinkable={!!ghRef}
                    onSelect={selectCommit}
                  />
                ) : (
                  <div className="commit-row commit-row--loading">{t('list.loading')}</div>
                )}
              </div>
            )
          })}
        </div>
      </div>
    </section>
  )
}
