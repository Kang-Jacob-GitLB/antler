import { useEffect, useState } from 'react'
import type { CommitDetail, GhPullSummary } from '@shared/types'
import { useT } from '../../i18n'
import { api, errorMessage, isAbort, newRequestId } from '../../lib/api'
import { formatDate } from '../../lib/format'
import { extractIssueKeys } from '../../lib/issueKeys'
import { useActiveRepo, useAppStore } from '../../stores/appStore'
import { useGithubStore } from '../../stores/githubStore'
import { useJiraStore } from '../../stores/jiraStore'
import { useLogStore } from '../../stores/logStore'
import { GhStateIcon, prState } from '../github/GhBits'

/** 키 검색으로 상세를 읽어 볼 커밋 수(부분 일치를 걸러 내므로 보일 수보다 넉넉히)와 보여 줄 커밋 수 */
const SCAN_COMMITS = 40
const SHOW_COMMITS = 10

type Loaded<T> = { state: 'loading' } | { state: 'ok'; items: T[] } | { state: 'error'; message: string } | { state: 'none' }

/**
 * 이슈 상세 오른쪽의 '개발' 패널: 지금 연 저장소에서 이 이슈 키를 언급한 PR(GitHub 검색)과 커밋(git log --grep).
 * 저장소와 Jira 프로젝트의 짝은 따로 없으니, 아래 화면에 열린 저장소를 기준으로 찾는다.
 */
export function JiraDevPanel({ issueKey }: { issueKey: string }): React.JSX.Element {
  const t = useT()
  const repo = useActiveRepo()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const [commits, setCommits] = useState<Loaded<CommitDetail>>({ state: 'loading' })
  const [pulls, setPulls] = useState<Loaded<GhPullSummary>>({ state: 'loading' })
  const repoId = repo?.id

  useEffect(() => {
    if (!repoId) return
    let alive = true
    const reqId = newRequestId('jira')
    setCommits({ state: 'loading' })
    setPulls({ state: 'loading' })

    void (async () => {
      try {
        // git log --grep 은 부분 일치(ABC-12 가 ABC-123 에도 걸림)라 제목·본문에서 키를 다시 뽑아 정확히 같은 것만 남긴다
        const found = await api.invoke('git:search', repoId, issueKey, undefined, reqId)
        const details = await Promise.all(found.hashes.slice(0, SCAN_COMMITS).map((h) => api.invoke('git:commit', repoId, h).catch(() => null)))
        const project = [issueKey.split('-')[0]!]
        const exact = details.filter((d): d is CommitDetail => !!d && extractIssueKeys([d.subject, d.body], project).includes(issueKey))
        if (alive) setCommits({ state: 'ok', items: exact.slice(0, SHOW_COMMITS) })
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
        const items = await api.invoke('gh:pullsForKey', repoId, issueKey)
        if (alive) setPulls({ state: 'ok', items })
      } catch (err) {
        if (alive) setPulls({ state: 'error', message: errorMessage(err) })
      }
    })()

    return () => {
      alive = false
      void api.invoke('req:cancel', reqId).catch(() => {})
    }
  }, [repoId, issueKey])

  const openCommit = (hash: string): void => {
    // Jira 화면이 창 전체를 덮고 있으니 닫고 그래프에서 그 커밋을 고른다
    useJiraStore.getState().show(null)
    void useLogStore.getState().reveal(hash)
  }
  const openPull = (n: number): void => {
    if (repoId) useGithubStore.getState().openAt(repoId, { kind: 'pull', number: n, tab: 'conversation' })
  }

  return (
    <section className="jira-panel">
      <h3 className="jira-panel__title">
        {t('jira.dev')}
        {repo && <span className="jira-dev__repo">{repo.name}</span>}
      </h3>
      {!repo ? (
        <p className="gh-muted">{t('jira.devNoRepo')}</p>
      ) : (
        <>
          {pulls.state !== 'none' && (
            <DevGroup title={t('jira.devPulls')} loaded={pulls} empty={t('jira.devNoPulls')}>
              {(p) => (
                <button key={p.number} className="jira-dev__row" title={p.title} onClick={() => openPull(p.number)}>
                  <GhStateIcon kind="pull" state={prState(p)} size={13} />
                  <span className="jira-dev__text">
                    <span className="jira-dev__main">{p.title}</span>
                    <span className="jira-dev__sub">
                      #{p.number} · {p.author.login}
                    </span>
                  </span>
                </button>
              )}
            </DevGroup>
          )}
          <DevGroup title={t('jira.devCommits')} loaded={commits} empty={t('jira.devNoCommits')}>
            {(c) => (
              <button key={c.hash} className="jira-dev__row" title={c.subject} onClick={() => openCommit(c.hash)}>
                <span className="jira-dev__sha mono">{c.hash.slice(0, 7)}</span>
                <span className="jira-dev__text">
                  <span className="jira-dev__main">{c.subject}</span>
                  <span className="jira-dev__sub">
                    {c.authorName} · {formatDate(c.authorDate, locale)}
                  </span>
                </span>
              </button>
            )}
          </DevGroup>
        </>
      )}
    </section>
  )
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
