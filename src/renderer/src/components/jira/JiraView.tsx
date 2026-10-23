import { Fragment, useEffect, useMemo, useState } from 'react'
import type { JiraIssueSummary, JiraSavedFilter, JiraStatusCategory } from '@shared/types'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { formatAbsolute, formatCalendarDate, isBeforeToday } from '../../lib/format'
import { useAppStore } from '../../stores/appStore'
import { MINE_VIEW_JQL, useJiraSiteData, useJiraStore, type JiraView as View } from '../../stores/jiraStore'
import { HtmlBody, useRelative } from '../github/GhBits'
import { IconBoard, IconRefresh, IconRows, IconSearch } from '../icons'
import { JiraDevPanel } from './JiraDevPanel'
import { JiraTransitionMenu } from './JiraTransitionMenu'
import { WorklogView } from './worklog/WorklogView'
import { isEpic, isoToEpoch, issueSub, JiraAvatar, JiraPerson, JiraStatus, JiraType, statusCat } from './JiraBits'
import '../github/GitHubView.css'
import './JiraView.css'

const NO_FILTERS: JiraSavedFilter[] = []
const NO_CATS: JiraStatusCategory[] = []
const CATS: JiraStatusCategory[] = ['new', 'indeterminate', 'done']
const openOnJira = (url: string): void => void api.invoke('jira:openExternal', url).catch(() => {})

/** Jira 작업 공간(JiraWorkspace)의 본문: 이슈 목록 / 이슈 상세. GitHub 화면과 같은 레이아웃·클래스를 쓴다 */
export function JiraView(): React.JSX.Element | null {
  const view = useJiraStore((s) => s.view)
  if (!view) return null
  return <section className="gh-view jira-view">{view.kind === 'list' ? <ListView view={view} /> : view.kind === 'worklog' ? <WorklogView /> : <IssueView view={view} />}</section>
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
  const layout = useAppStore((s) => s.config?.ui.jiraLayout ?? 'list')
  const hidden = useAppStore((s) => s.config?.ui.jiraHiddenCategories ?? NO_CATS)
  const { setUi } = useAppStore.getState()
  const [query, setQuery] = useState('')

  // 상태 분류별 개수(불러온 항목 기준)와, 숨긴 분류·검색어를 거친 항목
  const counts = useMemo(() => {
    const c: Record<JiraStatusCategory, number> = { new: 0, indeterminate: 0, done: 0 }
    for (const i of list.items) c[statusCat(i.statusCategory)]++
    return c
  }, [list.items])
  const shown = useMemo(() => {
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean)
    return list.items.filter((i) => {
      if (hidden.includes(statusCat(i.statusCategory))) return false
      if (terms.length === 0) return true
      const hay = `${i.key} ${i.summary} ${i.assignee} ${i.issueType} ${i.status}`.toLowerCase()
      return terms.every((w) => hay.includes(w))
    })
  }, [list.items, hidden, query])
  const keyCh = useMemo(() => shown.reduce((n, i) => Math.max(n, i.key.length), 0), [shown])
  const toggleCat = (c: JiraStatusCategory): void => {
    void setUi({ jiraHiddenCategories: hidden.includes(c) ? hidden.filter((x) => x !== c) : [...hidden, c] })
  }

  // 다른 목록으로 바뀌면 입력칸도 그 목록의 JQL 로
  useEffect(() => setJql(view.source === 'mine' ? MINE_VIEW_JQL : (saved?.jql ?? useJiraStore.getState().list.jql)), [view.source, saved?.jql])

  const run = (): void => {
    if (jql.trim()) void loadList(jql.trim())
  }
  const canSave = view.source === 'custom' && !!jql.trim() && !!name.trim()
  // 불러오는 중 / 다음 페이지 — 걸러서 하나도 안 보여도 더 불러올 수 있게 늘 아래에 둔다
  const foot = list.loading ? (
    <div className="gh-notice">{t('list.loading')}</div>
  ) : list.nextCursor ? (
    <button className="btn gh-more" onClick={() => void loadList(list.jql, true)}>
      {t('gh.more')}
    </button>
  ) : null

  return (
    <>
      <header className="gh-head">
        <h2 className="gh-head__title">Jira · {title}</h2>
        <div className="seg" role="radiogroup" aria-label={t('jira.layout')}>
          {(['list', 'board'] as const).map((l) => (
            <button
              key={l}
              role="radio"
              aria-checked={layout === l}
              className={`seg__btn jira-seg-icon${layout === l ? ' is-on' : ''}`}
              title={t(`jira.layout.${l}`)}
              onClick={() => void setUi({ jiraLayout: l })}
            >
              {l === 'list' ? <IconRows size={14} /> : <IconBoard size={14} />}
              {t(`jira.layout.${l}`)}
            </button>
          ))}
        </div>
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

      {/* 불러온 항목을 바로 거른다: 상태 분류(켜고 끄기, 설정에 기억) · 검색어 */}
      <div className="gh-filters jira-tools">
        <div className="seg" role="group" aria-label={t('jira.catFilter')}>
          {CATS.map((c) => {
            const on = !hidden.includes(c)
            return (
              <button key={c} aria-pressed={on} className={`seg__btn jira-cat jira-cat--${c}${on ? ' is-on' : ''}`} onClick={() => toggleCat(c)}>
                <span className="jira-cat__dot" />
                {t(`jira.cat.${c}`)}
                <span className="jira-cat__n">{counts[c]}</span>
              </button>
            )
          })}
        </div>
        <label className="jira-search">
          <IconSearch size={13} />
          <input
            className="jira-input"
            value={query}
            spellCheck={false}
            placeholder={t('jira.search')}
            aria-label={t('jira.search')}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === 'Escape' && setQuery('')}
          />
        </label>
      </div>

      {list.error && <div className="gh-notice is-error">{list.error}</div>}
      {layout === 'board' && shown.length > 0 ? (
        <>
          <Board items={shown} rel={rel} onOpen={openIssue} />
          {foot}
        </>
      ) : (
        // 키 칸을 가장 긴 키에 맞춰 고정해 제목 시작 위치를 맞춘다
        <div className="gh-list" style={{ '--jira-key-ch': keyCh } as React.CSSProperties}>
          {shown.map((i) => (
            <IssueRow key={i.key} i={i} when={<IssueWhen i={i} rel={rel} />} onOpen={() => openIssue(i.key)} />
          ))}
          {!list.error && !list.loading && shown.length === 0 && (
            <div className="gh-notice">{list.items.length === 0 ? t('gh.empty') : t('jira.filteredOut', { n: list.items.length })}</div>
          )}
          {foot}
        </div>
      )}
    </>
  )
}

type BoardLane = 'epic' | 'work'

/**
 * 칸반: 상태 이름별 열. 열 순서는 할 일 → 진행 중 → 완료, 같은 분류 안에서는 목록에 처음 나온 순서.
 * 에픽과 그 밖의 작업 항목은 같은 칸에 섞지 않고 줄(스윔레인)을 나눈다 — 열은 두 줄이 함께 쓴다
 */
function Board({ items, rel, onOpen }: { items: JiraIssueSummary[]; rel(epoch: number): string; onOpen(key: string): void }): React.JSX.Element {
  const t = useT()
  const { columns, lanes } = useMemo(() => {
    const byStatus = new Map<string, { name: string; cat: JiraStatusCategory; n: number }>()
    const cells: Record<BoardLane, Map<string, JiraIssueSummary[]>> = { epic: new Map(), work: new Map() }
    const count: Record<BoardLane, number> = { epic: 0, work: 0 }
    for (const i of items) {
      const name = i.status || '—'
      const col = byStatus.get(name) ?? byStatus.set(name, { name, cat: statusCat(i.statusCategory), n: 0 }).get(name)!
      col.n++
      const lane: BoardLane = isEpic(i.issueType) ? 'epic' : 'work'
      const cell = cells[lane].get(name) ?? cells[lane].set(name, []).get(name)!
      cell.push(i)
      count[lane]++
    }
    const lanes = (['epic', 'work'] as const).filter((l) => count[l] > 0).map((l) => ({ id: l, n: count[l], cells: cells[l] }))
    return { columns: [...byStatus.values()].sort((a, b) => CATS.indexOf(a.cat) - CATS.indexOf(b.cat)), lanes }
  }, [items])
  // 한 종류뿐이면 줄 머리는 군더더기라 뺀다
  const laneHeads = lanes.length > 1

  return (
    <div className="jira-board" style={{ '--jira-cols': columns.length } as React.CSSProperties}>
      {columns.map((col) => (
        <header key={col.name} className={`jira-col__head jira-col--${col.cat}`}>
          <span className="jira-cat__dot" />
          <span className="jira-col__name">{col.name}</span>
          <span className="jira-cat__n">{col.n}</span>
        </header>
      ))}
      {lanes.map((lane) => (
        <Fragment key={lane.id}>
          {laneHeads && (
            <div className="jira-lane">
              <span className="jira-lane__label">
                {t(`jira.lane.${lane.id}`)}
                <span className="jira-cat__n">{lane.n}</span>
              </span>
            </div>
          )}
          {columns.map((col) => (
            <section key={col.name} className="jira-cell" aria-label={laneHeads ? `${t(`jira.lane.${lane.id}`)} · ${col.name}` : col.name}>
              {lane.cells.get(col.name)?.map((i) => (
                <div key={i.key} className="jira-card" role="button" tabIndex={0} onClick={() => onOpen(i.key)} onKeyDown={(e) => e.key === 'Enter' && onOpen(i.key)}>
                  <div className="jira-card__title">{i.summary}</div>
                  <div className="jira-card__meta">
                    <span className="jira-key mono">{i.key}</span>
                    <JiraType type={i.issueType} />
                    <span className="jira-card__sub">{issueSub(i, t('jira.unassigned'))}</span>
                    {i.assignee && <JiraAvatar name={i.assignee} url={i.assigneeAvatar} className="jira-card__avatar" />}
                  </div>
                  <div className="jira-card__when">
                    <IssueWhen i={i} rel={rel} />
                  </div>
                </div>
              ))}
            </section>
          ))}
        </Fragment>
      ))}
    </div>
  )
}

/** 목록 행·카드의 날짜: 기한이 있으면 기한(완료 전인데 지났으면 빨강), 없으면 마지막으로 수정한 때 */
function IssueWhen({ i, rel }: { i: JiraIssueSummary; rel(epoch: number): string }): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  if (!i.dueDate) return <>{rel(isoToEpoch(i.updated))}</>
  const overdue = statusCat(i.statusCategory) !== 'done' && isBeforeToday(i.dueDate)
  return (
    <span className={overdue ? 'jira-props__overdue' : undefined} title={t('jira.dueDate')}>
      {t('jira.dueDate')} {formatCalendarDate(i.dueDate, locale)}
    </span>
  )
}

function IssueRow({ i, when, onOpen }: { i: JiraIssueSummary; when: React.ReactNode; onOpen(): void }): React.JSX.Element {
  const t = useT()
  return (
    <div className="gh-row" role="button" tabIndex={0} onClick={onOpen} onKeyDown={(e) => e.key === 'Enter' && onOpen()}>
      <span className="jira-key mono">{i.key}</span>
      <div className="gh-row__main">
        <div className="gh-row__title">
          <JiraType type={i.issueType} />
          <span className="gh-row__text">{i.summary}</span>
        </div>
        <div className="gh-row__sub">{issueSub(i, t('jira.unassigned'))}</div>
      </div>
      <div className="gh-row__people">{i.assignee && <JiraAvatar name={i.assignee} url={i.assigneeAvatar} />}</div>
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
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const jiraSite = siteUrl ? { jiraSite: siteUrl } : {}

  return (
    <>
      <header className="gh-head">
        {view.from && (
          <button className="btn btn--sm" onClick={() => show(view.from)}>
            ← {view.from.kind === 'worklog' ? t('jira.worklog.title') : t('jira.list')}
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

      {cur?.error && <div className="gh-notice is-error">{cur.error}</div>}

      {/* Jira 이슈 화면처럼: 왼쪽 = 설명 · 댓글(구역을 나눔), 오른쪽 = 세부 정보 · 개발(관련 커밋·PR) */}
      <div className="gh-scroll jira-issue">
        {!d && !cur?.error && <div className="gh-notice">{t('list.loading')}</div>}
        {d && (
          <>
            <div className="jira-issue__main">
              <section className="jira-sec">
                <h3 className="jira-sec__title">{t('jira.description')}</h3>
                {d.descriptionHtml ? <HtmlBody html={d.descriptionHtml} {...jiraSite} /> : <p className="gh-muted">{t('gh.noDescription')}</p>}
              </section>

              <section className="jira-sec">
                <h3 className="jira-sec__title">
                  {t('jira.comments')}
                  {cur?.comments && <span className="jira-cat__n">{cur.comments.length}</span>}
                </h3>
                {cur?.commentsError && <div className="gh-notice is-error">{cur.commentsError}</div>}
                {!cur?.comments && !cur?.commentsError && <p className="gh-muted">{t('list.loading')}</p>}
                {cur?.comments?.length === 0 && <p className="gh-muted">{t('jira.noComments')}</p>}
                {cur?.comments?.map((c, n) => (
                  <article key={n} className="jira-cmt">
                    <JiraAvatar name={c.author} url={c.authorAvatar} className="jira-cmt__avatar" />
                    <div className="jira-cmt__main">
                      <header className="jira-cmt__head">
                        <strong>{c.author}</strong>
                        <span className="gh-muted" title={formatAbsolute(isoToEpoch(c.created), locale)}>
                          {rel(isoToEpoch(c.created))}
                        </span>
                      </header>
                      <HtmlBody html={c.bodyHtml} {...jiraSite} />
                    </div>
                  </article>
                ))}
              </section>
            </div>

            <aside className="jira-issue__side">
              <section className="jira-panel">
                <h3 className="jira-panel__title">{t('jira.details')}</h3>
                <dl className="jira-props">
                  <Prop label={t('jira.status')}>
                    <JiraTransitionMenu issueKey={view.key} status={d.status} category={d.statusCategory} />
                  </Prop>
                  <Prop label={t('jira.type')}>
                    <JiraType type={d.issueType} withName />
                  </Prop>
                  <Prop label={t('jira.priority')}>{d.priority}</Prop>
                  <Prop label={t('jira.assignee')}>{d.assignee ? <JiraPerson name={d.assignee} url={d.assigneeAvatar} /> : <span className="gh-muted">{t('jira.unassigned')}</span>}</Prop>
                  <Prop label={t('jira.reporter')}>{d.reporter && <JiraPerson name={d.reporter} url={d.reporterAvatar} />}</Prop>
                  <Prop label={t('jira.startDate')}>{formatCalendarDate(d.startDate, locale)}</Prop>
                  <Prop label={t('jira.dueDate')}>
                    {d.dueDate && <span className={d.statusCategory && d.statusCategory !== 'done' && isBeforeToday(d.dueDate) ? 'jira-props__overdue' : undefined}>{formatCalendarDate(d.dueDate, locale)}</span>}
                  </Prop>
                  <Prop label={t('jira.endDate')}>{formatCalendarDate(d.endDate, locale)}</Prop>
                  <Prop label={t('jira.created')}>{d.created && formatAbsolute(isoToEpoch(d.created), locale)}</Prop>
                  <Prop label={t('jira.updated')}>{d.updated && formatAbsolute(isoToEpoch(d.updated), locale)}</Prop>
                </dl>
              </section>
              <JiraDevPanel issueKey={view.key} />
            </aside>
          </>
        )}
      </div>
    </>
  )
}

/** 세부 정보 한 줄(값이 없으면 줄째 생략) */
function Prop({ label, children }: { label: string; children: React.ReactNode }): React.JSX.Element | null {
  if (children === '' || children === null || children === undefined || children === false) return null
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  )
}
