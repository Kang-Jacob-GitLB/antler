import { useRef } from 'react'
import './Splitter.css'

export interface SplitterHandlers {
  onDragStart(): void
  /** 드래그 중 호출. dx 는 드래그 시작점으로부터의 이동량(px) */
  onDrag(dx: number): void
  onDragEnd(): void
  /** 더블클릭 시 기본 폭으로 */
  onReset(): void
}

/** 패널 사이 세로 분할선. 포인터 캡처로 창 밖까지 드래그를 추적한다. */
export function Splitter({ onDragStart, onDrag, onDragEnd, onReset }: SplitterHandlers): React.JSX.Element {
  const start = useRef<number | null>(null)
  return (
    <div
      className="splitter"
      role="separator"
      aria-orientation="vertical"
      onDoubleClick={onReset}
      onPointerDown={(e) => {
        if (e.button !== 0) return
        start.current = e.clientX
        onDragStart()
        e.currentTarget.setPointerCapture(e.pointerId)
        document.body.classList.add('is-resizing')
      }}
      onPointerMove={(e) => {
        if (start.current !== null) onDrag(e.clientX - start.current)
      }}
      onPointerUp={(e) => {
        if (start.current === null) return
        start.current = null
        e.currentTarget.releasePointerCapture(e.pointerId)
        document.body.classList.remove('is-resizing')
        onDragEnd()
      }}
    />
  )
}
