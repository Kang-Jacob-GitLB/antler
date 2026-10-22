import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { GhAccessibleRepo } from '@shared/types'
import { useT } from '../i18n'
import {
  filterGhRepos,
  formatPushed,
  ghKey,
  listOwners,
  ownerFilterValue,
  parseOwnerFilter
} from '../lib/ghRepoList'
import { useAppStore } from '../stores/appStore'
import { useGithubStore } from '../stores/githubStore'
import type { GhAccessibleState } from './useGhAccessibleRepos'
import { IconBranch, IconLock, IconRefresh } from './icons'
import './GithubRepoPicker.css'

/** 목록 한 줄 높이(px): 이름 줄 + 설명 줄. 가상화에 쓰므로 고정이다 */
const ROW_H = 48

interface Props {
  state: GhAccessibleState & { load(fresh: boolean): void }
  /** 이미 등록된 저장소의 ghKey 집합 */
  registered: Set<string>
  /** 추가할 것으로 고른 저장소의 ghKey */
  checked: Set<string>
  onChecked(next: Set<string>): void
  /** 추가하는 중이면 고르기를 막는다 */
  busy: boolean
}

/**
 * 저장소 추가 창의 GitHub 탭 본문: 로그인 안내 · 검색 · 소유자 거르기 · 다중 선택 목록.
 * 데이터는 useGhAccessibleRepos 가 받아 오고, 추가(제출)는 창이 한다.
 */
export function GithubRepoPicker({ state, registered, checked, onChecked, busy }: Props): React.JSX.Element {
  const t = useT()
  const { result, loading, error, load } = state

  if (!result && !error) {
    return <div className="gh-pick__state">{loading ? t('add.gh.loading') : null}</div>
  }
  if (result?.needsLogin) return <LoginPanel />
  if (!result) {
    return (
      <div className="gh-pick__state">
        <div className="add-dialog__msg is-error" role="alert">
          {error}
        </div>
        <div>
          <button type="button" className="btn" disabled={loading} onClick={() => load(true)}>
            <IconRefresh size={14} />
            {t('add.gh.retry')}
          </button>
        </div>
      </div>
    )
  }
  return (
    <RepoList
      repos={result.repos}
      truncated={result.truncated}
      loading={loading}
      error={error}
      load={load}
      registered={registered}
      checked={checked}
      onChecked={onChecked}
      busy={busy}
    />
  )
}

function RepoList({
  repos,
  truncated,
  loading,
  error,
  load,
  registered,
  checked,
  onChecked,
  busy
}: {
  repos: GhAccessibleRepo[]
  truncated: boolean
  loading: boolean
  error: string | null
  load(fresh: boolean): void
  registered: Set<string>
  checked: Set<string>
  onChecked(next: Set<string>): void
  busy: boolean
}): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const [query, setQuery] = useState('')
  const [ownerValue, setOwnerValue] = useState('all')
  const scrollRef = useRef<HTMLUListElement>(null)

  const owners = useMemo(() => listOwners(repos), [repos])
  // 새로 받은 목록에 없는 조직을 고르고 있었다면 전체로 되돌린다
  useEffect(() => {
    if (ownerValue.startsWith('org:') && !owners.orgs.some((o) => `org:${o}` === ownerValue)) setOwnerValue('all')
    if (ownerValue === 'user' && !owners.hasUser) setOwnerValue('all')
  }, [owners, ownerValue])

  const visible = useMemo(() => filterGhRepos(repos, query, parseOwnerFilter(ownerValue)), [repos, query, ownerValue])
  const selectable = useMemo(() => visible.filter((r) => !registered.has(ghKey(r.owner, r.repo))), [visible, registered])
  const allOn = selectable.length > 0 && selectable.every((r) => checked.has(ghKey(r.owner, r.repo)))

  const virtualizer = useVirtualizer({
    count: visible.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 8
  })

  // 걸러 낸 결과가 바뀌면 맨 위부터 본다
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [query, ownerValue])

  const toggle = (key: string, on: boolean): void => {
    const next = new Set(checked)
    if (on) next.add(key)
    else next.delete(key)
    onChecked(next)
  }
  // 전체 선택·해제는 지금 걸러진 항목만 바꾼다(걸러 낸 밖의 선택은 그대로)
  const toggleAll = (): void => {
    const next = new Set(checked)
    for (const r of selectable) {
      const k = ghKey(r.owner, r.repo)
      if (allOn) next.delete(k)
      else next.add(k)
    }
    onChecked(next)
  }

  const countLabel =
    visible.length === repos.length
      ? t('add.gh.count', { n: repos.length.toLocaleString(locale) })
      : t('add.gh.countFiltered', { n: repos.length.toLocaleString(locale), shown: visible.length.toLocaleString(locale) })

  return (
    <>
      <div className="field">
        <div className="field__row">
          <input
            className="field__input"
            type="text"
            value={query}
            placeholder={t('add.gh.search')}
            aria-label={t('add.gh.search')}
            spellCheck={false}
            autoFocus
            onChange={(e) => setQuery(e.target.value)}
            // Enter 는 폼 제출이 아니라 검색어 입력일 뿐이다. Esc 는 검색어가 있으면 그것만 지운다
            onKeyDown={(e) => {
              if (e.key === 'Enter') e.preventDefault()
              else if (e.key === 'Escape' && query) {
                e.stopPropagation()
                setQuery('')
              }
            }}
          />
          <select
            className="field__input gh-pick__owner"
            value={ownerValue}
            aria-label={t('add.gh.owner')}
            onChange={(e) => setOwnerValue(e.target.value)}
          >
            <option value={ownerFilterValue({ kind: 'all' })}>{t('add.gh.ownerAll')}</option>
            {owners.hasUser && <option value={ownerFilterValue({ kind: 'user' })}>{t('add.gh.ownerUser')}</option>}
            {owners.orgs.length > 0 && (
              <optgroup label={t('add.gh.ownerOrgs')}>
                {owners.orgs.map((o) => (
                  <option key={o} value={ownerFilterValue({ kind: 'org', owner: o })}>
                    {o}
                  </option>
                ))}
              </optgroup>
            )}
          </select>
          <button type="button" className="btn" disabled={loading || busy} aria-busy={loading} onClick={() => load(true)}>
            <IconRefresh size={14} />
            {t('add.gh.refresh')}
          </button>
        </div>
      </div>

      <div className="field">
        <div className="add-found__head">
          <span className="field__label">{countLabel}</span>
          <span className="add-dialog__spacer" />
          <button type="button" className="btn btn--sm" disabled={selectable.length === 0 || busy} onClick={toggleAll}>
            {allOn ? t('add.selectNone') : t('add.selectAll')}
          </button>
        </div>
        <ul ref={scrollRef} className="add-found gh-pick__list" aria-label={countLabel}>
          {visible.length === 0 ? (
            <li className="gh-pick__none">{t(repos.length === 0 ? 'add.gh.empty' : 'add.gh.noMatch')}</li>
          ) : (
            <li className="gh-pick__sizer" style={{ height: virtualizer.getTotalSize() }}>
              {virtualizer.getVirtualItems().map((vi) => {
                const r = visible[vi.index]!
                const key = ghKey(r.owner, r.repo)
                const done = registered.has(key)
                return (
                  <label
                    key={key}
                    className={`add-found__item gh-pick__item${done ? ' is-done' : ''}`}
                    style={{ height: ROW_H, transform: `translateY(${vi.start}px)` }}
                    title={r.fullName}
                  >
                    <input
                      type="checkbox"
                      checked={!done && checked.has(key)}
                      disabled={done || busy}
                      onChange={(e) => toggle(key, e.target.checked)}
                    />
                    <span className="gh-pick__main">
                      <span className="gh-pick__line">
                        {r.private && (
                          <span className="gh-pick__lock" role="img" aria-label={t('add.gh.private')} title={t('add.gh.private')}>
                            <IconLock size={12} />
                          </span>
                        )}
                        <span className="gh-pick__name">
                          <span className="gh-pick__owner-name">{r.owner}/</span>
                          {r.repo}
                        </span>
                        {r.archived && <span className="gh-pick__tag">{t('add.gh.archived')}</span>}
                        {r.fork && <span className="gh-pick__tag">{t('add.gh.fork')}</span>}
                        {done && <span className="add-found__tag">{t('add.already')}</span>}
                      </span>
                      <span className={`gh-pick__desc${r.description ? '' : ' is-empty'}`}>{r.description || t('add.gh.noDesc')}</span>
                    </span>
                    <span className="gh-pick__meta">
                      <span className="gh-pick__branch mono">
                        <IconBranch size={11} />
                        {r.defaultBranch}
                      </span>
                      <span title={new Date(r.pushedAt).toLocaleString(locale)}>
                        {t('add.gh.pushed', { ago: formatPushed(r.pushedAt, locale) })}
                      </span>
                    </span>
                  </label>
                )
              })}
            </li>
          )}
        </ul>
        {error && (
          <span className="field__hint is-warn" role="alert">
            {error}
          </span>
        )}
        <span className="field__hint">{truncated ? t('add.gh.truncated') : t('add.gh.pickHint')}</span>
      </div>
    </>
  )
}

/** 로그인 안 된 상태: 안내와 로그인(토큰 · gh CLI · 브라우저 Device Flow). 로그인되면 목록을 자동으로 다시 받는다 */
function LoginPanel(): React.JSX.Element {
  const t = useT()
  const { auth, device, devicePending, authError, loginPat, importCli, startDevice, cancelDevice, refreshAuth } = useGithubStore()
  const clientId = useAppStore((s) => s.config?.ui.githubClientId ?? '')
  const [pat, setPat] = useState('')
  const [busy, setBusy] = useState<'pat' | 'cli' | null>(null)

  useEffect(() => {
    void refreshAuth()
  }, [refreshAuth])

  const run = async (kind: 'pat' | 'cli'): Promise<void> => {
    if (kind === 'pat' && !pat.trim()) return
    setBusy(kind)
    try {
      if (kind === 'pat') {
        await loginPat(pat.trim())
        setPat('')
      } else await importCli()
    } catch {
      /* 오류는 authError 로 보인다 */
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="gh-pick__login">
      <div className="add-dialog__msg gh-pick__notice">{t('add.gh.loginNeeded')}</div>
      {device ? (
        <div className="field">
          <span className="field__label">{t('gh.auth.deviceTitle')}</span>
          <div className="gh-pick__code mono selectable">{device.userCode}</div>
          <span className="field__hint">{t('gh.auth.deviceHint', { url: device.verificationUri })}</span>
          <div className="field__row">
            <button type="button" className="btn" onClick={() => void navigator.clipboard.writeText(device.userCode)}>
              {t('detail.copy')}
            </button>
            <button type="button" className="btn" onClick={cancelDevice}>
              {t('add.cancel')}
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="field">
            <label className="field__label" htmlFor="add-gh-pat">
              {t('gh.auth.pat')}
            </label>
            <div className="field__row">
              <input
                id="add-gh-pat"
                className="field__input mono"
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={pat}
                placeholder="github_pat_…"
                autoFocus
                onChange={(e) => setPat(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault()
                    void run('pat')
                  }
                }}
              />
              <button type="button" className="btn btn--primary" disabled={!!busy || !pat.trim()} onClick={() => void run('pat')}>
                {busy === 'pat' ? t('add.adding') : t('gh.auth.signInShort')}
              </button>
            </div>
            <span className="field__hint">{t('gh.auth.patHint')}</span>
          </div>
          {(auth.cliAvailable || clientId) && (
            <div className="field__row gh-pick__alt">
              {auth.cliAvailable && (
                <button type="button" className="btn" disabled={!!busy} title={t('gh.auth.importCliHint')} onClick={() => void run('cli')}>
                  {busy === 'cli' ? t('add.adding') : t('gh.auth.importCli')}
                </button>
              )}
              {clientId && (
                <button type="button" className="btn" disabled={!!devicePending} onClick={() => void startDevice()}>
                  {t('gh.auth.device')}
                </button>
              )}
            </div>
          )}
        </>
      )}
      {authError && (
        <div className="add-dialog__msg is-error" role="alert">
          {authError}
        </div>
      )}
    </div>
  )
}
