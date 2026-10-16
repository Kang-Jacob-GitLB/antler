import { useRef } from 'react'
import './Splitter.css'

export interface SplitterHandlers {
  onDragStart(): void
  /** 드래그 중 호출. dx 는 드래그 시작점으로부터의 이동량(px) — 가로 분할선이면 세로 이동량 */
  onDrag(dx: number): void
  onDragEnd(): void
  /** 더블클릭 시 기본 크기로 */
  onReset(): void
}

/**
 * 패널 사이 분할선. 포인터 캡처로 창 밖까지 드래그를 추적한다.
 * 기본은 좌우 패널 사이의 세로 선, horizontal 이면 위아래 영역 사이의 가로 선.
 */
export function Splitter({
  onDragStart,
  onDrag,
  onDragEnd,
  onReset,
  horizontal = false
}: SplitterHandlers & { horizontal?: boolean }): React.JSX.Element {
  const start = useRef<number | null>(null)
  const resizingClass = horizontal ? 'is-resizing-h' : 'is-resizing'
  const pos = (e: React.PointerEvent): number => (horizontal ? e.clientY : e.clientX)
  return (
    <div
      className={horizontal ? 'splitter splitter--h' : 'splitter'}
      role="separator"
      aria-orientation={horizontal ? 'horizontal' : 'vertical'}
      onDoubleClick={onReset}
      onPointerDown={(e) => {
        if (e.button !== 0) return
        start.current = pos(e)
        onDragStart()
        e.currentTarget.setPointerCapture(e.pointerId)
        document.body.classList.add(resizingClass)
      }}
      onPointerMove={(e) => {
        if (start.current !== null) onDrag(pos(e) - start.current)
      }}
      onPointerUp={(e) => {
        if (start.current === null) return
        start.current = null
        e.currentTarget.releasePointerCapture(e.pointerId)
        document.body.classList.remove(resizingClass)
        onDragEnd()
      }}
    />
  )
}
