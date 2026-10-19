import { useEffect, useMemo, useRef, useState } from 'react'
import { useT } from '../i18n'
import { filterCommands } from '../lib/commandFilter'
import { CATEGORY_KEY, readCommandCtx, resolveCommands, type ResolvedCommand } from '../lib/commands'
import { matchesShortcut, shortcutParts } from '../lib/shortcuts'
import { useAppStore } from '../stores/appStore'
import { usePaletteStore } from '../stores/paletteStore'
import { IconCheck, IconSearch } from './icons'
import { Keys } from './Keys'
import { useModalFocus } from './useModalFocus'
import './RepoSwitcher.css' // 저장소 전환과 같은 팝업 틀(.switcher*)을 쓴다
import './CommandPalette.css'

const PALETTE = { code: 'KeyP', shift: true } as const

/** 제목에서 일치한 글자(코드 포인트 위치)를 강조해 그린다 */
function Highlighted({ text, indices }: { text: string; indices: number[] }): React.JSX.Element {
  if (indices.length === 0) return <>{text}</>
  const hit = new Set(indices)
  return (
    <>
      {[...text].map((ch, i) =>
        hit.has(i) ? (
          <mark key={i} className="palette__hit">
            {ch}
          </mark>
        ) : (
          ch
        )
      )}
    </>
  )
}

/** Ctrl/Cmd+Shift+P 명령 팔레트: 명령을 퍼지 검색해 실행한다. 저장소 전환(Ctrl+P)과 같은 모양의 팝업 */
export function CommandPalette(): React.JSX.Element | null {
  const open = usePaletteStore((s) => s.paletteOpen)
  if (!open) return null
  return <PaletteBody />
}

function PaletteBody(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const [query, setQuery] = useState('')
  const [cursor, setCursor] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLUListElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  useModalFocus(dialogRef)
  const mac = window.antler.platform === 'darwin'

  // 열린 시점의 상태로 보일 명령을 정한다(열려 있는 동안 상태는 바뀌지 않는다)
  const all = useMemo(() => resolveCommands(t, readCommandCtx()), [locale])
  const matches = useMemo(() => filterCommands(all, query), [all, query])

  useEffect(() => {
    inputRef.current?.focus()
    // 열려 있는 동안 전역 단축키는 멈춰 있으므로, 같은 키로 다시 눌러 닫는 것은 여기서 받는다(입력 칸에 포커스가 없어도 동작)
    const onKey = (e: KeyboardEvent): void => {
      if (matchesShortcut(e, PALETTE)) {
        e.preventDefault()
        usePaletteStore.getState().setPaletteOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' })
  }, [cursor, matches])

  const close = (): void => usePaletteStore.getState().setPaletteOpen(false)
  const run = (cmd: ResolvedCommand | undefined): void => {
    if (!cmd) return
    // 먼저 닫아 포커스·키 처리를 비운 뒤 실행한다(다음 팝업을 여는 명령이 있다)
    close()
    cmd.run()
  }

  return (
    <div className="switcher-backdrop" onMouseDown={close}>
      <div
        ref={dialogRef}
        className="switcher palette"
        role="dialog"
        aria-modal="true"
        aria-label={t('cmd.palette')}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="switcher__input-row">
          <IconSearch className="switcher__icon" />
          <input
            ref={inputRef}
            className="switcher__input"
            placeholder={t('palette.placeholder')}
            value={query}
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={matches[cursor] ? `palette-${matches[cursor]!.item.id}` : undefined}
            onChange={(e) => {
              setQuery(e.target.value)
              setCursor(0)
            }}
            onKeyDown={(e) => {
              // 한글 조합 중의 Enter·방향키는 입력기가 쓴다
              if (e.nativeEvent.isComposing) return
              if (e.key === 'Escape') {
                e.preventDefault()
                close()
              } else if (e.key === 'Enter') {
                e.preventDefault()
                run(matches[cursor]?.item)
              } else if (e.key === 'ArrowDown') {
                e.preventDefault()
                setCursor((c) => Math.min(c + 1, matches.length - 1))
              } else if (e.key === 'ArrowUp') {
                e.preventDefault()
                setCursor((c) => Math.max(c - 1, 0))
              }
            }}
          />
        </div>
        <ul id="palette-list" ref={listRef} className="switcher__list" role="listbox">
          {matches.length === 0 && <li className="switcher__none">{t('palette.none')}</li>}
          {matches.map(({ item, indices }, i) => (
            <li
              key={item.id}
              id={`palette-${item.id}`}
              role="option"
              aria-selected={i === cursor}
              className={`switcher__item palette__item${i === cursor ? ' is-active' : ''}`}
              onMouseMove={() => i !== cursor && setCursor(i)}
              onClick={() => run(item)}
            >
              <span className="palette__check" aria-hidden="true">
                {item.checked && <IconCheck size={13} />}
              </span>
              <span className="palette__title">
                <Highlighted text={item.label} indices={indices} />
              </span>
              <span className="palette__cat">{t(CATEGORY_KEY[item.category])}</span>
              {item.shortcut && <Keys parts={shortcutParts(item.shortcut, mac)} mac={mac} />}
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
