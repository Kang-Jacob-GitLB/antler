import { useEffect, useState } from 'react'
import type { CommitDetail, GhPullSummary, JiraDevCommit, JiraDevPull } from '@shared/types'
import { useT } from '../../i18n'
import { api, errorMessage, isAbort, newRequestId } from '../../lib/api'
import { formatDate } from '../../lib/format'
import { extractIssueKeys } from '../../lib/issueKeys'
import { useActiveRepo, useAppStore } from '../../stores/appStore'
import { useGithubStore } from '../../stores/githubStore'
import { useJiraStore } from '../../stores/jiraStore'
import { useLogStore } from '../../stores/logStore'
import { GhStateIcon, prState } from '../github/GhBits'
import { isoToEpoch } from './JiraBits'

/** 키 검색으로 상세를 읽어 볼 커밋 수(부분 일치를 걸러 내므로 보일 수보다 넉넉히)와 보여 줄 커밋 수 */
const SCAN_COMMITS = 40
const SHOW_COMMITS = 10
const GH_PULL_URL_RE = /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)(?:[/?#]|$)/

type Loaded<T> = { state: 'loading' } | { state: 'ok'; items: T[] } | { state: 'error'; message: string } | { state: 'none' }

/** 한 줄에 보일 PR·커밋(Jira 연동 결과와 검색 결과를 같은 모양으로) */
interface PullRow {
  id: string
  title: string
  state: 'open' | 'merged' | 'closed'
  draft?: boolean
  sub: string
  open(): void
}
interface CommitRow {
  sha: string
  title: string
  sub: string
  open(): void
}

/**
 * 이슈 상세 오른쪽의 '개발' 패널. 먼저 Jira 의 GitHub 연동(dev-status)이 모아 둔 PR·커밋을 읽고,
 * 연동이 없거나 비어 있으면 지금 연 저장소에서 이 키를 언급한 커밋(git log --grep)·PR(GitHub 검색)을 찾는다.
 */
export function JiraDevPanel({ issueKey }: { issueKey: string }): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const [source, setSource] = useState<'loading' | 'jira' | 'search'>('loading')
  const [commits, setCommits] = useState<Loaded<CommitRow>>({ state: 'loading' })
  const [pulls, setPulls] = useState<Loaded<PullRow>>({ state: 'loading' })
  const repoId = repo?.id

  useEffect(() => {
    let alive = true
    const reqs = [newRequestId('jira'), newRequestId('jira')] as const
    setSource('loading')
    setCommits({ state: 'loading' })
    setPulls({ state: 'loading' })

    const when = (iso: string): string => (iso ? formatDate(isoToEpoch(iso), locale) : '')
    const join = (...parts: string[]): string => parts.filter(Boolean).join(' · ')

    // ── 1) Jira 연동 ──
    const fromJira = (p: JiraDevPull[], c: JiraDevCommit[]): void => {
      setSource('jira')
      setPulls({
        state: 'ok',
        items: p.map((x, i) => ({ id: x.url || `p${i}`, title: x.title, state: x.state, sub: join(x.repo, x.author, when(x.updated)), open: () => openDevPull(x.url) }))
      })
      setCommits({
        state: 'ok',
        items: c.map((x) => ({ sha: x.sha, title: x.message, sub: join(x.repo, x.author, when(x.date)), open: () => void openDevCommit(x.sha, x.url) }))
      })
    }

    // ── 2) 대신: 지금 연 저장소에서 검색 ──
    const fromSearch = (): void => {
      setSource('search')
      if (!repoId) return
      void (async () => {
        try {
          // git log --grep 은 부분 일치(ABC-12 가 ABC-123 에도 걸림)라 제목·본문에서 키를 다시 뽑아 정확히 같은 것만 남긴다
          const found = await api.invoke('git:search', repoId, issueKey, undefined, reqs[1])
          const details = await Promise.all(found.hashes.slice(0, SCAN_COMMITS).map((h) => api.invoke('git:commit', repoId, h).catch(() => null)))
          const project = [issueKey.split('-')[0]!]
          const exact = details.filter((d): d is CommitDetail => !!d && extractIssueKeys([d.subject, d.body], project).includes(issueKey))
          if (!alive) return
          setCommits({
            state: 'ok',
            items: exact.slice(0, SHOW_COMMITS).map((d) => ({ sha: d.hash, title: d.subject, sub: join(d.authorName, formatDate(d.authorDate, locale)), open: () => revealCommit(d.hash) }))
          })
        } catch (err) {
          if (alive && !isAbort(err)) setCommits({ state: 'error', message: errorMessage(err) })
        }
      })()
      void (async () => {
        try {
          // GitHub 저장소가 아니면 PR 구역은 숨긴다
          if (!(await api.invoke('gh:repo', repoId))) {
            if (alive) setPulls({ state: 'none' })
            return
          }
          const items: GhPullSummary[] = await api.invoke('gh:pullsForKey', repoId, issueKey)
          if (!alive) return
          setPulls({
            state: 'ok',
            items: items.map((p) => ({
              id: String(p.number),
              title: p.title,
              state: p.state,
              draft: p.draft,
              sub: join(`#${p.number}`, p.author.login),
              open: () => useGithubStore.getState().openAt(repoId, { kind: 'pull', number: p.number, tab: 'conversation' })
            }))
          })
        } catch (err) {
          if (alive) setPulls({ state: 'error', message: errorMessage(err) })
        }
      })()
    }

    void api
      .invoke('jira:dev', issueKey, reqs[0])
      .then((dev) => {
        if (!alive) return
        if (dev.available && dev.pulls.length + dev.commits.length > 0) fromJira(dev.pulls, dev.commits)
        else fromSearch()
      })
      .catch((err) => alive && !isAbort(err) && fromSearch())

    return () => {
      alive = false
      for (const id of reqs) void api.invoke('req:cancel', id).catch(() => {})
    }
    // locale 이 바뀌어도 다시 조회하지는 않는다(날짜 표기만 다음 조회부터)
  }, [repoId, issueKey])

  /** 연동이 준 PR: 지금 연 저장소의 PR 이면 앱 안의 GitHub 화면으로, 아니면 브라우저로 */
  const openDevPull = (url: string): void => {
    const m = GH_PULL_URL_RE.exec(url)
    if (!m) return
    void (async () => {
      const ref = repoId ? await api.invoke('gh:repo', repoId).catch(() => null) : null
      if (repoId && ref && ref.owner.toLowerCase() === m[1]!.toLowerCase() && ref.repo.toLowerCase() === m[2]!.toLowerCase()) {
        useGithubStore.getState().openAt(repoId, { kind: 'pull', number: Number(m[3]), tab: 'conversation' })
      } else {
        void api.invoke('gh:openExternal', url).catch(() => {})
      }
    })()
  }

  /** 연동이 준 커밋: 지금 연 저장소에 있으면 그래프에서, 없으면 브라우저로 */
  const openDevCommit = async (sha: string, url: string): Promise<void> => {
    const here = repoId ? await api.invoke('git:commit', repoId, sha).then(() => true, () => false) : false
    if (here) revealCommit(sha)
    else if (url) void api.invoke('gh:openExternal', url).catch(() => {})
  }

  return (
    <section className="jira-panel">
      <h3 className="jira-panel__title">
        {t('jira.dev')}
        {source === 'jira' && <span className="jira-dev__repo">{t('jira.devFromJira')}</span>}
        {source === 'search' && repo && <span className="jira-dev__repo">{repo.name}</span>}
      </h3>
      {source === 'loading' ? (
        <p className="gh-muted">{t('list.loading')}</p>
      ) : source === 'search' && !repo ? (
        <p className="gh-muted">{t('jira.devNoRepo')}</p>
      ) : (
        <>
          {pulls.state !== 'none' && (
            <DevGroup title={t('jira.devPulls')} loaded={pulls} empty={t('jira.devNoPulls')}>
              {(p) => (
                <button key={p.id} className="jira-dev__row" title={p.title} onClick={p.open}>
                  <GhStateIcon kind="pull" state={prState(p)} size={13} />
                  <span className="jira-dev__text">
                    <span className="jira-dev__main">{p.title}</span>
                    <span className="jira-dev__sub">{p.sub}</span>
                  </span>
                </button>
              )}
            </DevGroup>
          )}
          <DevGroup title={t('jira.devCommits')} loaded={commits} empty={t('jira.devNoCommits')}>
            {(c) => (
              <button key={c.sha} className="jira-dev__row" title={c.title} onClick={c.open}>
                <span className="jira-dev__sha mono">{c.sha.slice(0, 7)}</span>
                <span className="jira-dev__text">
                  <span className="jira-dev__main">{c.title}</span>
                  <span className="jira-dev__sub">{c.sub}</span>
                </span>
              </button>
            )}
          </DevGroup>
        </>
      )}
    </section>
  )
}

/** Jira 화면이 창 전체를 덮고 있으니 닫고 그래프에서 그 커밋을 고른다 */
function revealCommit(hash: string): void {
  useJiraStore.getState().show(null)
  void useLogStore.getState().reveal(hash)
}

function DevGroup<T>({
  title,
  loaded,
  empty,
  children
}: {
  title: string
  loaded: Loaded<T>
  empty: string
  children(item: T): React.JSX.Element
}): React.JSX.Element {
  const t = useT()
  return (
    <div className="jira-dev__group">
      <div className="jira-dev__head">
        {title}
        {loaded.state === 'ok' && <span className="jira-cat__n">{loaded.items.length}</span>}
      </div>
      {loaded.state === 'loading' && <p className="gh-muted">{t('list.loading')}</p>}
      {loaded.state === 'error' && <p className="gh-muted jira-dev__error">{loaded.message}</p>}
      {loaded.state === 'ok' && (loaded.items.length === 0 ? <p className="gh-muted">{empty}</p> : loaded.items.map(children))}
    </div>
  )
}
