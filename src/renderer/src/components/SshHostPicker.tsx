import { useId, useState } from 'react'
import type { SshHostInfo } from '@shared/types'
import { useT } from '../i18n'
import './SshHostPicker.css'

interface Props {
  hosts: SshHostInfo[]
  value: string
  onChange: (v: string) => void
  placeholder?: string
  autoFocus?: boolean
}

/** 별칭 옆에 보여 줄 실제 접속 대상(`user@host:port via jump`) */
export function hostDetail(h: SshHostInfo): string {
  return `${h.user ? `${h.user}@` : ''}${h.hostName}${h.port !== 22 ? `:${h.port}` : ''}${h.proxyJump ? ` via ${h.proxyJump}` : ''}`
}

/**
 * `~/.ssh/config` 별칭을 고르는 입력칸. 직접 입력(`user@host:port`)도 된다.
 * `<datalist>` 를 쓰지 않는다 — macOS 의 WKWebView 는 datalist 제안을 띄우지 않아 목록이 보이지 않았다.
 */
export function SshHostPicker({ hosts, value, onChange, placeholder, autoFocus }: Props): React.JSX.Element {
  const t = useT()
  const listId = useId()
  const [open, setOpen] = useState(false)
  const [active, setActive] = useState(-1)

  const q = value.trim().toLowerCase()
  // 이미 고른 별칭과 정확히 같으면 전체를 보여 준다(다른 호스트로 바꾸기 쉽게)
  const exact = hosts.some((h) => h.alias.toLowerCase() === q)
  const shown = !q || exact ? hosts : hosts.filter((h) => h.alias.toLowerCase().includes(q) || hostDetail(h).toLowerCase().includes(q))
  const visible = open && shown.length > 0

  const pick = (h: SshHostInfo): void => {
    onChange(h.alias)
    setOpen(false)
    setActive(-1)
  }

  return (
    <div className="ssh-host-picker">
      <input
        className="field__input mono"
        role="combobox"
        aria-expanded={visible}
        aria-controls={listId}
        aria-autocomplete="list"
        aria-activedescendant={visible && active >= 0 ? `${listId}-${active}` : undefined}
        autoComplete="off"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        autoFocus={autoFocus}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onChange={(e) => {
          onChange(e.target.value)
          setOpen(true)
          setActive(-1)
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault()
            if (!visible) {
              setOpen(true)
              return
            }
            const step = e.key === 'ArrowDown' ? 1 : -1
            setActive((a) => (a + step + shown.length) % shown.length)
          } else if (e.key === 'Enter' && visible && shown[active]) {
            e.preventDefault()
            pick(shown[active])
          } else if (e.key === 'Escape' && visible) {
            // 목록만 닫고 대화상자는 닫지 않는다
            e.preventDefault()
            e.stopPropagation()
            setOpen(false)
          }
        }}
      />
      {visible && (
        <ul id={listId} className="ssh-host-picker__list" role="listbox" aria-label={t('add.hostList')}>
          {shown.map((h, i) => (
            <li
              key={h.alias}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              className={`ssh-host-picker__item${i === active ? ' is-active' : ''}`}
              // 입력칸의 포커스(blur 로 목록이 닫히는 것)를 지키고 고른다
              onMouseDown={(e) => {
                e.preventDefault()
                pick(h)
              }}
              onMouseEnter={() => setActive(i)}
            >
              <span className="ssh-host-picker__alias mono">{h.alias}</span>
              <span className="ssh-host-picker__detail mono">{hostDetail(h)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
