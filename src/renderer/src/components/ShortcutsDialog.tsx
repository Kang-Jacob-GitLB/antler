import { useEffect, useMemo, useRef } from 'react'
import { useT } from '../i18n'
import {
  CATEGORY_KEY,
  CATEGORY_ORDER,
  EXTRA_SHORTCUTS,
  readCommandCtx,
  resolveCommands
} from '../lib/commands'
import { matchesShortcut, shortcutParts } from '../lib/shortcuts'
import { useAppStore } from '../stores/appStore'
import { usePaletteStore } from '../stores/paletteStore'
import { IconClose } from './icons'
import { Keys } from './Keys'
import './RepoSwitcher.css'
import './CommandPalette.css'

const SHORTCUTS = { code: 'Slash' } as const

/** Ctrl/Cmd+/ 단축키 목록. 명령 레지스트리의 단축키와, 각 화면이 직접 처리하는 키를 범주별 표로 보여 준다 */
export function ShortcutsDialog(): React.JSX.Element | null {
  const open = usePaletteStore((s) => s.shortcutsOpen)
  if (!open) return null
  return <Body />
}

function Body(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const ref = useRef<HTMLDivElement>(null)
  const mac = window.antler.platform === 'darwin'
  const close = (): void => usePaletteStore.getState().setShortcutsOpen(false)

  useEffect(() => {
    ref.current?.focus()
    // 열려 있는 동안 전역 단축키는 멈춰 있으므로 닫는 키는 여기서 직접 받는다
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' || matchesShortcut(e, SHORTCUTS)) {
        e.preventDefault()
        close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const groups = useMemo(() => {
    const cmds = resolveCommands(t, readCommandCtx()).filter((c) => c.shortcut)
    return CATEGORY_ORDER.map((cat) => ({
      cat,
      rows: [
        ...cmds
          .filter((c) => c.category === cat)
          .map((c) => ({ id: c.id, label: c.label, chords: [shortcutParts(c.shortcut!, mac)] })),
        ...EXTRA_SHORTCUTS.filter((x) => x.category === cat).map((x) => ({ id: x.id, label: t(x.title), chords: x.chords }))
      ]
    })).filter((g) => g.rows.length > 0)
  }, [locale, mac])

  return (
    <div className="switcher-backdrop" onMouseDown={close}>
      <div
        ref={ref}
        className="shortcuts"
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-title"
        tabIndex={-1}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="shortcuts__head">
          <h2 id="shortcuts-title" className="shortcuts__title">
            {t('shortcuts.title')}
          </h2>
          <button className="icon-btn icon-btn--sm" title={t('shortcuts.close')} aria-label={t('shortcuts.close')} onClick={close}>
            <IconClose size={13} />
          </button>
        </div>
        <div className="shortcuts__body">
          {groups.map((g) => (
            <section key={g.cat}>
              <h3 className="shortcuts__group">{t(CATEGORY_KEY[g.cat])}</h3>
              <table className="shortcuts__table">
                <tbody>
                  {g.rows.map((r) => (
                    <tr key={r.id}>
                      <th scope="row">{r.label}</th>
                      <td>
                        <span className="shortcuts__alt">
                          {r.chords.map((parts, i) => (
                            <Keys key={i} parts={parts.map((p) => (p === 'Mouse4' ? t('sc.mouseBack') : p))} mac={mac} />
                          ))}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          ))}
        </div>
      </div>
    </div>
  )
}
