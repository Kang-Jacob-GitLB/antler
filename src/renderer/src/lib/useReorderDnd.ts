import { useRef, useState } from 'react'
import { moveItem } from './repoOrder'

/**
 * 같은 묶음 안에서만 끌어 놓아 순서를 바꾸는 HTML5 드래그앤드롭.
 * kind+scope 가 같은 대상 위에서만 놓을 수 있다(환경↔환경, 같은 환경의 그룹↔그룹, 같은 그룹의 저장소↔저장소).
 * handle(끄는 손잡이)과 target(놓는 영역)을 따로 둬, 카드 머리를 끌어 카드 전체 위에 놓을 수 있다.
 * 다른 kind 를 끄는 중이면 target 은 이벤트를 막지 않아 바깥 대상(카드)까지 올라간다.
 */
export type DragKind = 'source' | 'group' | 'repo'

interface Item {
  kind: DragKind
  scope: string
  id: string
}

const MIME = 'application/x-antler-reorder'

export interface ReorderDnd {
  /** 끄는 손잡이에 붙일 속성 */
  handle(kind: DragKind, scope: string, id: string): React.HTMLAttributes<HTMLElement> & { draggable: boolean }
  /** 놓는 영역에 붙일 속성. list = 지금 화면 순서, onMove = 바뀐 순서 */
  target(kind: DragKind, scope: string, id: string, list: readonly string[], onMove: (next: string[]) => void): React.HTMLAttributes<HTMLElement>
  /** 그 항목에 붙일 상태 클래스(' is-dragging' · ' is-drop-before' · ' is-drop-after') */
  cls(kind: DragKind, scope: string, id: string): string
}

export function useReorderDnd(): ReorderDnd {
  const [drag, setDrag] = useState<Item | null>(null)
  const [over, setOver] = useState<(Item & { after: boolean }) | null>(null)
  // dragover 는 아주 자주 오므로 최신 값은 ref 로 읽는다
  const dragRef = useRef<Item | null>(null)
  const same = (a: Item | null, kind: DragKind, scope: string): a is Item => !!a && a.kind === kind && a.scope === scope
  const end = (): void => {
    dragRef.current = null
    setDrag(null)
    setOver(null)
  }
  const isAfter = (e: React.DragEvent<HTMLElement>): boolean => {
    const r = e.currentTarget.getBoundingClientRect()
    return e.clientY > r.top + r.height / 2
  }

  return {
    handle: (kind, scope, id) => ({
      draggable: true,
      onDragStart: (e) => {
        e.stopPropagation()
        e.dataTransfer.effectAllowed = 'move'
        e.dataTransfer.setData(MIME, id)
        const item = { kind, scope, id }
        dragRef.current = item
        setDrag(item)
      },
      onDragEnd: end
    }),
    target: (kind, scope, id, list, onMove) => ({
      onDragOver: (e) => {
        const d = dragRef.current
        if (!same(d, kind, scope)) return
        e.preventDefault()
        e.stopPropagation()
        e.dataTransfer.dropEffect = 'move'
        const after = isAfter(e)
        setOver((o) => (o && o.kind === kind && o.scope === scope && o.id === id && o.after === after ? o : { kind, scope, id, after }))
      },
      onDrop: (e) => {
        const d = dragRef.current
        if (!same(d, kind, scope)) return
        e.preventDefault()
        e.stopPropagation()
        const next = moveItem(list, d.id, id, isAfter(e))
        end()
        if (next.some((x, i) => x !== list[i])) onMove(next)
      }
    }),
    cls: (kind, scope, id) => {
      if (!same(drag, kind, scope)) return ''
      if (drag.id === id) return ' is-dragging'
      if (over && over.kind === kind && over.scope === scope && over.id === id) return over.after ? ' is-drop-after' : ' is-drop-before'
      return ''
    }
  }
}
