import { useEffect, useRef } from 'react'
import { useT } from '../i18n'
import { useSearchStore } from '../stores/searchStore'
import { IconChevronDown, IconClose, IconSearch } from './icons'
import './SearchBar.css'

const DEBOUNCE_MS = 250

/** Ctrl/Cmd+F 커밋 검색(메시지·작성자·해시). 일치하지 않는 행은 흐리게 하고 Enter 로 다음 일치로 이동한다. */
export function SearchBar(): React.JSX.Element | null {
  const t = useT()
  const s = useSearchStore()
  const inputRef = useRef<HTMLInputElement>(null)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)

  // 열 때와 Ctrl/Cmd+F 를 다시 누를 때마다 입력창을 선택한다
  useEffect(() => {
    if (s.open) inputRef.current?.select()
  }, [s.open, s.focusSeq])

  useEffect(() => () => clearTimeout(timer.current), [])

  if (!s.open) return null

  const status = s.loading
    ? t('search.searching')
    : s.error
      ? s.error
      : s.query.trim() && s.query.trim() === s.ranQuery
        ? s.hits.length
          ? t('search.position', { n: s.cursor + 1, total: `${s.hits.length}${s.truncated ? '+' : ''}` })
          : t('search.none')
        : ''

  return (
    <div className="search-bar" role="search">
      <IconSearch className="search-bar__icon" />
      <input
        ref={inputRef}
        className="search-bar__input"
        placeholder={t('search.placeholder')}
        value={s.query}
        autoFocus
        onChange={(e) => {
          s.setQuery(e.target.value)
          clearTimeout(timer.current)
          timer.current = setTimeout(() => void useSearchStore.getState().run(), DEBOUNCE_MS)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            s.close()
          } else if (e.key === 'Enter') {
            e.preventDefault()
            clearTimeout(timer.current)
            // 입력이 마지막으로 실행한 질의와 다르면 새로 검색, 같으면(검색이 끝났을 때) 다음/이전 일치로 이동
            if (s.query.trim() !== s.ranQuery) void s.run()
            else if (!s.loading) s.step(e.shiftKey ? -1 : 1)
          }
        }}
      />
      <span className={`search-bar__status${s.error ? ' is-error' : ''}`}>{status}</span>
      <button className="icon-btn icon-btn--sm" title={t('search.prev')} onClick={() => s.step(-1)} disabled={!s.hits.length}>
        <IconChevronDown size={14} style={{ transform: 'rotate(180deg)' }} />
      </button>
      <button className="icon-btn icon-btn--sm" title={t('search.next')} onClick={() => s.step(1)} disabled={!s.hits.length}>
        <IconChevronDown size={14} />
      </button>
      <button className="icon-btn icon-btn--sm" title={t('search.close')} onClick={s.close}>
        <IconClose size={13} />
      </button>
    </div>
  )
}
