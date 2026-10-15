import { useMemo } from 'react'
import type { GhComment, GhIssueSummary, GhListQuery, GhPullFile, GhPullSummary } from '@shared/types'
import { fileDiffFromPatch } from '../../diff/patch'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { useGithubStore, type GhView } from '../../stores/githubStore'
import { DiffBody, DiffModeToggle } from '../DiffView'
import { FileStatusIcon } from '../FileStatusIcon'
import { IconClose, IconRefresh } from '../icons'
import { Avatar, ChecksBadge, GhStateIcon, HtmlBody, LabelChip, prState, ReviewBadge, useRelative } from './GhBits'
import './GitHubView.css'

const openOnGithub = (url: string): void => void api.invoke('gh:openExternal', url).catch(() => {})

/** GitHub PR 파일 상태 → 앱 공용 변경 종류 */
const FILE_STATUS: Record<GhPullFile['status'], 'A' | 'M' | 'D' | 'R' | 'C'> = {
  added: 'A',
  removed: 'D',
  modified: 'M',
  renamed: 'R',
  copied: 'C',
  changed: 'M',
  unchanged: 'M'
}

export function GitHubView(): React.JSX.Element | null {
  const view = useGithubStore((s) => s.view)
  if (!view) return null
  return (
    <section className="gh-view">
      {view.kind === 'pulls' || view.kind === 'issues' ? <ListView kind={view.kind} /> : view.kind === 'pull' ? <PullView view={view} /> : <IssueView number={view.number} />}
    </section>
  )
}

// ── 목록 ───────────────────────────────────────────────

function ListView({ kind }: { kind: 'pulls' | 'issues' }): React.JSX.Element {
  const t = useT()
  const { ref, filters, show, loadList } = useGithubStore()
  const list = useGithubStore((s) => s[kind])
  const rel = useRelative()
  const q = list.query
  const setQ = (patch: Partial<GhListQuery>): void => void loadList(kind, patch)

  return (
    <>
      <header className="gh-head">
        <div className="seg" role="tablist">
          {(['pulls', 'issues'] as const).map((k) => (
            <button key={k} role="tab" aria-selected={kind === k} className={`seg__btn${kind === k ? ' is-on' : ''}`} onClick={() => show({ kind: k })}>
              {t(k === 'pulls' ? 'gh.pulls' : 'gh.issues')}
            </button>
          ))}
        </div>
        <span className="gh-head__repo mono">{ref ? `${ref.owner}/${ref.repo}` : ''}</span>
        <span className="gh-head__spacer" />
        <button className="icon-btn icon-btn--sm" title={t('toolbar.refresh')} onClick={() => setQ({})}>
          <IconRefresh size={14} />
        </button>
        <button className="icon-btn icon-btn--sm" title={`${t('diff.close')} (Esc)`} onClick={() => show(null)}>
          <IconClose size={13} />
        </button>
      </header>

      <div className="gh-filters">
        <div className="seg" role="radiogroup" aria-label={t('gh.filter.state')}>
          {(['open', 'closed', 'all'] as const).map((s) => (
            <button key={s} role="radio" aria-checked={q.state === s} className={`seg__btn${q.state === s ? ' is-on' : ''}`} onClick={() => setQ({ state: s })}>
              {t(`gh.filter.${s}`)}
            </button>
          ))}
        </div>
        <select className="gh-select" value={q.label ?? ''} onChange={(e) => setQ({ label: e.target.value || undefined })} aria-label={t('gh.filter.label')}>
          <option value="">{t('gh.filter.anyLabel')}</option>
          {filters?.labels.map((l) => (
            <option key={l.name} value={l.name}>
              {l.name}
            </option>
          ))}
        </select>
        <select className="gh-select" value={q.assignee ?? ''} onChange={(e) => setQ({ assignee: e.target.value || undefined })} aria-label={t('gh.filter.assignee')}>
          <option value="">{t('gh.filter.anyAssignee')}</option>
          <option value="none">{t('gh.filter.noAssignee')}</option>
          {filters?.assignees.map((u) => (
            <option key={u.login} value={u.login}>
              {u.login}
            </option>
          ))}
        </select>
      </div>

      <div className="gh-list">
        {list.error && <div className="gh-notice is-error">{list.error}</div>}
        {!list.error && !list.loading && list.items.length === 0 && <div className="gh-notice">{t('gh.empty')}</div>}
        {list.items.map((it) =>
          kind === 'pulls' ? (
            <PullRow key={it.number} p={it as GhPullSummary} rel={rel} onOpen={() => show({ kind: 'pull', number: it.number, tab: 'conversation' })} />
          ) : (
            <IssueRow key={it.number} i={it as GhIssueSummary} rel={rel} onOpen={() => show({ kind: 'issue', number: it.number })} />
          )
        )}
        {list.loading && <div className="gh-notice">{t('list.loading')}</div>}
        {list.hasMore && !list.loading && (
          <button className="btn gh-more" onClick={() => void loadList(kind, {}, true)}>
            {t('gh.more')}
          </button>
        )}
      </div>
    </>
  )
}

function PullRow({ p, rel, onOpen }: { p: GhPullSummary; rel: (n: number) => string; onOpen(): void }): React.JSX.Element {
  const t = useT()
  return (
    <div className="gh-row" role="button" tabIndex={0} onClick={onOpen} onKeyDown={(e) => e.key === 'Enter' && onOpen()}>
      <GhStateIcon kind="pull" state={prState(p)} />
      <div className="gh-row__main">
        <div className="gh-row__title">
          <span className="gh-row__text">{p.title}</span>
          {p.labels.map((l) => (
            <LabelChip key={l.name} label={l} />
          ))}
        </div>
        <div className="gh-row__sub">
          #{p.number} · {t('gh.openedBy', { who: p.author.login, when: rel(p.createdAt) })} · <span className="mono">{p.headRef}</span> → <span className="mono">{p.baseRef}</span>
        </div>
      </div>
      <ChecksBadge state={p.checks} />
      <ReviewBadge state={p.review} />
      <div className="gh-row__people">
        {p.assignees.slice(0, 3).map((u) => (
          <Avatar key={u.login} user={u} />
        ))}
      </div>
      {p.comments > 0 && <span className="gh-row__count">💬 {p.comments}</span>}
      <span className="gh-row__when">{rel(p.updatedAt)}</span>
    </div>
  )
}

function IssueRow({ i, rel, onOpen }: { i: GhIssueSummary; rel: (n: number) => string; onOpen(): void }): React.JSX.Element {
  const t = useT()
  return (
    <div className="gh-row" role="button" tabIndex={0} onClick={onOpen} onKeyDown={(e) => e.key === 'Enter' && onOpen()}>
      <GhStateIcon kind="issue" state={i.state} />
      <div className="gh-row__main">
        <div className="gh-row__title">
          <span className="gh-row__text">{i.title}</span>
          {i.labels.map((l) => (
            <LabelChip key={l.name} label={l} />
          ))}
        </div>
        <div className="gh-row__sub">
          #{i.number} · {t('gh.openedBy', { who: i.author.login, when: rel(i.createdAt) })}
        </div>
      </div>
      <div className="gh-row__people">
        {i.assignees.slice(0, 3).map((u) => (
          <Avatar key={u.login} user={u} />
        ))}
      </div>
      {i.comments > 0 && <span className="gh-row__count">💬 {i.comments}</span>}
      <span className="gh-row__when">{rel(i.updatedAt)}</span>
    </div>
  )
}

// ── 상세 공용 ───────────────────────────────────────────

function DetailHead({ title, number, url, state, kind, back }: { title: string; number: number; url?: string; state: 'open' | 'closed' | 'merged' | 'draft'; kind: 'pull' | 'issue'; back: GhView }): React.JSX.Element {
  const t = useT()
  const show = useGithubStore((s) => s.show)
  return (
    <header className="gh-head">
      <button className="btn btn--sm" onClick={() => show(back)}>
        ← {t(kind === 'pull' ? 'gh.pulls' : 'gh.issues')}
      </button>
      <GhStateIcon kind={kind} state={state} size={16} />
      <h2 className="gh-head__title selectable">
        {title} <span className="gh-head__num">#{number}</span>
      </h2>
      {url && (
        <button className="btn btn--sm" onClick={() => openOnGithub(url)}>
          {t('gh.openInBrowser')}
        </button>
      )}
      <button className="icon-btn icon-btn--sm" title={`${t('diff.close')} (Esc)`} onClick={() => show(null)}>
        <IconClose size={13} />
      </button>
    </header>
  )
}

function Comments({ items }: { items: GhComment[] | null }): React.JSX.Element | null {
  const rel = useRelative()
  if (!items) return null
  return (
    <>
      {items.map((c, i) => (
        <article key={i} className="gh-comment">
          <header className="gh-comment__head">
            <Avatar user={c.author} size={20} />
            <strong>{c.author.login}</strong>
            <span className="gh-comment__when">{rel(c.createdAt)}</span>
          </header>
          <HtmlBody html={c.bodyHtml} />
        </article>
      ))}
    </>
  )
}

// ── PR 상세 ─────────────────────────────────────────────

function PullView({ view }: { view: Extract<GhView, { kind: 'pull' }> }): React.JSX.Element {
  const t = useT()
  const { pull, show, pullFile, selectPullFile } = useGithubStore()
  const rel = useRelative()
  const d = pull?.number === view.number ? pull.detail : null
  const files = pull?.number === view.number ? pull.files : null
  const file = files?.find((f) => f.path === pullFile) ?? null
  const fileDiff = useMemo(() => (file ? fileDiffFromPatch(file.path, file.patch, file.oldPath) : null), [file])

  return (
    <>
      <DetailHead title={d?.title ?? '…'} number={view.number} url={d?.htmlUrl} state={d ? prState(d) : 'open'} kind="pull" back={{ kind: 'pulls' }} />
      {d && (
        <div className="gh-meta">
          <Avatar user={d.author} />
          <span>{t('gh.wantsToMerge', { who: d.author.login, n: d.changedFiles })}</span>
          <span className="mono gh-ref">{d.headRef}</span>→<span className="mono gh-ref">{d.baseRef}</span>
          <span className="is-add">+{d.additions}</span>
          <span className="is-del">−{d.deletions}</span>
          <ChecksBadge state={d.checks} />
          <ReviewBadge state={d.review} />
        </div>
      )}
      <div className="gh-tabs" role="tablist">
        {(['conversation', 'commits', 'files'] as const).map((tab) => (
          <button key={tab} role="tab" aria-selected={view.tab === tab} className={`gh-tab${view.tab === tab ? ' is-on' : ''}`} onClick={() => show({ ...view, tab })}>
            {t(`gh.tab.${tab}`)}
            {tab === 'commits' && pull?.commits && <span className="gh-tab__n">{pull.commits.length}</span>}
            {tab === 'files' && d && <span className="gh-tab__n">{d.changedFiles}</span>}
          </button>
        ))}
        {view.tab === 'files' && (
          <span className="gh-tabs__extra">
            <DiffModeToggle />
          </span>
        )}
      </div>

      {pull?.error && <div className="gh-notice is-error">{pull.error}</div>}

      {view.tab === 'conversation' && (
        <div className="gh-scroll">
          {!d ? (
            <div className="gh-notice">{t('list.loading')}</div>
          ) : (
            <>
              <article className="gh-comment">
                <header className="gh-comment__head">
                  <Avatar user={d.author} size={20} />
                  <strong>{d.author.login}</strong>
                  <span className="gh-comment__when">{rel(d.createdAt)}</span>
                </header>
                {d.bodyHtml ? <HtmlBody html={d.bodyHtml} /> : <p className="gh-muted">{t('gh.noDescription')}</p>}
              </article>
              {(d.checkRuns.length > 0 || d.reviews.length > 0 || d.requestedReviewers.length > 0) && (
                <div className="gh-status">
                  {d.reviews.map((r, i) => (
                    <div key={`r${i}`} className="gh-status__row">
                      <Avatar user={r.author} />
                      <span>{r.author.login}</span>
                      <ReviewBadge state={r.state} />
                      <span className="gh-muted">{t(`gh.review.${r.state}`)}</span>
                    </div>
                  ))}
                  {d.requestedReviewers.map((u) => (
                    <div key={`q${u.login}`} className="gh-status__row">
                      <Avatar user={u} />
                      <span>{u.login}</span>
                      <span className="gh-muted">{t('gh.review.pending')}</span>
                    </div>
                  ))}
                  {d.checkRuns.map((c, i) => (
                    <div key={`c${i}`} className="gh-status__row">
                      <ChecksBadge state={c.state} />
                      <span>{c.name}</span>
                      <span className="gh-muted">{t(`gh.checks.${c.state}`)}</span>
                    </div>
                  ))}
                </div>
              )}
              <Comments items={pull?.comments ?? null} />
            </>
          )}
        </div>
      )}

      {view.tab === 'commits' && (
        <div className="gh-scroll">
          {!pull?.commits ? (
            <div className="gh-notice">{t('list.loading')}</div>
          ) : (
            pull.commits.map((c) => (
              <div key={c.sha} className="gh-commit">
                <span className="mono gh-commit__sha">{c.sha.slice(0, 7)}</span>
                <span className="gh-commit__msg">{c.message.split('\n')[0]}</span>
                {c.author ? <Avatar user={c.author} /> : null}
                <span className="gh-muted">{c.author?.login ?? c.authorName}</span>
                <span className="gh-row__when">{rel(c.date)}</span>
              </div>
            ))
          )}
        </div>
      )}

      {view.tab === 'files' && (
        <div className="gh-files">
          <nav className="gh-files__list">
            {!files ? (
              <div className="gh-notice">{t('list.loading')}</div>
            ) : (
              files.map((f) => (
                <div key={f.path} role="button" tabIndex={0} className={`file-row${f.path === pullFile ? ' is-active' : ''}`} title={f.path} onClick={() => selectPullFile(f.path)}>
                  <FileStatusIcon status={FILE_STATUS[f.status]} />
                  <span className="file-row__name">
                    <span className="file-row__base">{f.path.split('/').pop()}</span>
                  </span>
                  <span className="file-row__stat">
                    {!!f.additions && <span className="is-add">+{f.additions}</span>}
                    {!!f.deletions && <span className="is-del">−{f.deletions}</span>}
                  </span>
                </div>
              ))
            )}
          </nav>
          <div className="gh-files__diff">
            {file && <div className="diff-meta mono">{file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}</div>}
            <DiffBody diff={fileDiff} loading={!files} resetKey={pullFile} />
          </div>
        </div>
      )}
    </>
  )
}

// ── 이슈 상세 ───────────────────────────────────────────

function IssueView({ number }: { number: number }): React.JSX.Element {
  const t = useT()
  const issue = useGithubStore((s) => s.issue)
  const rel = useRelative()
  const d = issue?.number === number ? issue.detail : null
  return (
    <>
      <DetailHead title={d?.title ?? '…'} number={number} url={d?.htmlUrl} state={d?.state ?? 'open'} kind="issue" back={{ kind: 'issues' }} />
      {d && (
        <div className="gh-meta">
          <Avatar user={d.author} />
          <span>{t('gh.openedBy', { who: d.author.login, when: rel(d.createdAt) })}</span>
          {d.labels.map((l) => (
            <LabelChip key={l.name} label={l} />
          ))}
          {d.assignees.length > 0 && (
            <span className="gh-meta__people">
              {t('gh.assignees')} {d.assignees.map((u) => <Avatar key={u.login} user={u} />)}
            </span>
          )}
        </div>
      )}
      {issue?.error && <div className="gh-notice is-error">{issue.error}</div>}
      <div className="gh-scroll">
        {!d ? (
          <div className="gh-notice">{t('list.loading')}</div>
        ) : (
          <>
            <article className="gh-comment">
              <header className="gh-comment__head">
                <Avatar user={d.author} size={20} />
                <strong>{d.author.login}</strong>
                <span className="gh-comment__when">{rel(d.createdAt)}</span>
              </header>
              {d.bodyHtml ? <HtmlBody html={d.bodyHtml} /> : <p className="gh-muted">{t('gh.noDescription')}</p>}
            </article>
            <Comments items={issue?.comments ?? null} />
          </>
        )}
      </div>
    </>
  )
}
