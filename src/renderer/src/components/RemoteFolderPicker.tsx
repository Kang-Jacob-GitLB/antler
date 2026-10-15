import { useCallback, useEffect, useRef, useState } from 'react'
import type { RemoteDirListing } from '@shared/types'
import { useT } from '../i18n'
import { api, errorMessage } from '../lib/api'
import { IconChevronRight, IconFolder, IconRepo } from './icons'
import './RemoteFolderPicker.css'

/**
 * SSH 원격 폴더 탐색(저장소 추가 창 안에서 띄운다). OS 폴더 선택 창은 원격을 볼 수 없으므로 SFTP 목록으로 둘러본다.
 * 더블클릭·Enter 로 들어가고, [이 폴더 선택] 은 선택한 하위 폴더(없으면 지금 폴더)를 고른다.
 */
export function RemoteFolderPicker(props: {
  host: string
  initialPath: string
  onPick: (path: string) => void
  onClose: () => void
}): React.JSX.Element {
  const t = useT()
  const [listing, setListing] = useState<RemoteDirListing | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [address, setAddress] = useState(props.initialPath || '~')
  const [selected, setSelected] = useState<string | null>(null)
  const seq = useRef(0)
  const listRef = useRef<HTMLDivElement>(null)

  const load = useCallback(
    async (path: string): Promise<void> => {
      const s = ++seq.current
      setLoading(true)
      setError(null)
      try {
        const r = await api.invoke('env:listDir', { sshHost: props.host, path })
        if (s !== seq.current) return
        setListing(r)
        setAddress(r.path)
        setSelected(null)
        listRef.current?.focus()
      } catch (err) {
        if (s !== seq.current) return
        setError(errorMessage(err))
      } finally {
        if (s === seq.current) setLoading(false)
      }
    },
    [props.host]
  )

  useEffect(() => {
    void load(props.initialPath || '~')
  }, [load, props.initialPath])

  const join = (base: string, name: string): string => (base.endsWith('/') ? base + name : `${base}/${name}`)
  const enter = (name: string): void => {
    if (listing) void load(join(listing.path, name))
  }
  const up = (): void => {
    if (listing?.parent) void load(listing.parent)
  }
  const pickPath = listing ? (selected ? join(listing.path, selected) : listing.path) : null
  const pickIsRepo = listing ? (selected ? listing.dirs.find((d) => d.name === selected)?.isRepo : listing.isRepo) : false

  const onListKey = (e: React.KeyboardEvent): void => {
    if (!listing) return
    const names = listing.dirs.map((d) => d.name)
    const i = selected ? names.indexOf(selected) : -1
    if (e.key === 'ArrowDown' && names.length) {
      e.preventDefault()
      setSelected(names[Math.min(names.length - 1, i + 1)]!)
    } else if (e.key === 'ArrowUp' && names.length) {
      e.preventDefault()
      setSelected(names[Math.max(0, i - 1)]!)
    } else if (e.key === 'Enter' && selected) {
      e.preventDefault()
      enter(selected)
    } else if (e.key === 'Backspace') {
      e.preventDefault()
      up()
    }
  }

  return (
    <div
      className="rfp"
      role="dialog"
      aria-label={t('add.remoteBrowseTitle', { host: props.host })}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          props.onClose()
        }
      }}
    >
      <div className="rfp__head">
        <span className="rfp__title">{t('add.remoteBrowseTitle', { host: props.host })}</span>
      </div>
      {/* 저장소 추가 창(form) 안이라 form 을 중첩하지 않는다 — Enter 는 직접 처리한다 */}
      <div className="rfp__addr">
        <button type="button" className="btn btn--sm" disabled={!listing?.parent || loading} onClick={up} title={t('add.up')}>
          ..
        </button>
        <input
          className="field__input mono"
          value={address}
          spellCheck={false}
          aria-label={t('add.path')}
          onChange={(e) => setAddress(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              void load(address)
            }
          }}
        />
        <button type="button" className="btn btn--sm" disabled={loading} onClick={() => void load(address)}>
          {t('add.go')}
        </button>
      </div>

      <div className="rfp__list" ref={listRef} tabIndex={0} role="listbox" aria-busy={loading} onKeyDown={onListKey}>
        {error ? (
          <div className="rfp__msg is-error" role="alert">
            {error}
          </div>
        ) : !listing ? (
          <div className="rfp__msg">{t('add.testing')}</div>
        ) : listing.dirs.length === 0 ? (
          <div className="rfp__msg">{t('add.noSubdirs')}</div>
        ) : (
          listing.dirs.map((d) => (
            <div
              key={d.name}
              role="option"
              aria-selected={selected === d.name}
              className={`rfp__item${selected === d.name ? ' is-sel' : ''}`}
              onClick={() => setSelected(d.name)}
              onDoubleClick={() => enter(d.name)}
            >
              {d.isRepo ? <IconRepo size={14} /> : <IconFolder size={14} />}
              <span className="rfp__name">{d.name}</span>
              {d.isRepo && <span className="rfp__badge">git</span>}
              <IconChevronRight size={12} className="rfp__chev" />
            </div>
          ))
        )}
        {listing?.truncated && <div className="rfp__msg">{t('add.truncated')}</div>}
      </div>

      <div className="rfp__foot">
        <span className="rfp__pick mono" title={pickPath ?? ''}>
          {pickPath}
          {pickIsRepo && <span className="rfp__badge">git</span>}
        </span>
        <button type="button" className="btn" onClick={props.onClose}>
          {t('add.cancel')}
        </button>
        <button type="button" className="btn btn--primary" disabled={!pickPath || loading} onClick={() => pickPath && props.onPick(pickPath)}>
          {t('add.pickThis')}
        </button>
      </div>
    </div>
  )
}
