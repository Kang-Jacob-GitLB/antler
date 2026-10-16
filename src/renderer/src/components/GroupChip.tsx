import { useId } from 'react'
import { useT } from '../i18n'
import { groupHue } from '../lib/groupColor'
import { useAppStore } from '../stores/appStore'
import { IconCheck } from './icons'
import './GroupChip.css'

/** 그룹 → 칩 색(hue). 설정에 저장된 색, 아직 없으면 새로 받을 색(lib/groupColor) */
function useHue(): (name: string) => number {
  const colors = useAppStore((s) => s.config?.ui.groupColors)
  return (name) => groupHue(name, colors)
}

/** 저장소 그룹 칩. 그룹마다 겹치지 않는 색을 쓴다 */
export function GroupChip({ name, className = '' }: { name: string; className?: string }): React.JSX.Element {
  const hue = useHue()
  return (
    <span className={`group-chip ${className}`} style={{ '--chip-h': hue(name) } as React.CSSProperties} title={name}>
      <span className="group-chip__label">{name}</span>
    </span>
  )
}

interface PickerProps {
  /** 이미 있는 그룹 이름들 */
  groups: string[]
  value: string
  onChange(value: string): void
  /** 입력란에서 Enter */
  onEnter?(): void
  autoFocus?: boolean
  placeholder?: string
  inputClassName?: string
  /** 입력란 오른쪽에 붙일 요소(저장 버튼 등) */
  trailing?: React.ReactNode
  /** 바깥 label 의 htmlFor 와 맞출 입력란 id */
  inputId?: string
}

/**
 * 그룹 고르기: 이미 있는 그룹을 칩으로 늘어놓고 누르면 고른다(다시 누르면 해제).
 * 새 그룹은 입력란에 이름을 쓰면 되고, 쓰는 동안 그 이름의 칩 색을 미리 보여 준다.
 */
export function GroupPicker({ groups, value, onChange, onEnter, autoFocus, placeholder, inputClassName = '', trailing, inputId }: PickerProps): React.JSX.Element {
  const t = useT()
  const hue = useHue()
  const autoId = useId()
  const id = inputId ?? autoId
  const v = value.trim()
  const isNew = v !== '' && !groups.some((g) => g === v)
  return (
    <div className="group-picker">
      <div className="group-picker__row">
        <div className="group-picker__field">
          {v && <span className="group-picker__swatch" style={{ '--chip-h': hue(v) } as React.CSSProperties} aria-hidden="true" />}
          <input
            id={id}
            className={`${inputClassName}${v ? ' has-swatch' : ''}`}
            value={value}
            placeholder={placeholder}
            autoFocus={autoFocus}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && onEnter) {
                e.preventDefault()
                onEnter()
              }
            }}
          />
        </div>
        {trailing}
      </div>
      {(groups.length > 0 || isNew) && (
        <div className="group-picker__chips" role="listbox" aria-label={t('group.existing')}>
          {groups.map((g) => {
            const on = g === v
            return (
              <button
                key={g}
                type="button"
                role="option"
                aria-selected={on}
                className={`group-chip group-chip--btn${on ? ' is-on' : ''}`}
                style={{ '--chip-h': hue(g) } as React.CSSProperties}
                title={on ? t('group.unpick') : g}
                onClick={() => onChange(on ? '' : g)}
              >
                {on && <IconCheck size={11} />}
                <span className="group-chip__label">{g}</span>
              </button>
            )
          })}
          {isNew && (
            <span className="group-chip is-new" style={{ '--chip-h': hue(v) } as React.CSSProperties} title={t('group.new')}>
              <span className="group-chip__label">{v}</span>
              <span className="group-chip__tag">{t('group.newTag')}</span>
            </span>
          )}
        </div>
      )}
    </div>
  )
}
