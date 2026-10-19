import { useEffect, useRef } from 'react'
import { useT } from '../i18n'
import { GROUP_HUES } from '../lib/groupColor'
import { IconCheck } from './icons'
import './ColorMenu.css'

interface Props {
  /** 지금 색(hue). 기본색이면 undefined */
  value: number | undefined
  onPick(hue: number | undefined): void
  onClose(): void
  /** '기본색' 칸을 보일지(환경 배지) */
  allowDefault?: boolean
}

/** 색 고르기 팝오버: 그룹 칩·환경 배지가 함께 쓰는 팔레트 */
export function ColorMenu({ value, onPick, onClose, allowDefault }: Props): React.JSX.Element {
  const t = useT()
  const ref = useRef<HTMLDivElement>(null)

  // 바깥을 누르거나 Esc 면 닫는다. 열리면 지금 색(없으면 첫 칸)에 포커스
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    document.addEventListener('mousedown', onDown)
    ref.current?.querySelector<HTMLButtonElement>('[aria-checked="true"], button')?.focus()
    return () => document.removeEventListener('mousedown', onDown)
  }, [onClose])

  const pick = (hue: number | undefined): void => {
    onPick(hue)
    onClose()
  }

  return (
    <div
      ref={ref}
      className="color-menu"
      role="dialog"
      aria-label={t('color.pick')}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => {
        e.preventDefault()
        e.stopPropagation()
      }}
      // 사이드바 행의 Enter/Space 토글로 올라가지 않게 막고, Esc 는 여기서 처리한다
      onKeyDown={(e) => {
        e.stopPropagation()
        if (e.key === 'Escape') onClose()
      }}
      draggable={false}
      onDragStart={(e) => {
        e.preventDefault()
        e.stopPropagation()
      }}
    >
      <div className="color-menu__label">{t('color.pick')}</div>
      <div className="color-menu__grid" role="radiogroup" aria-label={t('color.pick')}>
        {allowDefault && (
          <button
            type="button"
            role="radio"
            aria-checked={value === undefined}
            className="color-menu__swatch color-menu__swatch--default"
            title={t('color.default')}
            onClick={() => pick(undefined)}
          >
            {value === undefined && <IconCheck size={11} />}
          </button>
        )}
        {GROUP_HUES.map((h) => (
          <button
            key={h}
            type="button"
            role="radio"
            aria-checked={value === h}
            className="color-menu__swatch"
            style={{ '--chip-h': h } as React.CSSProperties}
            title={`${t('color.hue')} ${h}°`}
            onClick={() => pick(h)}
          >
            {value === h && <IconCheck size={11} />}
          </button>
        ))}
      </div>
    </div>
  )
}
