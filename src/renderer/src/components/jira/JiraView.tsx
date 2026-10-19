import { useEffect, useState } from 'react'
import type { JiraIssueSummary, JiraSavedFilter } from '@shared/types'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { MINE_JQL, useJiraSiteData, useJiraStore, type JiraView as View } from '../../stores/jiraStore'
import { HtmlBody, useRelative } from '../github/GhBits'
import { IconRefresh } from '../icons'
import { isoToEpoch, issueSub, JiraStatus } from './JiraBits'
import '../github/GitHubView.css'
import './JiraView.css'

const NO_FILTERS: JiraSavedFilter[] = []
const openOnJira = (url: string): void => void api.invoke('jira:openExternal', url).catch(() => {})

/** Jira 작업 공간(JiraWorkspace)의 본문: 이슈 목록 / 이슈 상세. GitHub 화면과 같은 레이아웃·클래스를 쓴다 */
export function JiraView(): React.JSX.Element | null {
  const view = useJiraStore((s) => s.view)
  if (!view) return null
  return <section className="gh-view jira-view">{view.kind === 'list' ? <ListView view={view} /> : <IssueView view={view} />}</section>
}

// ── 목록 ───────────────────────────────────────────────

function ListView({ view }: { view: Extract<View, { kind: 'list' }> }): React.JSX.Element {
  const t = useT()
  const list = useJiraStore((s) => s.list)
  const { openIssue, loadList, saveFilter } = useJiraStore.getState()
  const filters = useJiraSiteData().filters ?? NO_FILTERS
  const saved = typeof view.source === 'number' ? filters[view.source] : undefined
  const title = view.source === 'mine' ? t('jira.mine') : saved ? saved.name : t('jira.customJql')
  const [jql, setJql] = useState(list.jql)
  const [name, setName] = useState('')
  const rel = useRelative()

  // 다른 목록으로 바뀌면 입력칸도 그 목록의 JQL 로
  useEffect(() => setJql(view.source === 'mine' ? MINE_JQL : (saved?.jql ?? useJiraStore.getState().list.jql)), [view.source, saved?.jql])

  const run = (): void => {
    if (jql.trim()) void loadList(jql.trim())
  }
  const canSave = view.source === 'custom' && !!jql.trim() && !!name.trim()

  return (
    <>
      <header className="gh-head">
        <h2 className="gh-head__title">Jira · {title}</h2>
        <button className="icon-btn icon-btn--sm" title={t('toolbar.refresh')} onClick={run}>
          <IconRefresh size={14} />
        </button>
      </header>

      <div className="gh-filters jira-jql">
        <input
          className="jira-input mono"
          value={jql}
          readOnly={view.source !== 'custom'}
          spellCheck={false}
          aria-label="JQL"
          placeholder="project = ABC AND status = Open"
          onChange={(e) => setJql(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && run()}
        />
        {view.source === 'custom' && (
          <>
            <button className="btn btn--sm btn--primary" disabled={!jql.trim()} onClick={run}>
              {t('jira.run')}
            </button>
            <input className="jira-input jira-input--name" value={name} placeholder={t('jira.filterName')} aria-label={t('jira.filterName')} onChange={(e) => setName(e.target.value)} />
            <button
              className="btn btn--sm"
              disabled={!canSave}
              onClick={() => {
                saveFilter({ name: name.trim(), jql: jql.trim() })
                setName('')
              }}
            >
              {t('jira.saveFilter')}
            </button>
          </>
        )}
      </div>

      <div className="gh-list">
        {list.error && <div className="gh-notice is-error">{list.error}</div>}
        {!list.error && !list.loading && list.items.length === 0 && <div className="gh-notice">{t('gh.empty')}</div>}
        {list.items.map((i) => (
          <IssueRow key={i.key} i={i} when={rel(isoToEpoch(i.updated))} onOpen={() => openIssue(i.key)} />
        ))}
        {list.loading && <div className="gh-notice">{t('list.loading')}</div>}
        {list.nextCursor && !list.loading && (
          <button className="btn gh-more" onClick={() => void loadList(list.jql, true)}>
            {t('gh.more')}
          </button>
        )}
      </div>
    </>
  )
}

function IssueRow({ i, when, onOpen }: { i: JiraIssueSummary; when: string; onOpen(): void }): React.JSX.Element {
  const t = useT()
  return (
    <div className="gh-row" role="button" tabIndex={0} onClick={onOpen} onKeyDown={(e) => e.key === 'Enter' && onOpen()}>
      <span className="jira-key mono">{i.key}</span>
      <div className="gh-row__main">
        <div className="gh-row__title">
          <span className="gh-row__text">{i.summary}</span>
        </div>
        <div className="gh-row__sub">{issueSub(i, t('jira.unassigned'))}</div>
      </div>
      <JiraStatus name={i.status} category={i.statusCategory} />
      <span className="gh-row__when">{when}</span>
    </div>
  )
}

// ── 상세 ───────────────────────────────────────────────

function IssueView({ view }: { view: Extract<View, { kind: 'issue' }> }): React.JSX.Element {
  const t = useT()
  const issue = useJiraStore((s) => s.issue)
  const siteUrl = useJiraStore((s) => s.auth.siteUrl)
  const { show } = useJiraStore.getState()
  const rel = useRelative()
  const cur = issue?.key === view.key ? issue : null
  const d = cur?.detail ?? null

  return (
    <>
      <header className="gh-head">
        {view.from && (
          <button className="btn btn--sm" onClick={() => show(view.from)}>
            ← {t('jira.list')}
          </button>
        )}
        <h2 className="gh-head__title selectable">
          <span className="jira-key mono">{view.key}</span> {d?.summary ?? '…'}
        </h2>
        {d?.url && (
          <button className="btn btn--sm" onClick={() => openOnJira(d.url)}>
            {t('jira.openInBrowser')}
          </button>
        )}
      </header>

      {d && (
        <div className="gh-meta jira-meta">
          <JiraStatus name={d.status} category={d.statusCategory} />
          <Field label={t('jira.type')} value={d.issueType} />
          <Field label={t('jira.priority')} value={d.priority} />
          <Field label={t('jira.assignee')} value={d.assignee || t('jira.unassigned')} />
          <Field label={t('jira.reporter')} value={d.reporter} />
          <span className="gh-muted">{t('jira.updatedAt', { when: rel(isoToEpoch(d.updated)) })}</span>
        </div>
      )}
      {cur?.error && <div className="gh-notice is-error">{cur.error}</div>}

      <div className="gh-scroll">
        {!d && !cur?.error && <div className="gh-notice">{t('list.loading')}</div>}
        {d && (
          <>
            <article className="gh-comment">
              {d.descriptionHtml ? <HtmlBody html={d.descriptionHtml} {...(siteUrl ? { jiraSite: siteUrl } : {})} /> : <p className="gh-muted">{t('gh.noDescription')}</p>}
            </article>
            {cur?.commentsError && <div className="gh-notice is-error">{cur.commentsError}</div>}
            {cur?.comments?.map((c, n) => (
              <article key={n} className="gh-comment">
                <header className="gh-comment__head">
                  <strong>{c.author}</strong>
                  <span className="gh-comment__when">{rel(isoToEpoch(c.created))}</span>
                </header>
                <HtmlBody html={c.bodyHtml} {...(siteUrl ? { jiraSite: siteUrl } : {})} />
              </article>
            ))}
          </>
        )}
      </div>
    </>
  )
}

function Field({ label, value }: { label: string; value: string }): React.JSX.Element | null {
  if (!value) return null
  return (
    <span className="jira-field">
      <span className="gh-muted">{label}</span> {value}
    </span>
  )
}
