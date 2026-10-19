import { useEffect, useMemo, useRef, useState } from 'react'
import type { ReleaseNotes } from '@shared/types'
import { useT } from '../i18n'
import { api } from '../lib/api'
import { formatDate } from '../lib/format'
import { markdownToHtml } from '../lib/markdown'
import { useAppStore } from '../stores/appStore'
import { usePaletteStore } from '../stores/paletteStore'
import { HtmlBody } from './github/GhBits'
import { IconClose, IconGlobe } from './icons'
import { useModalFocus } from './useModalFocus'
import './CommandPalette.css'
// 마크다운 본문 모양(.gh-body)은 PR·이슈 본문과 같은 것을 쓴다
import './github/GitHubView.css'
import './ReleaseNotesDialog.css'

/** 현재 버전의 릴리스 노트를 마크다운으로 렌더링해 보여 준다. '더보기'는 GitHub 릴리스 페이지 */
export function ReleaseNotesDialog(): React.JSX.Element | null {
  const open = usePaletteStore((s) => s.notesOpen)
  if (!open) return null
  return <Body />
}

type Load = { status: 'loading' } | { status: 'error'; error: string } | { status: 'ok'; notes: ReleaseNotes }

function Body(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const ref = useRef<HTMLDivElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  useModalFocus(ref)
  const [load, setLoad] = useState<Load>({ status: 'loading' })
  const close = (): void => usePaletteStore.getState().setNotesOpen(false)

  useEffect(() => {
    let alive = true
    api
      .invoke('update:releaseNotes')
      .then((notes) => alive && setLoad({ status: 'ok', notes }))
      .catch((err: unknown) => alive && setLoad({ status: 'error', error: err instanceof Error ? err.message : String(err) }))
    bodyRef.current?.focus()
    // 열려 있는 동안 전역 단축키는 멈춰 있으므로 Esc 는 여기서 직접 받는다
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => {
      alive = false
      window.removeEventListener('keydown', onKey)
    }
  }, [])

  const notes = load.status === 'ok' ? load.notes : null
  const html = useMemo(() => (notes?.body ? markdownToHtml(notes.body) : ''), [notes])
  const more = (): void => {
    void api.invoke('gh:openExternal', notes?.url ?? 'https://github.com/Kang-Jacob-GitLB/antler/releases').catch(() => {})
  }

  return (
    <div className="switcher-backdrop" onMouseDown={close}>
      <div
        ref={ref}
        className="shortcuts notes"
        role="dialog"
        aria-modal="true"
        aria-labelledby="notes-title"
        tabIndex={-1}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="shortcuts__head">
          <div className="notes__heading">
            <h2 id="notes-title" className="shortcuts__title">
              {t('notes.title')}
              {notes && <span className="notes__version">v{notes.version}</span>}
            </h2>
            {notes?.publishedAt && <span className="notes__date">{formatDate(Date.parse(notes.publishedAt) / 1000, locale)}</span>}
          </div>
          <button className="icon-btn icon-btn--sm" title={t('shortcuts.close')} aria-label={t('shortcuts.close')} onClick={close}>
            <IconClose size={13} />
          </button>
        </div>
        <div ref={bodyRef} className="shortcuts__body notes__body" tabIndex={0}>
          {load.status === 'loading' && <div className="notes__state">{t('notes.loading')}</div>}
          {load.status === 'error' && <div className="notes__state is-error">{t('notes.error', { error: load.error })}</div>}
          {notes && !notes.exact && <div className="notes__fallback">{t('notes.fallback', { version: notes.version })}</div>}
          {notes && (html ? <HtmlBody html={html} className="notes__md" /> : <div className="notes__state">{t('notes.empty')}</div>)}
          {notes?.truncated && <div className="notes__state">{t('notes.truncated')}</div>}
        </div>
        <div className="notes__foot">
          <button className="btn btn--sm" onClick={more}>
            <IconGlobe size={12} />
            {t('notes.more')}
          </button>
        </div>
      </div>
    </div>
  )
}
