import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../i18n'
import { repoLocation } from '../lib/external'
import { useAppStore } from '../stores/appStore'
import { EnvBadge } from './EnvBadge'
import { IconSearch } from './icons'
import './RepoSwitcher.css'

/** Ctrl/Cmd+P 저장소 빠른 전환 */
export function RepoSwitcher(): React.JSX.Element | null {
  const t = useT()
  const open = useAppStore((s) => s.switcherOpen)
  const config = useAppStore((s) => s.config)
  const { setSwitcherOpen, selectRepo } = useAppStore.getState()
  const [query, setQuery] = useState('')
  const [cursor, setCursor] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setQuery('')
      setCursor(0)
      requestAnimationFrame(() => inputRef.current?.focus())
    }
  }, [open])

  const matches = useMemo(() => {
    if (!config) return []
    const q = query.trim().toLowerCase()
    return config.repos
      .filter((r) => !q || r.name.toLowerCase().includes(q) || repoLocation(r).toLowerCase().includes(q))
      .map((r) => ({ repo: r, source: config.sources.find((s) => s.id === r.sourceId) }))
  }, [config, query])

  if (!open) return null

  const close = (): void => setSwitcherOpen(false)
  const pick = (i: number): void => {
    const m = matches[i]
    if (!m) return
    selectRepo(m.repo.id)
    close()
  }

  return (
    <div className="switcher-backdrop" onMouseDown={close}>
      <div className="switcher" role="dialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
        <div className="switcher__input-row">
          <IconSearch className="switcher__icon" />
          <input
            ref={inputRef}
            className="switcher__input"
            placeholder={t('switcher.placeholder')}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setCursor(0)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Escape') close()
              else if (e.key === 'Enter') pick(cursor)
              else if (e.key === 'ArrowDown') {
                e.preventDefault()
                setCursor((c) => Math.min(c + 1, matches.length - 1))
              } else if (e.key === 'ArrowUp') {
                e.preventDefault()
                setCursor((c) => Math.max(c - 1, 0))
              }
            }}
          />
        </div>
        <ul className="switcher__list" role="listbox">
          {matches.length === 0 && <li className="switcher__none">{t('switcher.none')}</li>}
          {matches.map(({ repo, source }, i) => (
            <li
              key={repo.id}
              role="option"
              aria-selected={i === cursor}
              className={`switcher__item${i === cursor ? ' is-active' : ''}`}
              onMouseEnter={() => setCursor(i)}
              onClick={() => pick(i)}
            >
              {source && <EnvBadge kind={source.kind} sourceId={source.id} />}
              <span className="switcher__name">{repo.name}</span>
              <span className="switcher__path">{repoLocation(repo)}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
