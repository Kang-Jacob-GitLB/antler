import { useMemo } from 'react'
import type { Repo, Source } from '@shared/types'
import { useT } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { EnvBadge } from './EnvBadge'
import { IconClose, IconPlus, IconRepo } from './icons'
import './Sidebar.css'

/** 저장소 목록. 소스(환경)별 → 그룹별로 묶는다. 브랜치·태그·워크트리 섹션은 M2 에서 추가된다. */
export function Sidebar({ width }: { width: number }): React.JSX.Element {
  const t = useT()
  const config = useAppStore((s) => s.config)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const { selectRepo, addLocalRepo, removeRepo } = useAppStore.getState()

  const groups = useMemo(() => {
    if (!config) return []
    const bySource = new Map<string, { source: Source; repos: Repo[] }>()
    for (const s of config.sources) bySource.set(s.id, { source: s, repos: [] })
    for (const r of config.repos) bySource.get(r.sourceId)?.repos.push(r)
    return [...bySource.values()]
      .filter((g) => g.repos.length)
      .map((g) => ({
        ...g,
        repos: [...g.repos].sort((a, b) => (a.group ?? '').localeCompare(b.group ?? '') || a.name.localeCompare(b.name))
      }))
  }, [config])

  return (
    <aside className="sidebar" style={{ width }}>
      <div className="sidebar__section-head">
        <span>{t('sidebar.repos')}</span>
        <button className="icon-btn icon-btn--sm" title={t('sidebar.addRepo')} onClick={() => void addLocalRepo()}>
          <IconPlus size={14} />
        </button>
      </div>

      <nav className="sidebar__list">
        {groups.length === 0 && <div className="sidebar__empty">{t('sidebar.empty')}</div>}
        {groups.map(({ source, repos }) => (
          <div key={source.id} className="sidebar__group">
            {groups.length > 1 && <div className="sidebar__group-label">{source.label}</div>}
            {repos.map((repo) => (
              <div
                key={repo.id}
                role="button"
                tabIndex={0}
                className={`repo-item${repo.id === activeRepoId ? ' is-active' : ''}`}
                onClick={() => selectRepo(repo.id)}
                onKeyDown={(e) => e.key === 'Enter' && selectRepo(repo.id)}
                title={repo.path}
              >
                <IconRepo size={14} className="repo-item__icon" />
                <span className="repo-item__name">{repo.name}</span>
                <EnvBadge kind={source.kind} />
                <button
                  className="repo-item__remove"
                  title={t('sidebar.removeRepo')}
                  onClick={(e) => {
                    e.stopPropagation()
                    void removeRepo(repo.id)
                  }}
                >
                  <IconClose size={12} />
                </button>
              </div>
            ))}
          </div>
        ))}
      </nav>
    </aside>
  )
}
