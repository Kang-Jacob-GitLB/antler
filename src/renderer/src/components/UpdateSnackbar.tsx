import { useEffect } from 'react'
import { useT } from '../i18n'
import { api } from '../lib/api'
import { useUpdateStore } from '../stores/updateStore'
import { IconClose } from './icons'
import './UpdateSnackbar.css'

const NOT_AVAILABLE_MS = 4000

/**
 * 앱 업데이트 스낵바(화면 아래 가운데 떠 있는 평면 카드).
 * 바깥 role="status" 영역은 항상 마운트해 두어, 안의 문구가 바뀔 때 스크린 리더가 정중하게(polite) 읽는다.
 */
export function UpdateSnackbar(): React.JSX.Element {
  const t = useT()
  const state = useUpdateStore((x) => x.state)
  const hiddenKey = useUpdateStore((x) => x.hiddenKey)
  const manual = useUpdateStore((x) => x.manual)

  useEffect(() => useUpdateStore.getState().init(), [])
  // 수동 확인 결과 '최신 버전'은 잠깐만 보여 준다
  useEffect(() => {
    if (state?.status !== 'not-available') return
    const timer = setTimeout(() => useUpdateStore.getState().hide(), NOT_AVAILABLE_MS)
    return () => clearTimeout(timer)
  }, [state])

  const { check, download, cancel, install, dismiss, hide } = useUpdateStore.getState()
  const s = state
  const v = s?.version ?? ''
  const visible =
    !!s &&
    `${s.status}|${s.version ?? ''}` !== hiddenKey &&
    (s.status === 'available' ||
      s.status === 'downloading' ||
      s.status === 'downloaded' ||
      s.status === 'error' ||
      (manual && (s.status === 'checking' || s.status === 'not-available')))

  const openNotes = (): void => {
    if (s?.notesUrl) void api.invoke('gh:openExternal', s.notesUrl).catch(() => {})
  }

  let msg = ''
  let actions: React.ReactNode = null
  let progress: number | null = null
  if (s && visible) {
    switch (s.status) {
      case 'checking':
        msg = t('update.checking')
        break
      case 'not-available':
        msg = t('update.upToDate', { v: s.currentVersion })
        break
      case 'available':
        msg = t('update.available', { v })
        actions = (
          <>
            <button className="btn btn--primary btn--sm" onClick={() => void download()}>
              {t('update.action')}
            </button>
            <button className="btn btn--sm" onClick={openNotes}>
              {t('update.notes')}
            </button>
            <button className="btn btn--sm" onClick={() => dismiss('later')}>
              {t('update.later')}
            </button>
            <button className="update-snack__link" onClick={() => dismiss('skip')}>
              {t('update.skip')}
            </button>
          </>
        )
        break
      case 'downloading':
        progress = s.progress ?? 0
        msg = t('update.downloading', { v, p: progress })
        actions = (
          <button className="btn btn--sm" onClick={cancel}>
            {t('update.cancel')}
          </button>
        )
        break
      case 'downloaded':
        if (s.mode === 'auto') {
          msg = t('update.downloaded', { v })
          actions = (
            <button className="btn btn--primary btn--sm" onClick={install}>
              {t('update.restart')}
            </button>
          )
        } else {
          const name = s.fileName ?? ''
          msg = t(s.mode === 'manual-dmg' ? 'update.openedDmg' : name.endsWith('.deb') ? 'update.openedDeb' : 'update.openedFile')
          actions = (
            <>
              <button className="btn btn--sm" onClick={install}>
                {t('update.reopen')}
              </button>
              {name && <span className="update-snack__file">{name}</span>}
            </>
          )
        }
        break
      case 'error':
        msg = t('update.error', { msg: s.error ?? '' })
        actions = (
          <button className="btn btn--primary btn--sm" onClick={() => void (s.version ? download() : check())}>
            {t('update.retry')}
          </button>
        )
        break
    }
  }

  return (
    <div className="update-live" role="status" aria-live="polite">
      {visible && s && (
        <div className={`update-snack${s.status === 'error' ? ' is-error' : ''}`}>
          <div className="update-snack__body">
            <p className="update-snack__msg">{msg}</p>
            {progress !== null && (
              <div
                className="update-snack__bar"
                role="progressbar"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progress}
                aria-label={t('update.downloading', { v, p: progress })}
              >
                <div className="update-snack__fill" style={{ width: `${progress}%` }} />
              </div>
            )}
            {actions && <div className="update-snack__actions">{actions}</div>}
          </div>
          <button className="icon-btn update-snack__x" title={t('update.close')} aria-label={t('update.close')} onClick={hide}>
            <IconClose size={14} />
          </button>
        </div>
      )}
    </div>
  )
}
