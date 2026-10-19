import { useEffect, useMemo, useRef } from 'react'
import type { EditorKind } from '@shared/types'
import { useT, type MessageKey } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { IconClose } from './icons'
import { useModalFocus } from './useModalFocus'
import './SettingsDialog.css'

const EDITORS: { v: EditorKind; key: MessageKey }[] = [
  { v: 'vscode', key: 'editor.vscode' },
  { v: 'cursor', key: 'editor.cursor' },
  { v: 'jetbrains', key: 'editor.jetbrains' },
  { v: 'custom', key: 'editor.custom' }
]

/** 설정 대화상자: 에디터(외부 도구) 선택과 저장소 목록에 보일 그룹 */
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
  const repos = useAppStore((s) => s.config?.repos)
  const { setUi, pickEditorExe, openSettings } = useAppStore.getState()
  const close = (): void => openSettings(false)

  // 열려 있는 동안 전역 단축키는 멈춰 있으므로 Esc 는 여기서 직접 받는다
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

          {groups.length > 0 && (
            <section className="settings-sec" aria-labelledby="settings-groups">
              <h3 id="settings-groups" className="settings-sec__title">
                {t('settings.groups')}
              </h3>
              <div className="settings-groups">
                {groups.map((g) => (
                  <label key={g} className="settings-check">
                    <input type="checkbox" checked={isShown(g)} onChange={() => toggleGroup(g)} />
                    <span>{g}</span>
                  </label>
                ))}
              </div>
              <p className="settings-sec__hint">{t('settings.groupsHint')}</p>
            </section>
          )}
        </div>
      </div>
    </div>
  )
}

