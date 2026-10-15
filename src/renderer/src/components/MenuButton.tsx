import { useEffect, useRef, useState } from 'react'
import { IconCheck } from './icons'
import './MenuButton.css'

export interface MenuOption<V extends string> {
  value: V
  label: string
  icon?: React.ReactNode
}

/**
 * 툴바용 단일 선택 메뉴(테마·언어 등). 토글처럼 눌러서 순환하지 않고, 선택지를 펼쳐 현재 값을 보여 주며 고르게 한다.
 * WAI-ARIA menu 패턴(menuitemradio): 열면 현재 항목에 포커스, ↑↓·Home·End 이동, Enter·Space 선택, Esc·바깥 클릭으로 닫힘.
 */
export function MenuButton<V extends string>(props: {
  label: string
  value: V
  options: MenuOption<V>[]
  onChange: (v: V) => void
  /** 버튼 안에 그릴 내용(현재 값을 나타내는 아이콘 등) */
  children: React.ReactNode
  className?: string
  /** 메뉴가 열리는 방향. 화면 아래의 상태 표시줄에서는 'top'(위로) */
  placement?: 'bottom' | 'top'
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([])

  useEffect(() => {
    if (!open) return
    const i = Math.max(0, props.options.findIndex((o) => o.value === props.value))
    itemRefs.current[i]?.focus()
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
    // 열릴 때 한 번만 포커스를 옮긴다(값이 바뀔 때마다 다시 옮기지 않는다)
  }, [open])

  const close = (refocus: boolean): void => {
    setOpen(false)
    if (refocus) btnRef.current?.focus()
  }
  const pick = (v: V): void => {
    props.onChange(v)
    close(true)
  }
  const onMenuKey = (e: React.KeyboardEvent): void => {
    const items = itemRefs.current.filter((x): x is HTMLButtonElement => !!x)
    const i = items.indexOf(document.activeElement as HTMLButtonElement)
    const move = (n: number): void => {
      e.preventDefault()
      items[(n + items.length) % items.length]?.focus()
    }
    if (e.key === 'ArrowDown') move(i + 1)
    else if (e.key === 'ArrowUp') move(i - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(items.length - 1)
    else if (e.key === 'Escape' || e.key === 'Tab') {
      e.stopPropagation()
      close(e.key === 'Escape')
    }
  }

  return (
    <div className="menu-btn" ref={ref}>
      <button
        ref={btnRef}
        className={`icon-btn ${props.className ?? ''}`}
        title={props.label}
        aria-label={props.label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-pressed={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' && !open) {
            e.preventDefault()
            setOpen(true)
          }
        }}
      >
        {props.children}
      </button>
      {open && (
        <div className={`menu-btn__pop${props.placement === 'top' ? ' menu-btn__pop--top' : ''}`} role="menu" aria-label={props.label} onKeyDown={onMenuKey}>
          <div className="menu-btn__title">{props.label}</div>
          {props.options.map((o, i) => {
            const on = o.value === props.value
            return (
              <button
                key={o.value}
                ref={(el) => {
                  itemRefs.current[i] = el
                }}
                role="menuitemradio"
                aria-checked={on}
                className={`menu-btn__item${on ? ' is-on' : ''}`}
                onClick={() => pick(o.value)}
              >
                <span className="menu-btn__icon">{o.icon}</span>
                <span className="menu-btn__label">{o.label}</span>
                <span className="menu-btn__check">{on && <IconCheck size={14} />}</span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
