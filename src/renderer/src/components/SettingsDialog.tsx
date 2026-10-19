import { useEffect, useMemo, useRef, useState } from 'react'
import type { EditorKind, Profile } from '@shared/types'
import { useT, type MessageKey } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { IconCheck, IconClose } from './icons'
import { useModalFocus } from './useModalFocus'
import './SettingsDialog.css'

const EDITORS: { v: EditorKind; key: MessageKey }[] = [
  { v: 'vscode', key: 'editor.vscode' },
  { v: 'cursor', key: 'editor.cursor' },
  { v: 'jetbrains', key: 'editor.jetbrains' },
  { v: 'custom', key: 'editor.custom' }
]

/** 설정 대화상자: 에디터(외부 도구) 선택과 프로필(이름 붙인 설정 묶음) 관리 */
export function SettingsDialog(): React.JSX.Element | null {
  const open = useAppStore((s) => s.settingsOpen)
  if (!open) return null
  return <Body />
}

function Body(): React.JSX.Element {
  const t = useT()
  const ref = useRef<HTMLDivElement>(null)
  useModalFocus(ref)
  const ui = useAppStore((s) => s.config?.ui)
  const profiles = useAppStore((s) => s.config?.profiles ?? [])
  const repos = useAppStore((s) => s.config?.repos)
  const { setUi, saveProfile, applyProfile, duplicateProfile, deleteProfile, pickEditorExe, openSettings } = useAppStore.getState()
  const [name, setName] = useState('')
  const close = (): void => openSettings(false)

  // 열려 있는 동안 전역 단축키는 멈춰 있으므로 Esc 는 여기서 직접 받는다(이름 변경 입력 중의 Esc 는 그 입력이 먼저 처리한다)
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !e.defaultPrevented) {
        e.preventDefault()
        close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const groups = useMemo(() => [...new Set((repos ?? []).map((r) => r.group).filter((g): g is string => !!g))].sort(), [repos])
  const editor = ui?.editor ?? 'vscode'
  const visible = ui?.visibleGroups
  const isShown = (g: string): boolean => !visible || visible.includes(g)
  const toggleGroup = (g: string): void => {
    const shown = new Set(groups.filter(isShown))
    if (shown.has(g)) shown.delete(g)
    else shown.add(g)
    // 모두 보이면 제한 없음: 빈 목록을 보내면 main 이 설정을 지운다(패치로는 키를 지울 수 없다)
    void setUi({ visibleGroups: shown.size === groups.length ? [] : groups.filter((x) => shown.has(x)) })
  }

  const create = async (): Promise<void> => {
    if (await saveProfile({ name })) setName('')
  }

  return (
    <div className="modal-backdrop" onMouseDown={close}>
      <div
        ref={ref}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        tabIndex={-1}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="settings-dialog__head">
          <h2 id="settings-title" className="settings-dialog__title">
            {t('settings.title')}
          </h2>
          <button className="icon-btn icon-btn--sm" title={t('settings.close')} aria-label={t('settings.close')} onClick={close}>
            <IconClose size={13} />
          </button>
        </div>

        <div className="settings-dialog__body">
          <section className="settings-sec" aria-labelledby="settings-editor">
            <h3 id="settings-editor" className="settings-sec__title">
              {t('settings.editor')}
            </h3>
            <div className="seg" role="radiogroup" aria-label={t('settings.editor')}>
              {EDITORS.map((e) => (
                <button
                  key={e.v}
                  role="radio"
                  aria-checked={editor === e.v}
                  className={`seg__btn${editor === e.v ? ' is-on' : ''}`}
                  onClick={() => void setUi({ editor: e.v })}
                >
                  {t(e.key)}
                </button>
              ))}
            </div>
            {(editor === 'jetbrains' || editor === 'custom') && (
              <div className="settings-exe">
                <span className="settings-exe__path mono" title={ui?.editorExePath}>
                  {ui?.editorExePath ?? t('settings.noExe')}
                </span>
                <button className="btn btn--sm" onClick={() => void pickEditorExe()}>
                  {t('settings.pickExe')}
                </button>
              </div>
            )}
            <p className="settings-sec__hint">{t('settings.editorHint')}</p>
          </section>

          <section className="settings-sec" aria-labelledby="settings-profiles">
            <h3 id="settings-profiles" className="settings-sec__title">
              {t('profile.section')}
            </h3>
            <p className="settings-sec__hint">{t('profile.hint')}</p>
            <form
              className="settings-new"
              onSubmit={(e) => {
                e.preventDefault()
                void create()
              }}
            >
              <input
                className="field__input"
                value={name}
                maxLength={50}
                placeholder={t('profile.namePlaceholder')}
                aria-label={t('profile.namePlaceholder')}
                onChange={(e) => setName(e.target.value)}
              />
              <button type="submit" className="btn btn--primary" disabled={!name.trim()}>
                {t('profile.create')}
              </button>
            </form>
            {profiles.length === 0 ? (
              <div className="settings-sec__hint">{t('profile.empty')}</div>
            ) : (
              <ul className="settings-profiles">
                {profiles.map((p) => (
                  <ProfileRow
                    key={p.id}
                    profile={p}
                    active={ui?.activeProfileId === p.id}
                    onApply={() => void applyProfile(p.id)}
                    onOverwrite={() => void saveProfile({ id: p.id })}
                    onRename={(n) => saveProfile({ id: p.id, name: n })}
                    onDuplicate={() => void duplicateProfile(p.id, t('profile.copyName', { name: p.name }).slice(0, 50))}
                    onDelete={() => void deleteProfile(p.id)}
                  />
                ))}
              </ul>
            )}
          </section>

          {groups.length > 0 && (
            <section className="settings-sec" aria-labelledby="settings-groups">
              <h3 id="settings-groups" className="settings-sec__title">
                {t('profile.groups')}
              </h3>
              <div className="settings-groups">
                {groups.map((g) => (
                  <label key={g} className="settings-check">
                    <input type="checkbox" checked={isShown(g)} onChange={() => toggleGroup(g)} />
                    <span>{g}</span>
                  </label>
                ))}
              </div>
              <p className="settings-sec__hint">{t('profile.groupsHint')}</p>
            </section>
          )}
        </div>
      </div>
    </div>
  )
}

function ProfileRow(props: {
  profile: Profile
  active: boolean
  onApply(): void
  onOverwrite(): void
  onRename(name: string): Promise<boolean>
  onDuplicate(): void
  onDelete(): void
}): React.JSX.Element {
  const t = useT()
  const { profile: p, active } = props
  const [renaming, setRenaming] = useState(false)
  const [draft, setDraft] = useState(p.name)
  const [confirmDelete, setConfirmDelete] = useState(false)

  const commitRename = async (): Promise<void> => {
    if (draft.trim() && draft.trim() !== p.name && !(await props.onRename(draft))) return
    setRenaming(false)
  }

  return (
    <li className={`settings-profile${active ? ' is-active' : ''}`}>
      <div className="settings-profile__name">
        {renaming ? (
          <input
            className="field__input"
            autoFocus
            value={draft}
            maxLength={50}
            aria-label={t('profile.rename')}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void commitRename()
              else if (e.key === 'Escape') {
                e.preventDefault()
                setRenaming(false)
              }
            }}
            onBlur={() => setRenaming(false)}
          />
        ) : (
          <>
            <span className="settings-profile__label" title={p.name}>
              {p.name}
            </span>
            {active && (
              <span className="settings-profile__badge">
                <IconCheck size={11} /> {t('profile.active')}
              </span>
            )}
          </>
        )}
      </div>
      <div className="settings-profile__actions">
        {!active && (
          <button className="btn btn--sm" onClick={props.onApply}>
            {t('profile.apply')}
          </button>
        )}
        <button className="btn btn--sm" title={t('profile.overwrite')} onClick={props.onOverwrite}>
          {t('profile.overwrite')}
        </button>
        <button className="btn btn--sm" onClick={() => (setDraft(p.name), setRenaming(true))}>
          {t('profile.rename')}
        </button>
        <button className="btn btn--sm" onClick={props.onDuplicate}>
          {t('profile.duplicate')}
        </button>
        {confirmDelete ? (
          <button className="btn btn--sm btn--danger" onClick={props.onDelete} onBlur={() => setConfirmDelete(false)} autoFocus>
            {t('profile.confirmDelete')}
          </button>
        ) : (
          <button className="btn btn--sm btn--danger" onClick={() => setConfirmDelete(true)}>
            {t('profile.delete')}
          </button>
        )}
      </div>
    </li>
  )
}
