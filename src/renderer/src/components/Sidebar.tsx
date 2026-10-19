import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import type { Branch, Repo, Source, Stash, SubmoduleInfo, Tag, ViewRef, Worktree } from '@shared/types'
import { useT } from '../i18n'
import { formatDate } from '../lib/format'
import { branchKey, laneIndexFor, laneVar } from '../lib/laneColor'
import { useAppStore } from '../stores/appStore'
import { canHide, inList, isHidden, remoteAllRef } from '../lib/refView'
import { useLogStore } from '../stores/logStore'
import { EnvBadge } from './EnvBadge'
import { RepoMenu } from './RepoMenu'
import { BranchFlowBadge } from './FlowBadge'
import { ChecksBadge, GhStateIcon, prState } from './github/GhBits'
import { useGithubStore } from '../stores/githubStore'
import { useJiraSiteData, useJiraStore } from '../stores/jiraStore'
import {
  IconCheck,
  IconChevronRight,
  IconClose,
  IconCloud,
  IconFolder,
  IconEye,
  IconEyeOff,
  IconFunnel,
  IconMore,
  IconMonitor,
  IconPalette,
  IconPlus,
  IconRepo,
  IconSearch,
  IconSolo,
  IconStash,
  IconSubmodule,
  IconTag,
  IconWorktree
} from './icons'
import { GroupChip } from './GroupChip'
import { Collapse } from './Collapse'
import { ColorMenu } from './ColorMenu'
import { permuteSlots, sortByOrder } from '../lib/repoOrder'
import { useReorderDnd } from '../lib/useReorderDnd'
import './Sidebar.css'

/** 기본으로 접어 둘 폴더 크기 */
const AUTO_COLLAPSE = 30
/** 사이드바에 보이는 PR·이슈 수(나머지는 '전체 보기') */
const SIDEBAR_GH_MAX = 30

// ── 접힘 상태 ─────────────────────────────────────────
// ui.collapsed 에는 기본값과 다르게 바꾼 키만 저장한다: 'key' = 접음, '!key' = 펼침

function useCollapse(): { isOpen(key: string, defaultOpen?: boolean): boolean; toggle(key: string, defaultOpen?: boolean): void } {
  const collapsed = useAppStore((s) => s.config?.ui.collapsed)
  const set = useMemo(() => new Set(collapsed ?? []), [collapsed])
  const isOpen = (key: string, defaultOpen = true): boolean =>
    set.has(key) ? false : set.has(`!${key}`) ? true : defaultOpen
  const toggle = (key: string, defaultOpen = true): void => {
    const open = isOpen(key, defaultOpen)
    const next = new Set(set)
    next.delete(key)
    next.delete(`!${key}`)
    // 바뀐 결과가 기본값과 다를 때만 기록한다
    if (open === defaultOpen) next.add(open ? key : `!${key}`)
    void useAppStore.getState().setUi({ collapsed: [...next] })
  }
  return { isOpen, toggle }
}

// ── 브랜치 이름 → 폴더 트리 ──────────────────────────

interface TreeNode<T> {
  name: string
  path: string
  children: TreeNode<T>[]
  items: { label: string; value: T }[]
  count: number
}

function buildTree<T>(entries: { name: string; value: T }[]): TreeNode<T> {
  const root: TreeNode<T> = { name: '', path: '', children: [], items: [], count: 0 }
  for (const e of entries) {
    const parts = e.name.split('/')
    let node = root
    node.count++
    for (let i = 0; i < parts.length - 1; i++) {
      const path = parts.slice(0, i + 1).join('/')
      let child = node.children.find((c) => c.name === parts[i])
      if (!child) {
        child = { name: parts[i]!, path, children: [], items: [], count: 0 }
        node.children.push(child)
      }
      child.count++
      node = child
    }
    node.items.push({ label: parts[parts.length - 1]!, value: e.value })
  }
  return root
}

// ── 공용 행 ─────────────────────────────────────────

interface RowProps {
  depth: number
  icon: ReactNode
  label: string
  title?: string
  active?: boolean
  muted?: boolean
  disabled?: boolean
  trailing?: ReactNode
  onClick?(): void
  /** Solo / Hide 대상 ref. 있으면 더블클릭 solo, hover 아이콘, 숨김 행 흐리게가 붙는다 */
  view?: ViewRef
  onContextMenu?(e: React.MouseEvent): void
  /** 끌어서 순서 바꾸기(저장소 행): 드래그 속성과 상태 클래스 */
  drag?: { props: React.HTMLAttributes<HTMLElement>; cls: string }
}

/** hover 때 나타나는 숨기기(눈)·Solo 버튼. 숨김·Solo 상태인 행에서는 항상 보인다 */
function ViewActions({ view }: { view: ViewRef }): React.JSX.Element {
  const t = useT()
  const soloOn = useLogStore((s) => inList(s.solo, view.id))
  const hideOn = useLogStore((s) => inList(s.hide, view.id))
  // 리모트 전체가 숨겨져 있어 이 브랜치도 가려진 상태 — 개별로는 풀 수 없다
  const viaRemote = useLogStore((s) => !inList(s.hide, view.id) && isHidden(s.hide, view.id))
  const click = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation()
    fn()
  }
  const { toggleHide, toggleSolo } = useLogStore.getState()
  return (
    <>
      {canHide(view) && (
        <button
          className={`side-item__action${hideOn || viaRemote ? ' is-on' : ''}`}
          title={viaRemote ? t('view.hiddenByRemote') : hideOn ? t('view.show') : t('view.hide')}
          aria-pressed={hideOn || viaRemote}
          disabled={viaRemote}
          onClick={click(() => toggleHide(view))}
          onDoubleClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
        >
          {hideOn || viaRemote ? <IconEyeOff size={12} /> : <IconEye size={12} />}
        </button>
      )}
      <button
        className={`side-item__action${soloOn ? ' is-on' : ''}`}
        title={soloOn ? t('view.unsolo') : t('view.solo')}
        aria-pressed={soloOn}
        onClick={click(() => toggleSolo(view))}
        onDoubleClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <IconSolo size={12} />
      </button>
    </>
  )
}

function ItemRow({ depth, icon, label, title, active, muted, disabled, trailing, onClick, view, onContextMenu, drag }: RowProps): React.JSX.Element {
  const soloOn = useLogStore((s) => !!view && inList(s.solo, view.id))
  // Hide 는 solo 가 없을 때만 적용된다
  const hidden = useLogStore((s) => !!view && s.solo.length === 0 && isHidden(s.hide, view.id))
  return (
    <div
      role="button"
      tabIndex={0}
      aria-disabled={disabled || undefined}
      {...drag?.props}
      className={`side-item${active || soloOn ? ' is-active' : ''}${muted ? ' is-muted' : ''}${hidden ? ' is-hidden' : ''}${drag?.cls ?? ''}`}
      style={{ paddingLeft: 8 + depth * 14 }}
      title={title ?? label}
      onClick={onClick}
      onDoubleClick={view ? () => useLogStore.getState().setFilter(view) : undefined}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onClick?.()
      }}
    >
      <span className="side-item__icon">{icon}</span>
      <span className="side-item__label">{label}</span>
      {trailing}
      {view && <ViewActions view={view} />}
    </div>
  )
}

function FolderRow({ depth, name, count, open, onToggle }: { depth: number; name: string; count: number; open: boolean; onToggle(): void }): React.JSX.Element {
  return (
    <div role="button" tabIndex={0} className="side-item side-item--folder" style={{ paddingLeft: 8 + depth * 14 }} onClick={onToggle}>
      <IconChevronRight size={12} className={`side-chev${open ? ' is-open' : ''}`} />
      <IconFolder size={13} className="side-item__icon" />
      <span className="side-item__label">{name}</span>
      <span className="side-count">{count}</span>
    </div>
  )
}

/** 리모트 폴더 행: 리모트 전체를 Solo / 숨기기 할 수 있다 */
function RemoteRow({ name, count, open, onToggle, onContextMenu }: { name: string; count: number; open: boolean; onToggle(): void; onContextMenu(e: React.MouseEvent): void }): React.JSX.Element {
  const view = useMemo(() => remoteAllRef(name), [name])
  const soloOn = useLogStore((s) => inList(s.solo, view.id))
  const hidden = useLogStore((s) => s.solo.length === 0 && inList(s.hide, view.id))
  return (
    <div
      role="button"
      tabIndex={0}
      className={`side-item side-item--folder${soloOn ? ' is-active' : ''}${hidden ? ' is-hidden' : ''}`}
      style={{ paddingLeft: 8 }}
      onClick={onToggle}
      onDoubleClick={() => useLogStore.getState().setFilter(view)}
      onContextMenu={onContextMenu}
    >
      <IconChevronRight size={12} className={`side-chev${open ? ' is-open' : ''}`} />
      <IconCloud size={13} className="side-item__icon" />
      <span className="side-item__label">{name}</span>
      <span className="side-count">{count}</span>
      <ViewActions view={view} />
    </div>
  )
}

/** 색 바꾸기 버튼과 팝오버(그룹 행·환경 카드 머리). 행을 우클릭해도 열린다(open/setOpen 을 행이 쥔다) */
function ColorAction({
  open,
  setOpen,
  value,
  onPick,
  allowDefault
}: {
  open: boolean
  setOpen(v: boolean): void
  value: number | undefined
  onPick(hue: number | undefined): void
  allowDefault?: boolean
}): React.JSX.Element {
  const t = useT()
  return (
    <>
      <button
        className={`side-item__action${open ? ' is-on' : ''}`}
        title={t('color.pick')}
        aria-haspopup="dialog"
        aria-expanded={open}
        // 열린 팝오버의 '바깥 누름 → 닫기'가 이 버튼에서 먼저 닫고 click 이 다시 여는 것을 막는다
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation()
          setOpen(!open)
        }}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <IconPalette size={12} />
      </button>
      {open && <ColorMenu value={value} onPick={onPick} onClose={() => setOpen(false)} allowDefault={allowDefault} />}
    </>
  )
}

/** 저장소 그룹 폴더 행: 폴더 아이콘·이름 대신 그룹 칩(이름별 색). 끌어서 순서를 바꾸고, 색을 고칠 수 있다 */
function GroupRow({
  name,
  count,
  open,
  onToggle,
  drag
}: {
  name: string
  count: number
  open: boolean
  onToggle(): void
  drag: { props: React.HTMLAttributes<HTMLElement>; cls: string }
}): React.JSX.Element {
  const t = useT()
  const colors = useAppStore((s) => s.config?.ui.groupColors)
  const [menu, setMenu] = useState(false)
  return (
    <div
      role="button"
      tabIndex={0}
      aria-expanded={open}
      {...drag.props}
      className={`side-item side-item--folder side-item--group${drag.cls}`}
      style={{ paddingLeft: 8 }}
      title={`${name} · ${t('sidebar.dragHint')}`}
      onClick={onToggle}
      onContextMenu={(e) => {
        e.preventDefault()
        setMenu(true)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
    >
      <IconChevronRight size={12} className={`side-chev${open ? ' is-open' : ''}`} />
      <GroupChip name={name} />
      <span className="side-item__spacer" />
      <ColorAction
        open={menu}
        setOpen={setMenu}
        value={colors?.[name]}
        onPick={(hue) => hue !== undefined && void useAppStore.getState().setUi({ groupColors: { ...colors, [name]: hue } })}
      />
      <span className="side-count">{count}</span>
    </div>
  )
}

function Section({
  id,
  title,
  count,
  children,
  extra,
  card
}: {
  id: string
  title: string
  count?: number
  children: ReactNode
  extra?: ReactNode
  /** 카드로 감싸 다른 섹션과 구분한다(저장소 목록) */
  card?: boolean
}): React.JSX.Element {
  const { isOpen, toggle } = useCollapse()
  const open = isOpen(`sec:${id}`)
  return (
    <section className={`side-section${card ? ' side-section--card' : ''}`}>
      <div className="side-section__head" role="button" tabIndex={0} onClick={() => toggle(`sec:${id}`)}>
        <IconChevronRight size={12} className={`side-chev${open ? ' is-open' : ''}`} />
        <span className="side-section__title">{title}</span>
        {count !== undefined && <span className="side-count">{count}</span>}
        {extra && (
          <span className="side-section__extra" onClick={(e) => e.stopPropagation()}>
            {extra}
          </span>
        )}
      </div>
      <Collapse open={open}>
        <div className="side-section__body">{children}</div>
      </Collapse>
    </section>
  )
}

/** 접을 수 있는 폴더 트리. renderItem 이 각 잎 행을 그린다 */
function Tree<T>({ node, scope, depth, renderItem }: { node: TreeNode<T>; scope: string; depth: number; renderItem(item: { label: string; value: T }, depth: number): ReactNode }): React.JSX.Element {
  const { isOpen, toggle } = useCollapse()
  return (
    <>
      {node.children.map((child) => {
        const key = `dir:${scope}:${child.path}`
        const def = child.count <= AUTO_COLLAPSE
        const open = isOpen(key, def)
        return (
          <div key={child.path}>
            <FolderRow depth={depth} name={child.name} count={child.count} open={open} onToggle={() => toggle(key, def)} />
            <Collapse open={open}>
              <Tree node={child} scope={scope} depth={depth + 1} renderItem={renderItem} />
            </Collapse>
          </div>
        )
      })}
      {node.items.map((item) => renderItem(item, depth))}
    </>
  )
}

interface CtxItem {
  label: string
  disabled?: boolean
  hint?: string
  onPick(): void
}

/** ref 행 우클릭 메뉴(현재 HEAD 와 비교 · Solo · 숨기기 · 모두 보이기). onClose(restore): restore 면 메뉴를 연 행으로 포커스를 돌려준다 */
function RefMenu({ x, y, items, onClose }: { x: number; y: number; items: CtxItem[]; onClose(restore: boolean): void }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  // 뷰포트 밖으로 나가지 않게 실제 크기로 좌표를 보정한다
  const [pos, setPos] = useState({ left: x, top: y })
  useLayoutEffect(() => {
    const r = ref.current?.getBoundingClientRect()
    if (!r) return
    setPos({ left: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)), top: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)) })
  }, [x, y])
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose(false)
    }
    // Esc·Tab 은 메뉴만 닫는다 — window 로 올라가면 열려 있던 diff 까지 닫힌다
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' || e.key === 'Tab') {
        e.preventDefault()
        e.stopPropagation()
        onClose(true)
      }
    }
    const onScroll = (): void => onClose(false)
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    // 사이드바 목록이 스크롤되면 기준 행이 움직이므로 닫는다(scroll 은 버블링하지 않아 capture)
    document.addEventListener('scroll', onScroll, true)
    window.addEventListener('blur', onScroll)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
      document.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('blur', onScroll)
    }
  }, [onClose])
  return (
    <div
      ref={ref}
      className="side-ctx"
      role="menu"
      style={pos}
      onContextMenu={(e) => e.preventDefault()}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) onClose(false)
      }}
    >
      {items.map((it, i) => (
        <button key={it.label} className="side-ctx__item" role="menuitem" disabled={it.disabled} title={it.hint} autoFocus={i === items.findIndex((x) => !x.disabled)} onClick={it.onPick}>
          {it.label}
        </button>
      ))}
    </div>
  )
}

// ── 사이드바 ─────────────────────────────────────────

function matches(q: string, ...texts: (string | undefined)[]): boolean {
  return !q || texts.some((s) => s?.toLowerCase().includes(q))
}

export function Sidebar({ width }: { width: number }): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const config = useAppStore((s) => s.config)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const refs = useAppStore((s) => s.refs)
  const { selectRepo, addRepo, openAddDialog, removeRepo, updateRepo } = useAppStore.getState()
  const [query, setQuery] = useState('')
  const q = query.trim().toLowerCase()
  const { isOpen, toggle } = useCollapse()
  /** 메뉴(그룹 지정·제거)를 연 저장소 */
  const [menuFor, setMenuFor] = useState<string | null>(null)
  // GitHub: remote 가 GitHub 저장소일 때만 PR·이슈 섹션을 보인다
  const ghRef = useGithubStore((s) => s.ref)
  const ghPulls = useGithubStore((s) => s.sidePulls)
  const ghIssues = useGithubStore((s) => s.sideIssues)
  const ghView = useGithubStore((s) => s.view)
  const ghShow = useGithubStore((s) => s.show)
  // Jira: 연결 여부와 상관없이 섹션을 보인다(연결 전에는 연결 버튼)
  const jiraAuth = useJiraStore((s) => s.auth)
  const jiraMine = useJiraStore((s) => s.mine)
  const jiraView = useJiraStore((s) => s.view)
  const jiraSite = useJiraSiteData()
  const jiraRecent = jiraSite.recent ?? []
  const jiraFilters = jiraSite.filters ?? []
  const allGroups = useMemo(
    () => [...new Set((config?.repos ?? []).map((r) => r.group).filter((g): g is string => !!g))].sort(),
    [config]
  )

  // 사용자 순서(끌어서 바꾼 순서, 없으면 환경은 등록 순·그룹과 저장소는 이름순). 숨긴 것까지 포함한 전체 순서다 —
  // 화면에 보이는 일부만 옮겨도 permuteSlots 로 그 자리 안에서만 바꾼다
  const order = useMemo(() => {
    if (!config) return { sources: [] as Source[], repoIds: [] as string[] }
    const { sourceOrder, repoOrder } = config.ui
    const sources = sortByOrder(config.sources, (s) => s.id, sourceOrder, (a, b) => config.sources.indexOf(a) - config.sources.indexOf(b))
    const repoIds = sortByOrder(config.repos, (r) => r.id, repoOrder, (a, b) => a.name.localeCompare(b.name)).map((r) => r.id)
    return { sources, repoIds }
  }, [config])

  const repoGroups = useMemo(() => {
    if (!config) return []
    const rank = new Map(order.repoIds.map((id, i) => [id, i]))
    const bySource = new Map<string, { source: Source; repos: Repo[]; groups: string[]; allGroups: string[] }>()
    for (const s of order.sources) bySource.set(s.id, { source: s, repos: [], groups: [], allGroups: [] })
    // 프로필의 '보일 그룹': 목록에 없는 그룹의 저장소는 숨긴다(그룹 없는 저장소와 지금 열어 둔 저장소는 항상 보인다)
    const visible = config.ui.visibleGroups
    for (const r of config.repos) {
      const g = bySource.get(r.sourceId)
      if (!g) continue
      if (r.group && !g.allGroups.includes(r.group)) g.allGroups.push(r.group)
      if (visible?.length && r.group && !visible.includes(r.group) && r.id !== activeRepoId) continue
      g.repos.push(r)
      if (r.group && !g.groups.includes(r.group)) g.groups.push(r.group)
    }
    return [...bySource.values()]
      .filter((g) => g.repos.length)
      .map((g) => {
        const groupOrder = config.ui.groupOrder?.[g.source.id]
        const byName = (a: string, b: string): number => a.localeCompare(b)
        return {
          ...g,
          repos: [...g.repos].sort((a, b) => rank.get(a.id)! - rank.get(b.id)!),
          groups: sortByOrder(g.groups, (x) => x, groupOrder, byName),
          allGroups: sortByOrder(g.allGroups, (x) => x, groupOrder, byName)
        }
      })
  }, [config, order, activeRepoId])

  const dnd = useReorderDnd()
  const moveSources = (next: string[]): void =>
    void useAppStore.getState().setUi({ sourceOrder: permuteSlots(order.sources.map((s) => s.id), next) })
  const moveGroups = (sourceId: string, all: string[], next: string[]): void => {
    // 없어진 환경의 항목은 이참에 지운다
    const live = new Set(config?.sources.map((s) => s.id))
    const prev = Object.fromEntries(Object.entries(config?.ui.groupOrder ?? {}).filter(([id]) => live.has(id)))
    void useAppStore.getState().setUi({ groupOrder: { ...prev, [sourceId]: permuteSlots(all, next) } })
  }
  const moveRepos = (next: string[]): void => void useAppStore.getState().setUi({ repoOrder: permuteSlots(order.repoIds, next) })
  /** 색을 고르는 중인 환경 카드(소스 id) */
  const [srcColorFor, setSrcColorFor] = useState<string | null>(null)

  const local = useMemo(() => (refs?.local ?? []).filter((b) => matches(q, b.name)), [refs, q])
  const remotes = useMemo(
    () => (refs?.remotes ?? []).map((r) => ({ ...r, branches: r.branches.filter((b) => matches(q, b.name)) })).filter((r) => r.branches.length || !q),
    [refs, q]
  )
  const tags = useMemo(() => (refs?.tags ?? []).filter((x) => matches(q, x.name)), [refs, q])
  const stashes = useMemo(() => (refs?.stashes ?? []).filter((s) => matches(q, s.message, s.ref)), [refs, q])
  const submodules = useMemo(() => (refs?.submodules ?? []).filter((m) => matches(q, m.name, m.path)), [refs, q])
  const worktrees = useMemo(() => (refs?.worktrees ?? []).filter((w) => matches(q, w.path, w.branch ?? undefined)), [refs, q])

  // Solo 로 다른 ref 만 보는 중이거나 그 ref 가 숨겨져 있으면 찾는 커밋이 목록에 없을 수 있으므로 보기를 푼 뒤 찾아간다
  const reveal = (hash: string, id?: string): void => {
    useLogStore.getState().releaseFor(id, [hash])
    void useLogStore.getState().reveal(hash)
  }
  // 폴더·리모트 접힘 키는 저장소마다 따로 둔다
  const scoped = (key: string): string => `${activeRepoId}:${key}`
  /** 서브모듈을 같은 환경의 저장소로 연다: 등록돼 있으면 전환, 아니면 같은 그룹으로 등록 후 전환 */
  const openSubmodule = (m: SubmoduleInfo): void => {
    const parent = config?.repos.find((r) => r.id === activeRepoId)
    const source = config?.sources.find((x) => x.id === parent?.sourceId)
    if (!config || !parent || !source) return
    // Windows 로컬 저장소는 역슬래시 경로로 저장돼 있다
    const win = parent.path.includes('\\') && !parent.path.startsWith('/')
    const rel = win ? m.path.replace(/\//g, '\\') : m.path
    const path = `${parent.path.replace(/[\\/]+$/, '')}${win ? '\\' : '/'}${rel}`
    const norm = (p: string): string => (win ? p.replace(/\\/g, '/').toLowerCase() : p)
    const existing = config.repos.find((r) => r.sourceId === source.id && norm(r.path) === norm(path))
    if (existing) {
      selectRepo(existing.id)
      return
    }
    void addRepo({
      kind: source.kind,
      ...(source.distro ? { distro: source.distro } : {}),
      ...(source.sshHost ? { sshHost: source.sshHost } : {}),
      path,
      ...(parent.group ? { group: parent.group } : {})
    }).catch((err: unknown) => useAppStore.setState({ error: err instanceof Error ? err.message : String(err) }))
  }
  const solo = (f: ViewRef): void => useLogStore.getState().setFilter(f)
  // ref 행 우클릭 메뉴(현재 HEAD 와 비교 · Solo · 숨기기): 비교는 두 쪽 모두 커밋 해시로 해석된 값을 쓴다
  const [ctx, setCtx] = useState<{ x: number; y: number; hash?: string; view?: ViewRef; row: HTMLElement } | null>(null)
  const closeCtx = useCallback(
    (restore: boolean) =>
      setCtx((c) => {
        if (restore) c?.row.focus()
        return null
      }),
    []
  )
  const headHash = refs?.head.hash ?? null
  const ctxOf = (view: ViewRef, hash?: string) => (e: React.MouseEvent): void => {
    e.preventDefault()
    setCtx({ x: e.clientX, y: e.clientY, hash, view, row: e.currentTarget as HTMLElement })
  }
  const compareWithHead = (hash: string, id?: string): void => {
    closeCtx(false)
    if (!headHash) return
    // Solo·숨김으로 두 커밋이 목록에 없을 수 있으므로 보기를 푼 뒤 연다
    useLogStore.getState().releaseFor(id, [hash, headHash])
    void useLogStore.getState().compareHashes(hash, headHash)
  }
  /** 우클릭 메뉴 항목: (브랜치·태그면) HEAD 와 비교 · Solo · 숨기기 · 모두 보이기 */
  const ctxItems = (c: NonNullable<typeof ctx>): CtxItem[] => {
    const log = useLogStore.getState()
    const v = c.view
    const items: CtxItem[] = []
    if (c.hash) {
      const hash = c.hash
      items.push({
        label: t('compare.withHead'),
        disabled: !headHash || hash === headHash,
        hint: !headHash ? t('compare.noHead') : hash === headHash ? t('compare.headSame') : undefined,
        onPick: () => compareWithHead(hash, v?.id)
      })
    }
    const pick = (fn: () => void) => (): void => {
      closeCtx(false)
      fn()
    }
    if (v) {
      items.push({ label: inList(log.solo, v.id) ? t('view.unsolo') : t('view.soloOnly'), onPick: pick(() => log.toggleSolo(v)) })
      if (canHide(v)) {
        const viaRemote = !inList(log.hide, v.id) && isHidden(log.hide, v.id)
        items.push({
          label: inList(log.hide, v.id) ? t('view.show') : t('view.hide'),
          disabled: viaRemote,
          hint: viaRemote ? t('view.hiddenByRemote') : undefined,
          onPick: pick(() => log.toggleHide(v))
        })
      } else {
        items.push({ label: t('view.hide'), disabled: true, hint: t('view.soloOnlyHint'), onPick: pick(() => {}) })
      }
    }
    if (log.solo.length || log.hide.length) items.push({ label: t('view.showAll'), onPick: pick(() => log.setFilter(null)) })
    return items
  }

  const branchRow = (b: Branch, label: string, depth: number, remote: boolean): ReactNode => {
    const lane = laneIndexFor(branchKey(b.name, remote))
    const view: ViewRef = { id: b.fullName, label: b.name, kind: remote ? 'remote' : 'branch' }
    const tracking =
      b.upstreamGone ? (
        <span className="side-track is-gone" title={t('sidebar.upstreamGone')}>
          ✕
        </span>
      ) : b.ahead || b.behind ? (
        <span className="side-track" title={b.upstream}>
          {b.ahead ? `↑${b.ahead}` : ''}
          {b.behind ? `↓${b.behind}` : ''}
        </span>
      ) : null
    const trailingAll = (
      <>
        <BranchFlowBadge name={b.name} remote={remote} />
        {tracking}
      </>
    )
    return (
      <ItemRow
        key={b.fullName}
        depth={depth}
        icon={
          b.isHead ? (
            <IconCheck size={13} className="side-head-mark" />
          ) : (
            <span className="side-lane-dot" style={{ background: laneVar(lane) }} />
          )
        }
        label={label}
        title={`${b.name}${b.upstream ? ` → ${b.upstream}` : ''}`}
        active={b.isHead}
        trailing={trailingAll}
        onClick={() => reveal(b.hash, b.fullName)}
        view={view}
        onContextMenu={ctxOf(view, b.hash)}
      />
    )
  }

  return (
    <aside className="sidebar" style={{ width }}>
      <div className="sidebar__filter">
        <IconSearch size={13} className="sidebar__filter-icon" />
        <input
          className="sidebar__filter-input"
          placeholder={t('sidebar.filterPlaceholder')}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === 'Escape' && setQuery('')}
        />
        {query && (
          <button className="icon-btn icon-btn--sm" onClick={() => setQuery('')} title={t('search.close')}>
            <IconClose size={12} />
          </button>
        )}
      </div>

      <div className="sidebar__scroll">
        <Section
          id="repos"
          title={t('sidebar.repos')}
          count={config?.repos.length}
          extra={
            <button className="icon-btn icon-btn--sm" title={t('sidebar.addRepo')} onClick={() => openAddDialog()}>
              <IconPlus size={13} />
            </button>
          }
        >
          {repoGroups.length === 0 && <div className="sidebar__empty">{t('sidebar.empty')}</div>}
          {repoGroups.map(({ source, repos, groups, allGroups: sourceGroups }) => {
            // 같은 환경 안에서는 사용자 그룹별로 접을 수 있는 폴더로 묶는다(그룹 없는 저장소는 맨 아래)
            const grouped = new Map<string, Repo[]>()
            const loose: Repo[] = []
            for (const r of repos) {
              if (r.group) grouped.set(r.group, [...(grouped.get(r.group) ?? []), r])
              else loose.push(r)
            }
            // 저장소는 같은 환경·같은 그룹 안에서만 끌어 옮긴다
            const repoRow = (repo: Repo, depth: number, siblings: Repo[]): ReactNode => {
              const scope = `${source.id}\0${repo.group ?? ''}`
              const ids = siblings.map((r) => r.id)
              return (
                <ItemRow
                  key={repo.id}
                  depth={depth}
                  icon={<IconRepo size={13} />}
                  label={repo.name}
                  title={`${source.label} · ${repo.path}`}
                  active={repo.id === activeRepoId}
                  onClick={() => selectRepo(repo.id)}
                  drag={{
                    // 메뉴가 열려 있으면 그 안의 입력칸을 끌지 않게 드래그를 끈다
                    props: { ...(menuFor === repo.id ? {} : dnd.handle('repo', scope, repo.id)), ...dnd.target('repo', scope, repo.id, ids, moveRepos) },
                    cls: dnd.cls('repo', scope, repo.id)
                  }}
                  trailing={
                    <>
                      <button
                        className="side-item__action"
                        title={t('sidebar.repoMenu')}
                        // 열린 메뉴의 '바깥 누름 → 닫기' 뒤에 click 이 다시 여는 것을 막는다
                        onMouseDown={(e) => e.stopPropagation()}
                        onClick={(e) => {
                          e.stopPropagation()
                          setMenuFor(menuFor === repo.id ? null : repo.id)
                        }}
                      >
                        <IconMore size={12} />
                      </button>
                      {menuFor === repo.id && (
                        <RepoMenu
                          repo={repo}
                          sourceKind={source.kind}
                          groups={allGroups}
                          onClose={() => setMenuFor(null)}
                          onGroup={(group) => void updateRepo(repo.id, { group })}
                          onRemove={() => void removeRepo(repo.id)}
                        />
                      )}
                    </>
                  }
                />
              )
            }
            // 환경(Local·WSL 배포판·SSH 호스트)마다 접을 수 있는 카드로 감싼다. 머리를 끌어 카드 순서를 바꾼다
            const srcKey = `src:${source.id}`
            const srcOpen = isOpen(srcKey)
            const colorOpen = srcColorFor === source.id
            return (
              <div
                key={source.id}
                className={`source-card${dnd.cls('source', '', source.id)}`}
                {...dnd.target('source', '', source.id, repoGroups.map((g) => g.source.id), moveSources)}
              >
                <div
                  className="source-card__head"
                  role="button"
                  tabIndex={0}
                  aria-expanded={srcOpen}
                  title={`${source.label} · ${t('sidebar.dragHint')}`}
                  {...(colorOpen ? {} : dnd.handle('source', '', source.id))}
                  onClick={() => toggle(srcKey)}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    setSrcColorFor(source.id)
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault()
                      toggle(srcKey)
                    }
                  }}
                >
                  <IconChevronRight size={12} className={`side-chev${srcOpen ? ' is-open' : ''}`} />
                  <EnvBadge kind={source.kind} sourceId={source.id} />
                  <span className="source-card__label">{source.label}</span>
                  <ColorAction
                    open={colorOpen}
                    setOpen={(v) => setSrcColorFor(v ? source.id : null)}
                    value={config?.ui.sourceColors?.[source.id]}
                    allowDefault
                    onPick={(hue) => {
                      const { [source.id]: _old, ...rest } = config?.ui.sourceColors ?? {}
                      void useAppStore.getState().setUi({ sourceColors: hue === undefined ? rest : { ...rest, [source.id]: hue } })
                    }}
                  />
                  <span className="side-count">{repos.length}</span>
                </div>
                <Collapse open={srcOpen}>
                  <div className="source-card__body">
                    {groups.map((group) => {
                      const items = grouped.get(group) ?? []
                      const key = `repogroup:${source.id}:${group}`
                      const open = isOpen(key)
                      return (
                        <div
                          key={group}
                          className={`side-group${dnd.cls('group', source.id, group)}`}
                          {...dnd.target('group', source.id, group, groups, (next) => moveGroups(source.id, sourceGroups, next))}
                        >
                          <GroupRow name={group} count={items.length} open={open} onToggle={() => toggle(key)} drag={{ props: dnd.handle('group', source.id, group), cls: '' }} />
                          <Collapse open={open}>{items.map((r) => repoRow(r, 1, items))}</Collapse>
                        </div>
                      )
                    })}
                    {loose.map((r) => repoRow(r, 0, loose))}
                  </div>
                </Collapse>
              </div>
            )
          })}
        </Section>

        {refs && (
          <>
            <Section id="local" title={t('sidebar.local')} count={local.length}>
              <Tree node={buildTree(local.map((b) => ({ name: b.name, value: b })))} scope={scoped("local")} depth={0} renderItem={(it, d) => branchRow(it.value, it.label, d, false)} />
            </Section>

            <Section id="remote" title={t('sidebar.remote')} count={remotes.reduce((n, r) => n + r.branches.length, 0)}>
              {remotes.map((r) => {
                const key = scoped(`remote:${r.name}`)
                const open = isOpen(key) || !!q
                return (
                  <div key={r.name}>
                    <RemoteRow name={r.name} count={r.branches.length} open={open} onToggle={() => toggle(key)} onContextMenu={ctxOf(remoteAllRef(r.name))} />
                    <Collapse open={open}>
                      <Tree
                        node={buildTree(r.branches.map((b) => ({ name: b.name.slice(r.name.length + 1), value: b })))}
                        scope={key}
                        depth={1}
                        renderItem={(it, d) => branchRow(it.value, it.label, d, true)}
                      />
                    </Collapse>
                  </div>
                )
              })}
            </Section>

            {ghRef && (
              <>
                <Section
                  id="pulls"
                  title={t('gh.pulls')}
                  count={ghPulls.items.length}
                  extra={
                    <button className="icon-btn icon-btn--sm" title={t('gh.viewAll')} onClick={() => ghShow({ kind: 'pulls' })}>
                      <IconMore size={13} />
                    </button>
                  }
                >
                  {ghPulls.error && <div className="sidebar__empty">{ghPulls.error}</div>}
                  {!ghPulls.error && ghPulls.items.length === 0 && !ghPulls.loading && <div className="sidebar__empty">{t('gh.empty')}</div>}
                  {ghPulls.items
                    .filter((p) => matches(q, p.title, `#${p.number}`, p.author.login))
                    .slice(0, SIDEBAR_GH_MAX)
                    .map((p) => (
                      <ItemRow
                        key={p.number}
                        depth={0}
                        icon={<GhStateIcon kind="pull" state={prState(p)} size={13} />}
                        label={p.title}
                        title={`#${p.number} ${p.title} — ${p.author.login}`}
                        active={ghView?.kind === 'pull' && ghView.number === p.number}
                        onClick={() => ghShow({ kind: 'pull', number: p.number, tab: 'conversation' })}
                        trailing={
                          <>
                            <ChecksBadge state={p.checks} />
                            <span className="side-sub">#{p.number}</span>
                          </>
                        }
                      />
                    ))}
                </Section>

                <Section
                  id="issues"
                  title={t('gh.issues')}
                  count={ghIssues.items.length}
                  extra={
                    <button className="icon-btn icon-btn--sm" title={t('gh.viewAll')} onClick={() => ghShow({ kind: 'issues' })}>
                      <IconMore size={13} />
                    </button>
                  }
                >
                  {ghIssues.error && <div className="sidebar__empty">{ghIssues.error}</div>}
                  {!ghIssues.error && ghIssues.items.length === 0 && !ghIssues.loading && <div className="sidebar__empty">{t('gh.empty')}</div>}
                  {ghIssues.items
                    .filter((i) => matches(q, i.title, `#${i.number}`, i.author.login))
                    .slice(0, SIDEBAR_GH_MAX)
                    .map((i) => (
                      <ItemRow
                        key={i.number}
                        depth={0}
                        icon={<GhStateIcon kind="issue" state={i.state} size={13} />}
                        label={i.title}
                        title={`#${i.number} ${i.title} — ${i.author.login}`}
                        active={ghView?.kind === 'issue' && ghView.number === i.number}
                        onClick={() => ghShow({ kind: 'issue', number: i.number })}
                        trailing={<span className="side-sub">#{i.number}</span>}
                      />
                    ))}
                </Section>

                <Section id="actions" title={t('gh.actions')}>
                  <ItemRow
                    depth={0}
                    icon={<IconCheck size={13} />}
                    label={t('gh.actions.sidebar')}
                    active={ghView?.kind === 'actions'}
                    onClick={() => ghShow({ kind: 'actions' })}
                  />
                </Section>
              </>
            )}

            <Section
              id="jira"
              title={t('jira.mine')}
              count={jiraAuth.connected ? jiraMine.items.length : undefined}
              extra={
                <>
                  {jiraAuth.connected && (
                    <button className="icon-btn icon-btn--sm" title={t('jira.viewAll')} onClick={() => useJiraStore.getState().show({ kind: 'list', source: 'mine' })}>
                      <IconMore size={13} />
                    </button>
                  )}
                  <button className="icon-btn icon-btn--sm" title={t('jira.title')} onClick={() => useJiraStore.getState().setDialogOpen(true)}>
                    <IconCloud size={13} />
                  </button>
                </>
              }
            >
              {!jiraAuth.connected && (
                <div className="sidebar__empty">
                  {t('jira.notConnected')}{' '}
                  <button className="btn btn--sm" onClick={() => useJiraStore.getState().setDialogOpen(true)}>
                    {t('jira.connect')}
                  </button>
                </div>
              )}
              {jiraAuth.connected && jiraMine.error && <div className="sidebar__empty">{jiraMine.error}</div>}
              {jiraAuth.connected && !jiraMine.error && jiraMine.items.length === 0 && !jiraMine.loading && <div className="sidebar__empty">{t('gh.empty')}</div>}
              {jiraAuth.connected &&
                jiraMine.items
                  .filter((i) => matches(q, i.summary, i.key))
                  .slice(0, SIDEBAR_GH_MAX)
                  .map((i) => (
                    <ItemRow
                      key={i.key}
                      depth={0}
                      icon={<span className={`jira-dot jira-dot--${i.statusCategory}`} />}
                      label={i.summary}
                      title={`${i.key} ${i.summary} — ${i.status}`}
                      active={jiraView?.kind === 'issue' && jiraView.key === i.key}
                      onClick={() => useJiraStore.getState().show({ kind: 'issue', key: i.key, from: { kind: 'list', source: 'mine' } })}
                      trailing={<span className="side-sub">{i.key}</span>}
                    />
                  ))}
            </Section>

            {jiraAuth.connected && (
              <>
                <Section id="jira-recent" title={t('jira.recent')} count={jiraRecent.length}>
                  {jiraRecent.length === 0 && <div className="sidebar__empty">{t('jira.recentEmpty')}</div>}
                  {jiraRecent
                    .filter((r) => matches(q, r.summary, r.key))
                    .map((r) => (
                      <ItemRow
                        key={r.key}
                        depth={0}
                        icon={<IconSearch size={13} />}
                        label={r.summary}
                        title={`${r.key} ${r.summary}`}
                        active={jiraView?.kind === 'issue' && jiraView.key === r.key}
                        onClick={() => useJiraStore.getState().openIssue(r.key)}
                        trailing={<span className="side-sub">{r.key}</span>}
                      />
                    ))}
                </Section>

                <Section
                  id="jira-filters"
                  title={t('jira.filters')}
                  count={jiraFilters.length}
                  extra={
                    <button className="icon-btn icon-btn--sm" title={t('jira.newFilter')} onClick={() => useJiraStore.getState().show({ kind: 'list', source: 'custom' })}>
                      <IconPlus size={13} />
                    </button>
                  }
                >
                  {jiraFilters.length === 0 && <div className="sidebar__empty">{t('jira.filtersEmpty')}</div>}
                  {jiraFilters.map((f, n) => (
                    <ItemRow
                      key={f.name}
                      depth={0}
                      icon={<IconFunnel size={13} />}
                      label={f.name}
                      title={f.jql}
                      active={jiraView?.kind === 'list' && jiraView.source === n}
                      onClick={() => useJiraStore.getState().show({ kind: 'list', source: n })}
                      trailing={
                        <button
                          className="side-item__action"
                          title={t('jira.removeFilter')}
                          onClick={(e) => {
                            e.stopPropagation()
                            useJiraStore.getState().removeFilter(n)
                          }}
                        >
                          <IconClose size={11} />
                        </button>
                      }
                    />
                  ))}
                </Section>
              </>
            )}

            <Section id="worktrees" title={t('sidebar.worktrees')} count={worktrees.length}>
              {worktrees.map((w: Worktree) => {
                const base = w.path.split(/[\\/]/).filter(Boolean).pop() ?? w.path
                const label = w.branch ?? (w.head ? w.head.slice(0, 7) : t('sidebar.noCommits'))
                const wtView: ViewRef | undefined = w.head ? { id: w.head, label: `${base} (${label})`, kind: 'commit' } : undefined
                return (
                  <ItemRow
                    key={w.path}
                    depth={0}
                    icon={w.isMain ? <IconMonitor size={13} /> : <IconWorktree size={13} />}
                    label={base}
                    title={w.path}
                    muted={w.prunable}
                    view={wtView}
                    trailing={<span className="side-sub">{label}</span>}
                    onClick={() => wtView && solo(wtView)}
                    onContextMenu={wtView ? ctxOf(wtView) : undefined}
                  />
                )
              })}
            </Section>

            <Section id="stashes" title={t('sidebar.stashes')} count={stashes.length}>
              {stashes.map((s: Stash) => {
                const view: ViewRef = { id: s.hash, label: s.ref, kind: 'stash' }
                return (
                  <ItemRow
                    key={s.hash}
                    depth={0}
                    icon={<IconStash size={13} />}
                    label={s.message}
                    title={`${s.ref} · ${s.message}`}
                    view={view}
                    trailing={<span className="side-sub">{formatDate(s.date, locale)}</span>}
                    onClick={() => solo(view)}
                    onContextMenu={ctxOf(view)}
                  />
                )
              })}
            </Section>

            <Section id="tags" title={t('sidebar.tags')} count={tags.length}>
              <Tree
                node={buildTree(tags.map((x) => ({ name: x.name, value: x })))}
                scope={scoped("tags")}
                depth={0}
                renderItem={(it: { label: string; value: Tag }, d) => (
                  <ItemRow
                    key={it.value.fullName}
                    depth={d}
                    icon={<IconTag size={13} />}
                    label={it.label}
                    title={it.value.name}
                    muted={!it.value.targetsCommit}
                    onClick={() => it.value.targetsCommit && reveal(it.value.hash, it.value.fullName)}
                    view={it.value.targetsCommit ? { id: it.value.fullName, label: it.value.name, kind: 'tag' } : undefined}
                    onContextMenu={
                      it.value.targetsCommit ? ctxOf({ id: it.value.fullName, label: it.value.name, kind: 'tag' }, it.value.hash) : undefined
                    }
                  />
                )}
              />
            </Section>

            <Section id="submodules" title={t('sidebar.submodules')} count={submodules.length}>
              {submodules.length === 0 && <div className="sidebar__empty">{t('sidebar.submoduleEmpty')}</div>}
              {submodules.map((m) => {
                const uninit = m.status === 'uninitialized'
                const modified = m.status === 'modified'
                const short = (h?: string): string => (h ? h.slice(0, 7) : '')
                const state = uninit
                  ? t('sidebar.submoduleUninitTip')
                  : modified
                    ? t('sidebar.submoduleModifiedTip', { recorded: short(m.recorded), head: short(m.head) })
                    : m.status === 'unknown'
                      ? t('sidebar.submoduleUnknown')
                      : t('sidebar.submoduleOpen')
                return (
                  <ItemRow
                    key={m.path}
                    depth={0}
                    icon={<IconSubmodule size={13} />}
                    label={m.name}
                    title={[m.path, m.url, state].filter(Boolean).join('\n')}
                    muted={uninit}
                    disabled={uninit}
                    trailing={
                      uninit ? (
                        <span className="side-badge">{t('sidebar.submoduleUninit')}</span>
                      ) : modified ? (
                        <span className="side-badge is-modified">{t('sidebar.submoduleModified')}</span>
                      ) : undefined
                    }
                    onClick={uninit ? undefined : () => openSubmodule(m)}
                  />
                )
              })}
            </Section>
          </>
        )}
      </div>
      {ctx && (
        <RefMenu x={ctx.x} y={ctx.y} items={ctxItems(ctx)} onClose={closeCtx} />
      )}
    </aside>
  )
}

