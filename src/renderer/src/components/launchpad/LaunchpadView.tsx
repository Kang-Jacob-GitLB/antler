// Launchpad: 여러 저장소의 내 PR · 리뷰 요청 · 할당 이슈(와 Jira 할당 이슈)를 한 화면에 모아 보는 읽기 전용 대시보드.
// 목록 행의 레이아웃은 GitHub 화면의 .gh-row 를 재사용한다.
import { useEffect, useMemo, useState } from 'react'
import { useT } from '../../i18n'
import {
  jiraEntries,
  LP_SECTIONS,
  snoozeUntil,
  toEntries,
  visibleEntries,
  type LpEntry,
  type LpScope,
  type LpSection,
  type SnoozePreset
} from '../../lib/launchpad'
import { useAppStore } from '../../stores/appStore'
import { useGithubStore } from '../../stores/githubStore'
import { useJiraStore } from '../../stores/jiraStore'
import { useLaunchpadStore } from '../../stores/launchpadStore'
import { Avatar, ChecksBadge, GhStateIcon, LabelChip, prState, useRelative } from '../github/GhBits'
import { GroupChip } from '../GroupChip'
import { IconClose, IconPin, IconRefresh, IconSnooze } from '../icons'
import { JiraStatus } from '../jira/JiraBits'
import { MenuButton } from '../MenuButton'
import { groupHue } from '../../lib/groupColor'
import './Launchpad.css'

const PRESETS: SnoozePreset[] = ['1h', '4h', 'tomorrow', '1w']
const NO_PINS: string[] = []
const NO_SNOOZES: Record<string, number> = {}

export function LaunchpadView(): React.JSX.Element {
  const t = useT()
  const rel = useRelative()
  const { scope, section, filter, loading, error, result: cached, setScope, setSection, setFilter, load, show } = useLaunchpadStore()
  const config = useAppStore((s) => s.config)
  const groupColors = config?.ui.groupColors
  const pins = config?.ui.launchpadPins ?? NO_PINS
  const snoozes = config?.ui.launchpadSnoozes ?? NO_SNOOZES
  const repos = config?.repos
  const jiraAuth = useJiraStore((s) => s.auth)
  const jiraList = useJiraStore((s) => s.mine)

  const groups = useMemo(() => [...new Set((repos ?? []).map((r) => r.group).filter((g): g is string => !!g))].sort(), [repos])
  const entries = useMemo(() => [...toEntries(cached?.items ?? [], repos ?? [], scope), ...jiraEntries(jiraList.items)], [cached, repos, scope, jiraList.items])
  // 스누즈가 끝나는 가장 가까운 시각마다 now 를 갱신해 만료된 항목이 다시 나타나게 한다
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined
    const arm = (): void => {
      const t = Date.now()
      setNow(t)
      const next = Object.values(snoozes).filter((u) => u > t)
      if (next.length > 0) timer = setTimeout(arm, Math.min(Math.min(...next) - t + 50, 2 ** 31 - 1))
    }
    arm()
    return () => clearTimeout(timer)
  }, [snoozes])
  const views = Object.fromEntries(LP_SECTIONS.map((s) => [s, visibleEntries(entries, s, filter, pins, snoozes, now)])) as Record<LpSection, ReturnType<typeof visibleEntries>>
  const view = views[section]
  // 저장소 필터 선택지: 범위 안에서 항목이 있는 저장소
  const repoOptions = useMemo(() => {
    const seen = new Map<string, string>()
    for (const e of entries) if (e.repoId && e.repoName) seen.set(e.repoId, e.repoName)
    return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1]))
  }, [entries])

  const isJira = section === 'jira'
  const noRepos = (repos?.length ?? 0) === 0
  const needsLogin = !isJira && (cached?.needsLogin === true)
  const pinned = new Set(pins)

  const refresh = (): void => {
    void load(true)
    if (jiraAuth.connected) void useJiraStore.getState().loadMine()
  }
  const setScopeKind = (s: LpScope): void => setScope(s)
  const busy = loading || (isJira && jiraList.loading)

  return (
    <section className="gh-view lp" aria-label={t('launchpad.title')}>
      <header className="gh-head">
        <h2 className="gh-head__title lp-title">{t('launchpad.title')}</h2>
        <span className="gh-muted lp-updated">{cached && !cached.needsLogin ? t('launchpad.updated', { when: rel(cached.fetchedAt / 1000) }) : t('launchpad.neverUpdated')}</span>
        <span className="gh-head__spacer" />
        <button className={`icon-btn icon-btn--sm${busy ? ' is-spinning' : ''}`} title={t('toolbar.refresh')} aria-busy={busy} onClick={refresh}>
          <IconRefresh size={14} />
        </button>
        <button className="icon-btn icon-btn--sm" title={`${t('diff.close')} (Esc)`} onClick={() => show(false)}>
          <IconClose size={13} />
        </button>
      </header>

      <div className="lp-scope" role="group" aria-label={t('launchpad.scope')}>
        <button className={`lp-chip${scope.kind === 'all' ? ' is-on' : ''}`} aria-pressed={scope.kind === 'all'} onClick={() => setScopeKind({ kind: 'all' })}>
          {t('launchpad.scopeAll')}
        </button>
        {groups.map((g) => {
          const on = scope.kind === 'group' && scope.name === g
          return (
            <button
              key={g}
              className={`group-chip group-chip--btn${on ? ' is-on' : ''}`}
              style={{ '--chip-h': groupHue(g, groupColors) } as React.CSSProperties}
              aria-pressed={on}
              onClick={() => setScopeKind({ kind: 'group', name: g })}
            >
              <span className="group-chip__label">{g}</span>
            </button>
          )
        })}
        <select
          className={`gh-select lp-repo-select${scope.kind === 'repo' ? ' is-on' : ''}`}
          aria-label={t('launchpad.pickRepo')}
          value={scope.kind === 'repo' ? scope.id : ''}
          onChange={(e) => (e.target.value ? setScopeKind({ kind: 'repo', id: e.target.value }) : setScopeKind({ kind: 'all' }))}
        >
          <option value="">{t('launchpad.scopeRepo')}</option>
          {(repos ?? []).map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
      </div>

      <div className="gh-head lp-tabs">
        <div className="seg" role="tablist">
          {LP_SECTIONS.map((s) => (
            <button key={s} role="tab" aria-selected={section === s} className={`seg__btn${section === s ? ' is-on' : ''}`} onClick={() => setSection(s)}>
              {t(`launchpad.section.${s}`)}
              {(s !== 'jira' || jiraAuth.connected) && <span className="lp-count">{views[s].items.length}</span>}
            </button>
          ))}
        </div>
      </div>

      <div className="gh-filters lp-filters">
        <input
          className="gh-auth__input lp-search"
          type="search"
          value={filter.text}
          placeholder={t('launchpad.search')}
          aria-label={t('launchpad.search')}
          spellCheck={false}
          onChange={(e) => setFilter({ text: e.target.value })}
        />
        {!isJira && (
          <select className="gh-select lp-filter-repo" aria-label={t('launchpad.repoFilter')} value={filter.repoId ?? ''} onChange={(e) => setFilter({ repoId: e.target.value || null })}>
            <option value="">{t('launchpad.allRepos')}</option>
            {repoOptions.map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </select>
        )}
        {!isJira && (
          <label className="lp-check">
            <input type="checkbox" checked={filter.hideDrafts} onChange={(e) => setFilter({ hideDrafts: e.target.checked })} />
            {t('launchpad.hideDrafts')}
          </label>
        )}
        {view.snoozed > 0 && (
          <button className="btn btn--sm" onClick={() => setFilter({ showSnoozed: !filter.showSnoozed })}>
            {filter.showSnoozed ? t('launchpad.hideSnoozed') : t('launchpad.showSnoozed', { n: view.snoozed })}
          </button>
        )}
      </div>

      <div className="gh-list lp-list">
        {!isJira && error && <div className="gh-notice is-error">{error}</div>}
        {!isJira && cached?.errors.map((m) => (
          <div key={m} className="gh-notice is-error">
            {m}
          </div>
        ))}
        {!isJira && cached && !cached.needsLogin && cached.incomplete && <div className="gh-notice lp-note">{t('launchpad.incomplete')}</div>}
        {!isJira && cached && cached.unresolved > 0 && <div className="gh-notice lp-note">{t('launchpad.unresolved', { n: cached.unresolved })}</div>}
        {!isJira && cached && cached.skipped > 0 && <div className="gh-notice lp-note">{t('launchpad.skipped', { n: cached.skipped })}</div>}

        {needsLogin && (
          <div className="lp-empty">
            <p>{t('launchpad.needLogin')}</p>
            <button className="btn btn--primary" onClick={() => useGithubStore.getState().setAuthPopoverOpen(true)}>
              {t('launchpad.signIn')}
            </button>
          </div>
        )}
        {!isJira && !needsLogin && noRepos && <div className="lp-empty"><p>{t('launchpad.noRepos')}</p></div>}
        {isJira && !jiraAuth.connected && (
          <div className="lp-empty">
            <p>{t('launchpad.jiraOff')}</p>
            <button className="btn btn--primary" onClick={() => useJiraStore.getState().setDialogOpen(true)}>
              {t('launchpad.jiraConnect')}
            </button>
          </div>
        )}
        {isJira && jiraList.error && <div className="gh-notice is-error">{jiraList.error}</div>}

        {!needsLogin && (isJira ? jiraAuth.connected : !noRepos) && !busy && !error && view.items.length === 0 && <div className="gh-notice">{t('launchpad.empty')}</div>}
        {!needsLogin && view.items.map((e) => (
          <Row key={`${e.section}:${e.key}`} e={e} pinned={pinned.has(e.key)} snoozedUntil={snoozes[e.key] ?? 0} now={now} rel={rel} />
        ))}
        {busy && view.items.length === 0 && <div className="gh-notice">{t('list.loading')}</div>}
      </div>
    </section>
  )
}

function Row({ e, pinned, snoozedUntil, now, rel }: { e: LpEntry; pinned: boolean; snoozedUntil: number; now: number; rel: (n: number) => string }): React.JSX.Element {
  const t = useT()
  const open = (): void => useLaunchpadStore.getState().openEntry(e)
  const snoozed = snoozedUntil > now
  const stop = (ev: React.SyntheticEvent): void => ev.stopPropagation()
  return (
    <div
      className={`gh-row lp-row${pinned ? ' is-pinned' : ''}${snoozed ? ' is-snoozed' : ''}`}
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(ev) => ev.target === ev.currentTarget && ev.key === 'Enter' && open()}
    >
      {e.kind === 'jira' ? (
        <span className="jira-key mono">{e.jiraKey}</span>
      ) : (
        <GhStateIcon kind={e.kind} state={prState({ state: e.state ?? 'open', draft: e.draft })} />
      )}
      <div className="gh-row__main">
        <div className="gh-row__title">
          <span className="gh-row__text">{e.title}</span>
          {e.labels.map((l) => (
            <LabelChip key={l.name} label={l} />
          ))}
        </div>
        <div className="gh-row__sub lp-sub">
          {e.kind === 'jira' ? (
            <>
              <JiraStatus name={e.status ?? ''} category={e.statusCategory ?? ''} />
              {e.assignee && <span>{e.assignee}</span>}
            </>
          ) : (
            <>
              <span className="lp-repo" title={e.slug}>
                {e.repoName}
              </span>
              {e.group && <GroupChip name={e.group} />}
              <span>
                #{e.number}
                {e.author && ` · ${e.author.login}`}
              </span>
            </>
          )}
          {snoozed && <span className="lp-tag">{t('launchpad.snoozedTag')}</span>}
        </div>
      </div>
      <ChecksBadge state={e.checks} />
      {e.author && <Avatar user={e.author} />}
      <span className="gh-row__when">{rel(e.updatedAt)}</span>
      <div className="lp-actions" onClick={stop} onKeyDown={stop}>
        <button
          className={`icon-btn icon-btn--sm lp-pin${pinned ? ' is-on' : ''}`}
          title={t(pinned ? 'launchpad.unpin' : 'launchpad.pin')}
          aria-label={t(pinned ? 'launchpad.unpin' : 'launchpad.pin')}
          aria-pressed={pinned}
          onClick={() => useLaunchpadStore.getState().togglePin(e.key)}
        >
          <IconPin size={13} filled={pinned} />
        </button>
        <MenuButton<string>
          label={t('launchpad.snooze')}
          value=""
          className="icon-btn--sm"
          options={[
            ...(snoozed ? [{ value: 'off', label: t('launchpad.unsnooze') }] : []),
            ...PRESETS.map((p) => ({ value: p, label: t(`launchpad.snooze.${p}` as const) }))
          ]}
          onChange={(v) => useLaunchpadStore.getState().snooze(e.key, v === 'off' ? null : snoozeUntil(v as SnoozePreset, Date.now()))}
        >
          <IconSnooze size={13} />
        </MenuButton>
      </div>
    </div>
  )
}
