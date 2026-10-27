import { useEffect, useRef, useState } from 'react'
import type { Repo, SourceKind } from '@shared/types'
import { useT } from '../i18n'
import { externalSupported, openExternal, openOnGithub, repoLocation } from '../lib/external'
import { EXTERNAL_ITEMS } from './ExternalMenu'
import { IconGithub } from './icons'
import { GroupPicker } from './GroupChip'
import './RepoMenu.css'

interface Props {
  repo: Repo
  /** 저장소 환경(SSH 는 에디터로만 열 수 있다) */
  sourceKind: SourceKind
  groups: string[]
  onClose(): void
  onGroup(group: string | null): void
  onRemove(): void
}

/** 사이드바 저장소 행의 작은 메뉴: 그룹 지정(기존 그룹 고르기·새로 입력·해제), 목록에서 제거 */
export function RepoMenu({ repo, sourceKind, groups, onClose, onGroup, onRemove }: Props): React.JSX.Element {
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
      if (e.key !== 'Escape') return
      e.preventDefault() // 같은 Esc 를 App 의 화면 닫기가 또 처리하지 않게 한다
      onClose()
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
    <div
      ref={ref}
      className="repo-menu"
      role="dialog"
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      // 메뉴 안의 키 입력(Enter 저장 등)이 사이드바 행으로 올라가 저장소를 전환하지 않게 막는다.
      // React 의 stopPropagation 은 document 리스너까지 막으므로 Esc 는 여기서 직접 처리한다
      onKeyDown={(e) => {
        e.stopPropagation()
        if (e.key === 'Escape') onClose()
      }}
    >
      <label className="repo-menu__label" htmlFor={`${listId}-input`}>
        {t('repoMenu.group')}
      </label>
      <GroupPicker
        inputId={`${listId}-input`}
        inputClassName="repo-menu__input"
        groups={groups}
        value={group}
        placeholder={t('repoMenu.groupPlaceholder')}
        autoFocus
        onChange={setGroup}
        onEnter={save}
        trailing={
          <button className="btn btn--sm" onClick={save}>
            {t('repoMenu.save')}
          </button>
        }
      />
      <div className="repo-menu__sep" />
      {/* 외부 도구로 열기: 파일 관리자 · 에디터 · 터미널(저장소 폴더) */}
      {sourceKind === 'github' && (
        // 원격 전용 저장소: 작업 폴더가 없으므로 GitHub 웹으로 연다
        <button
          className="repo-menu__item repo-menu__item--icon"
          onClick={() => {
            void openOnGithub(repo.id)
            onClose()
          }}
        >
          <IconGithub size={14} />
          <span>{t('ext.openGithub')}</span>
        </button>
      )}
      {sourceKind !== 'github' && EXTERNAL_ITEMS.map(({ action, label, icon: Icon }) => {
        const ok = externalSupported(sourceKind, action)
        return (
          <button
            key={action}
            className="repo-menu__item repo-menu__item--icon"
            disabled={!ok}
            title={ok ? undefined : t('ext.sshOnlyEditor')}
            onClick={() => {
              void openExternal(repo.id, action)
              onClose()
            }}
          >
            <Icon size={14} />
            <span>{t(label)}</span>
          </button>
        )
      })}
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
      <div className="repo-menu__path" title={repoLocation(repo)}>
        {repoLocation(repo)}
      </div>
    </div>
  )
}
