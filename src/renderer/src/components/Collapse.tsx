import { useEffect, useRef, useState } from 'react'
import './Collapse.css'

/** 전환이 끝났다고 보는 상한(ms). transitionend 가 오지 않아도(동작 줄이기 설정 등) 다음 상태로 넘어간다 */
const SETTLE_FALLBACK_MS = 320

/**
 * 열림 상태가 바뀔 때 나타남/사라짐을 애니메이션할 수 있도록 상태를 나눠 준다.
 * - mounted: DOM 에 둘지(닫히는 애니메이션 동안은 남겨 둔다)
 * - shown: 열린 모양으로 그릴지(열릴 때는 두 프레임 늦게 true 가 되어 전환이 일어난다)
 * - settled: 열린 상태로 전환이 끝났는지(끝난 뒤에만 넘치는 내용을 잘라 내지 않는다)
 * 처음 그릴 때는 애니메이션하지 않는다.
 */
export function usePresence(open: boolean): { mounted: boolean; shown: boolean; settled: boolean; onTransitionEnd(e: React.TransitionEvent): void } {
  const [mounted, setMounted] = useState(open)
  const [shown, setShown] = useState(open)
  const [settled, setSettled] = useState(open)
  const openRef = useRef(open)
  openRef.current = open
  const first = useRef(true)

  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    setSettled(false)
    if (open) {
      setMounted(true)
      // 닫힌 모양으로 한 번 그린 뒤 열린 모양으로 바꿔야 전환이 보인다
      let raf2 = 0
      const raf1 = requestAnimationFrame(() => {
        raf2 = requestAnimationFrame(() => setShown(true))
      })
      const timer = setTimeout(() => openRef.current && setSettled(true), SETTLE_FALLBACK_MS)
      return () => {
        cancelAnimationFrame(raf1)
        cancelAnimationFrame(raf2)
        clearTimeout(timer)
      }
    }
    setShown(false)
    const timer = setTimeout(() => !openRef.current && setMounted(false), SETTLE_FALLBACK_MS)
    return () => clearTimeout(timer)
  }, [open])

  const onTransitionEnd = (e: React.TransitionEvent): void => {
    if (e.target !== e.currentTarget) return
    if (openRef.current) setSettled(true)
    else setMounted(false)
  }
  return { mounted, shown, settled, onTransitionEnd }
}

/**
 * 좌우 패널(사이드바·상세)을 열고 닫을 때 폭을 부드럽게 바꾼다. 안쪽 패널은 고정 폭이라 내용이 다시 배치되지 않는다.
 * 전환은 열기/닫기 때만 켠다 — 분할선으로 폭을 끌 때나 저장된 폭을 처음 읽을 때는 즉시 반영한다.
 */
export function SidePane({
  open,
  width,
  side,
  children
}: {
  open: boolean
  width: number
  side: 'left' | 'right'
  children: React.ReactNode
}): React.JSX.Element | null {
  const { mounted, shown, settled, onTransitionEnd } = usePresence(open)
  if (!mounted) return null
  const animating = !settled
  return (
    <div
      className={`side-pane side-pane--${side}${animating ? ' is-animating' : ''}`}
      style={{ width: shown ? width : 0 }}
      onTransitionEnd={onTransitionEnd}
    >
      {children}
    </div>
  )
}

/** 높이가 부드럽게 늘고 주는 접기 영역. 닫혀 있으면 내용을 렌더하지 않는다 */
export function Collapse({ open, children }: { open: boolean; children: React.ReactNode }): React.JSX.Element | null {
  const { mounted, shown, settled, onTransitionEnd } = usePresence(open)
  if (!mounted) return null
  return (
    <div className={`collapse${shown ? ' is-open' : ''}${settled ? ' is-settled' : ''}`} onTransitionEnd={onTransitionEnd}>
      <div className="collapse__inner">{children}</div>
    </div>
  )
}
