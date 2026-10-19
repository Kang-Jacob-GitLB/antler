import { useEffect, type RefObject } from 'react'

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

/**
 * 모달 포커스 관리: 열릴 때의 포커스 요소를 기억했다가 닫힐 때 되돌리고(요소가 사라졌으면 생략),
 * 열려 있는 동안 Tab/Shift+Tab 이 모달 밖으로 나가지 않게 가둔다.
 */
export function useModalFocus(ref: RefObject<HTMLElement | null>): void {
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null
    const onKey = (e: KeyboardEvent): void => {
      const root = ref.current
      if (e.key !== 'Tab' || !root) return
      const items = [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null)
      e.preventDefault()
      if (items.length === 0) return
      const i = items.indexOf(document.activeElement as HTMLElement)
      const next = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : i < 0 || i === items.length - 1 ? 0 : i + 1
      items[next]!.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('keydown', onKey)
      if (prev && prev !== document.body && prev.isConnected) prev.focus()
    }
  }, [])
}
