import { useEffect, useRef, useState } from 'react'
import type { JiraTransition } from '@shared/types'
import { useT } from '../../i18n'
import { api, errorMessage } from '../../lib/api'
import { useJiraStore } from '../../stores/jiraStore'
import { JiraStatus } from './JiraBits'
import '../MenuButton.css'

/**
 * 상세의 상태 칩 = 상태 전환 메뉴 버튼. 누르면 지금 실행할 수 있는 전환을 불러와 드롭다운으로 보이고, 고르면 전환을 실행한다.
 * 메뉴 모양은 MenuButton 과 같은 CSS 를 쓰고, WAI-ARIA menu 패턴(↑↓·Home·End 이동, Esc·바깥 클릭으로 닫힘)을 따른다.
 */
export function JiraTransitionMenu({ issueKey, status, category }: { issueKey: string; status: string; category: string }): React.JSX.Element {
  const t = useT()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [items, setItems] = useState<JiraTransition[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([])

  // 이슈나 상태가 바뀌면 메뉴를 닫고 전환 목록을 버린다(전환 가능 목록은 현재 상태에 따라 달라진다)
  useEffect(() => {
    setOpen(false)
    setItems(null)
    setError(null)
  }, [issueKey, status])
  useEffect(() => setBusy(false), [issueKey])

  useEffect(() => {
    if (!open) return
    setItems(null)
    setError(null)
    let alive = true
    api
      .invoke('jira:transitions', issueKey)
      .then((list) => alive && setItems(list))
      .catch((err) => alive && setError(errorMessage(err)))
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => {
      alive = false
      document.removeEventListener('mousedown', onDown)
    }
  }, [open, issueKey])

  // 목록이 도착하면 첫 항목에 포커스
  useEffect(() => {
    if (open && items?.length) itemRefs.current[0]?.focus()
  }, [open, items])

  const close = (refocus: boolean): void => {
    setOpen(false)
    if (refocus) btnRef.current?.focus()
  }
  const pick = async (tr: JiraTransition): Promise<void> => {
    close(true)
    setBusy(true)
    try {
      await useJiraStore.getState().transitionIssue(issueKey, tr.id)
    } finally {
      setBusy(false)
    }
  }
  const onMenuKey = (e: React.KeyboardEvent): void => {
    const els = itemRefs.current.filter((x): x is HTMLButtonElement => !!x)
    const i = els.indexOf(document.activeElement as HTMLButtonElement)
    const move = (n: number): void => {
      e.preventDefault()
      els[(n + els.length) % els.length]?.focus()
    }
    if (e.key === 'ArrowDown') move(i + 1)
    else if (e.key === 'ArrowUp') move(i - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(els.length - 1)
    else if (e.key === 'Escape' || e.key === 'Tab') {
      e.stopPropagation()
      close(e.key === 'Escape')
    }
  }

  return (
    <div className="menu-btn" ref={ref}>
      <button
        ref={btnRef}
        className="jira-status-btn"
        title={t('jira.transition.title')}
        aria-label={`${t('jira.transition.title')}: ${status}`}
        aria-haspopup="menu"
        aria-expanded={open}
        // disabled 로 두면 포커스가 body 로 떨어지므로 aria-disabled 로 막는다
        aria-disabled={busy}
        onClick={() => !busy && setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && open) {
            // 전역 Esc(이슈 상세 닫기)로 넘기지 않고 메뉴만 닫는다
            e.preventDefault()
            e.stopPropagation()
            close(true)
          } else if (e.key === 'ArrowDown' && !open && !busy) {
            e.preventDefault()
            setOpen(true)
          }
        }}
      >
        <JiraStatus name={status} category={category} />
        <span aria-hidden="true">▾</span>
      </button>
      {open && (
        <div className="menu-btn__pop" style={{ left: 0, right: 'auto' }} role="menu" aria-label={t('jira.transition.title')} onKeyDown={onMenuKey}>
          <div className="menu-btn__title">{t('jira.transition.title')}</div>
          {!items && !error && <div className="menu-btn__title">{t('jira.transition.loading')}</div>}
          {error && (
            <div className="menu-btn__title" role="alert">
              {error}
            </div>
          )}
          {items?.length === 0 && <div className="menu-btn__title">{t('jira.transition.none')}</div>}
          {items?.map((tr, i) => (
            <button
              key={tr.id}
              ref={(el) => {
                itemRefs.current[i] = el
              }}
              role="menuitem"
              className="menu-btn__item"
              onClick={() => void pick(tr)}
            >
              <span className="menu-btn__label">{tr.name}</span>
              <JiraStatus name={tr.to.name} category={tr.to.category} />
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
