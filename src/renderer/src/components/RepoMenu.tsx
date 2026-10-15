import { useEffect, useRef, useState } from 'react'
import type { Repo } from '@shared/types'
import { useT } from '../i18n'
import './RepoMenu.css'

interface Props {
  repo: Repo
  groups: string[]
  onClose(): void
  onGroup(group: string | null): void
  onRemove(): void
}

/** 사이드바 저장소 행의 작은 메뉴: 그룹 지정(기존 그룹 고르기·새로 입력·해제), 목록에서 제거 */
export function RepoMenu({ repo, groups, onClose, onGroup, onRemove }: Props): React.JSX.Element {
  const t = useT()
  const [group, setGroup] = useState(repo.group ?? '')
  const [confirm, setConfirm] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const listId = `groups-${repo.id}`

  // 바깥을 누르거나 Esc 면 닫는다
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const save = (): void => {
    onGroup(group.trim() || null)
    onClose()
  }

  return (
    <div ref={ref} className="repo-menu" role="dialog" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
      <label className="repo-menu__label" htmlFor={`${listId}-input`}>
        {t('repoMenu.group')}
      </label>
      <div className="repo-menu__row">
        <input
          id={`${listId}-input`}
          className="repo-menu__input"
          list={listId}
          value={group}
          placeholder={t('repoMenu.groupPlaceholder')}
          autoFocus
          onChange={(e) => setGroup(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && save()}
        />
        <datalist id={listId}>
          {groups.map((g) => (
            <option key={g} value={g} />
          ))}
        </datalist>
        <button className="btn btn--sm" onClick={save}>
          {t('repoMenu.save')}
        </button>
      </div>
      <div className="repo-menu__sep" />
      {confirm ? (
        <div className="repo-menu__row">
          <span className="repo-menu__warn">{t('repoMenu.confirmRemove')}</span>
          <button
            className="btn btn--sm btn--danger"
            onClick={() => {
              onRemove()
              onClose()
            }}
          >
            {t('repoMenu.remove')}
          </button>
        </div>
      ) : (
        <button className="repo-menu__item is-danger" onClick={() => setConfirm(true)}>
          {t('sidebar.removeRepo')}
        </button>
      )}
      <div className="repo-menu__path" title={repo.path}>
        {repo.path}
      </div>
    </div>
  )
}
