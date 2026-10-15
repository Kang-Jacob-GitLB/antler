import { useMemo, useState, type ReactNode } from 'react'
import type { Branch, Repo, Source, Stash, Tag, Worktree } from '@shared/types'
import { useT } from '../i18n'
import { formatDate } from '../lib/format'
import { branchKey, laneIndexFor, laneVar } from '../lib/laneColor'
import { useAppStore } from '../stores/appStore'
import { useLogStore, type LogFilter } from '../stores/logStore'
import { EnvBadge } from './EnvBadge'
import { RepoMenu } from './RepoMenu'
import { ChecksBadge, GhStateIcon, prState } from './github/GhBits'
import { useGithubStore } from '../stores/githubStore'
import {
  IconCheck,
  IconChevronRight,
  IconClose,
  IconCloud,
  IconFolder,
  IconFunnel,
  IconMore,
  IconMonitor,
  IconPlus,
  IconRepo,
  IconSearch,
  IconStash,
  IconTag,
  IconWorktree
} from './icons'
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
  trailing?: ReactNode
  onClick?(): void
  onFilter?(): void
}

function ItemRow({ depth, icon, label, title, active, muted, trailing, onClick, onFilter }: RowProps): React.JSX.Element {
  const t = useT()
  return (
    <div
      role="button"
      tabIndex={0}
      className={`side-item${active ? ' is-active' : ''}${muted ? ' is-muted' : ''}`}
      style={{ paddingLeft: 8 + depth * 14 }}
      title={title ?? label}
      onClick={onClick}
      onDoubleClick={onFilter}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onClick?.()
      }}
    >
      <span className="side-item__icon">{icon}</span>
      <span className="side-item__label">{label}</span>
      {trailing}
      {onFilter && (
        <button
          className="side-item__action"
          title={t('sidebar.filter')}
          onClick={(e) => {
            e.stopPropagation()
            onFilter()
          }}
        >
          <IconFunnel size={12} />
        </button>
      )}
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

function Section({
  id,
  title,
  count,
  children,
  extra
}: {
  id: string
  title: string
  count?: number
  children: ReactNode
  extra?: ReactNode
}): React.JSX.Element {
  const { isOpen, toggle } = useCollapse()
  const open = isOpen(`sec:${id}`)
  return (
    <section className="side-section">
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
      {open && <div className="side-section__body">{children}</div>}
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
            {open && <Tree node={child} scope={scope} depth={depth + 1} renderItem={renderItem} />}
          </div>
        )
      })}
      {node.items.map((item) => renderItem(item, depth))}
    </>
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
  const { selectRepo, openAddDialog, removeRepo, updateRepo } = useAppStore.getState()
  const filter = useLogStore((s) => s.filter)
  const [query, setQuery] = useState('')
  const q = query.trim().toLowerCase()
  const { isOpen, toggle } = useCollapse()
  /** 메뉴(그룹 지정·제거)를 연 저장소 */
  const [menuFor, setMenuFor] = useState<string | null>(null)
  // GitHub: remote 가 GitHub 저장소일 때만 PR·이슈 섹션을 보인다
  const ghRef = useGithubStore((s) => s.ref)
  const ghPulls = useGithubStore((s) => s.pulls)
  const ghIssues = useGithubStore((s) => s.issues)
  const ghView = useGithubStore((s) => s.view)
  const ghShow = useGithubStore((s) => s.show)
  const allGroups = useMemo(
    () => [...new Set((config?.repos ?? []).map((r) => r.group).filter((g): g is string => !!g))].sort(),
    [config]
  )

  const repoGroups = useMemo(() => {
    if (!config) return []
    const bySource = new Map<string, { source: Source; repos: Repo[] }>()
    for (const s of config.sources) bySource.set(s.id, { source: s, repos: [] })
    for (const r of config.repos) bySource.get(r.sourceId)?.repos.push(r)
    return [...bySource.values()]
      .filter((g) => g.repos.length)
      .map((g) => ({ ...g, repos: [...g.repos].sort((a, b) => a.name.localeCompare(b.name)) }))
  }, [config])

  const local = useMemo(() => (refs?.local ?? []).filter((b) => matches(q, b.name)), [refs, q])
  const remotes = useMemo(
    () => (refs?.remotes ?? []).map((r) => ({ ...r, branches: r.branches.filter((b) => matches(q, b.name)) })).filter((r) => r.branches.length || !q),
    [refs, q]
  )
  const tags = useMemo(() => (refs?.tags ?? []).filter((x) => matches(q, x.name)), [refs, q])
  const stashes = useMemo(() => (refs?.stashes ?? []).filter((s) => matches(q, s.message, s.ref)), [refs, q])
  const worktrees = useMemo(() => (refs?.worktrees ?? []).filter((w) => matches(q, w.path, w.branch ?? undefined)), [refs, q])

  // 필터로 다른 ref 만 보는 중이면 찾는 커밋이 목록에 없을 수 있으므로 전체 보기로 돌아간 뒤 찾아간다
  const reveal = (hash: string): void => {
    const log = useLogStore.getState()
    if (log.filter) log.setFilter(null)
    void useLogStore.getState().reveal(hash)
  }
  // 폴더·리모트 접힘 키는 저장소마다 따로 둔다
  const scoped = (key: string): string => `${activeRepoId}:${key}`
  const solo = (f: LogFilter): void => useLogStore.getState().setFilter(f)
  const isFiltered = (rev: string): boolean => filter?.rev === rev

  const branchRow = (b: Branch, label: string, depth: number, remote: boolean): ReactNode => {
    const lane = laneIndexFor(branchKey(b.name, remote))
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
        active={isFiltered(b.fullName) || b.isHead}
        trailing={tracking}
        onClick={() => reveal(b.hash)}
        onFilter={() => solo({ rev: b.fullName, label: b.name })}
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
          {repoGroups.map(({ source, repos }) => {
            // 같은 환경 안에서는 사용자 그룹별로 접을 수 있는 폴더로 묶는다(그룹 없는 저장소는 맨 아래)
            const grouped = new Map<string, Repo[]>()
            const loose: Repo[] = []
            for (const r of repos) {
              if (r.group) grouped.set(r.group, [...(grouped.get(r.group) ?? []), r])
              else loose.push(r)
            }
            const repoRow = (repo: Repo, depth: number): ReactNode => (
              <ItemRow
                key={repo.id}
                depth={depth}
                icon={<IconRepo size={13} />}
                label={repo.name}
                title={`${source.label} · ${repo.path}`}
                active={repo.id === activeRepoId}
                onClick={() => selectRepo(repo.id)}
                trailing={
                  <>
                    <EnvBadge kind={source.kind} />
                    <button
                      className="side-item__action"
                      title={t('sidebar.repoMenu')}
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
            return (
              <div key={source.id}>
                {(repoGroups.length > 1 || source.kind !== 'local') && (
                  <div className="sidebar__group-label">{source.label}</div>
                )}
                {[...grouped.entries()]
                  .sort(([a], [b]) => a.localeCompare(b))
                  .map(([group, items]) => {
                    const key = `repogroup:${source.id}:${group}`
                    const open = isOpen(key)
                    return (
                      <div key={group}>
                        <FolderRow depth={0} name={group} count={items.length} open={open} onToggle={() => toggle(key)} />
                        {open && items.map((r) => repoRow(r, 1))}
                      </div>
                    )
                  })}
                {loose.map((r) => repoRow(r, 0))}
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
                    <div role="button" tabIndex={0} className="side-item side-item--folder" style={{ paddingLeft: 8 }} onClick={() => toggle(key)}>
                      <IconChevronRight size={12} className={`side-chev${open ? ' is-open' : ''}`} />
                      <IconCloud size={13} className="side-item__icon" />
                      <span className="side-item__label">{r.name}</span>
                      <span className="side-count">{r.branches.length}</span>
                    </div>
                    {open && (
                      <Tree
                        node={buildTree(r.branches.map((b) => ({ name: b.name.slice(r.name.length + 1), value: b })))}
                        scope={key}
                        depth={1}
                        renderItem={(it, d) => branchRow(it.value, it.label, d, true)}
                      />
                    )}
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
              </>
            )}

            <Section id="worktrees" title={t('sidebar.worktrees')} count={worktrees.length}>
              {worktrees.map((w: Worktree) => {
                const base = w.path.split(/[\\/]/).filter(Boolean).pop() ?? w.path
                const label = w.branch ?? (w.head ? w.head.slice(0, 7) : t('sidebar.noCommits'))
                return (
                  <ItemRow
                    key={w.path}
                    depth={0}
                    icon={w.isMain ? <IconMonitor size={13} /> : <IconWorktree size={13} />}
                    label={base}
                    title={w.path}
                    muted={w.prunable}
                    active={!!w.head && isFiltered(w.head)}
                    trailing={<span className="side-sub">{label}</span>}
                    onClick={() => w.head && solo({ rev: w.head, label: `${base} (${label})` })}
                  />
                )
              })}
            </Section>

            <Section id="stashes" title={t('sidebar.stashes')} count={stashes.length}>
              {stashes.map((s: Stash) => (
                <ItemRow
                  key={s.hash}
                  depth={0}
                  icon={<IconStash size={13} />}
                  label={s.message}
                  title={`${s.ref} · ${s.message}`}
                  active={isFiltered(s.hash)}
                  trailing={<span className="side-sub">{formatDate(s.date, locale)}</span>}
                  onClick={() => solo({ rev: s.hash, label: s.ref })}
                />
              ))}
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
                    active={isFiltered(it.value.fullName)}
                    muted={!it.value.targetsCommit}
                    onClick={() => it.value.targetsCommit && reveal(it.value.hash)}
                    onFilter={it.value.targetsCommit ? () => solo({ rev: it.value.fullName, label: it.value.name }) : undefined}
                  />
                )}
              />
            </Section>
          </>
        )}
      </div>
    </aside>
  )
}

