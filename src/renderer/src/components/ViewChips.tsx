import { useEffect, useRef, useState } from 'react'
import type { ViewRef } from '@shared/types'
import { useT } from '../i18n'
import { useLogStore } from '../stores/logStore'
import { IconClose, IconEyeOff, IconSolo } from './icons'

/** "Solo 3개" / "숨김 2개" 요약 칩. 누르면 목록이 열려 하나씩 빼거나 모두 해제한다 */
function ViewChip({
  kind,
  inactive,
  items,
  onRemove,
  onClear
}: {
  kind: 'solo' | 'hide'
  /** Solo 가 있어 이 목록이 지금은 적용되지 않음 */
  inactive?: boolean
  items: ViewRef[]
  onRemove(ref: ViewRef): void
  onClear(): void
}): React.JSX.Element {
  const t = useT()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    // Esc 는 팝오버만 닫는다(열려 있던 diff 까지 닫히지 않게)
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey, true)
    }
  }, [open])
  // 마지막 항목을 빼면 이 칩이 사라지므로 팝오버도 닫는다
  useEffect(() => {
    if (items.length === 0) setOpen(false)
  }, [items.length])

  const solo = kind === 'solo'
  return (
    <div className="view-chip" ref={rootRef}>
      <button
        className={`filter-chip filter-chip--${kind}${inactive ? ' is-inactive' : ''}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={t(solo ? 'view.soloMenu' : 'view.hideMenu')}
        onClick={() => setOpen((o) => !o)}
      >
        {solo ? <IconSolo size={12} /> : <IconEyeOff size={12} />}
        <span className="filter-chip__label">{t(solo ? 'view.soloChip' : 'view.hideChip', { n: items.length })}</span>
      </button>
      {open && (
        <div className="view-chip__pop" role="menu" aria-label={t(solo ? 'view.soloMenu' : 'view.hideMenu')}>
          <div className="view-chip__title">{t(solo ? 'view.soloMenu' : 'view.hideMenu')}</div>
          <div className="view-chip__list">
            {items.map((r) => (
              <div key={r.id} className="view-chip__item" role="menuitem">
                <span className="view-chip__name" title={r.label}>
                  {r.label}
                </span>
                <button className="icon-btn icon-btn--sm" title={t('view.remove')} aria-label={`${t('view.remove')}: ${r.label}`} onClick={() => onRemove(r)}>
                  <IconClose size={11} />
                </button>
              </div>
            ))}
          </div>
          <button
            className="view-chip__clear"
            onClick={() => {
              setOpen(false)
              onClear()
            }}
          >
            {t(solo ? 'view.clearSolo' : 'view.clearHide')}
          </button>
        </div>
      )}
    </div>
  )
}

/** 툴바의 Solo / 숨김 요약. 둘 다 비어 있으면 아무것도 그리지 않는다. Hide 는 Solo 가 있는 동안 적용되지 않으므로 흐리게 보인다 */
export function ViewChips(): React.JSX.Element | null {
  const solo = useLogStore((s) => s.solo)
  const hide = useLogStore((s) => s.hide)
  const restoreView = useLogStore((s) => s.restoreView)
  const t = useT()
  const { toggleSolo, toggleHide, clearSolo, clearHide, restore } = useLogStore.getState()
  if (!solo.length && !hide.length && !restoreView) return null
  return (
    <>
      {restoreView && (
        <button className="filter-chip" title={t('view.restoreTip')} onClick={restore}>
          <IconSolo size={12} />
          <span className="filter-chip__label">{t('view.restore')}</span>
        </button>
      )}
      {solo.length > 0 && <ViewChip kind="solo" items={solo} onRemove={toggleSolo} onClear={clearSolo} />}
      {hide.length > 0 && <ViewChip kind="hide" inactive={solo.length > 0} items={hide} onRemove={toggleHide} onClear={clearHide} />}
    </>
  )
}
