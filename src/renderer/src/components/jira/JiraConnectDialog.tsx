import { useEffect, useRef, useState } from 'react'
import type { JiraKind } from '@shared/types'
import { useT } from '../../i18n'
import { errorMessage } from '../../lib/api'
import { parseProjectKeys } from '../../lib/issueKeys'
import { useAppStore } from '../../stores/appStore'
import { useJiraStore } from '../../stores/jiraStore'
import { useModalFocus } from '../useModalFocus'
import '../AddRepoDialog.css'
import './JiraView.css'

const NO_KEYS: string[] = []

/** Jira 연결 설정: Cloud(이메일 + API 토큰) / Data Center·Server(Personal Access Token), 이슈 키로 인식할 프로젝트 키 */
export function JiraConnectDialog(): React.JSX.Element | null {
  const open = useJiraStore((s) => s.dialogOpen)
  return open ? <Dialog /> : null
}

function Dialog(): React.JSX.Element {
  const t = useT()
  const auth = useJiraStore((s) => s.auth)
  const { connect, disconnect, setDialogOpen } = useJiraStore.getState()
  const savedKeys = useAppStore((s) => s.config?.ui.jiraProjectKeys) ?? NO_KEYS
  const [kind, setKind] = useState<JiraKind>(auth.kind ?? 'cloud')
  const [siteUrl, setSiteUrl] = useState(auth.siteUrl ?? '')
  const [email, setEmail] = useState(auth.email ?? '')
  const [token, setToken] = useState('')
  const [keys, setKeys] = useState(savedKeys.join(', '))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const ref = useRef<HTMLFormElement>(null)
  useModalFocus(ref)

  const close = (): void => setDialogOpen(false)
  // 토큰은 입력창에만 있고, 창을 닫으면 사라진다
  useEffect(() => () => setToken(''), [])

  const saveKeys = (): void => {
    const next = parseProjectKeys(keys)
    if (next.join(',') !== savedKeys.join(',')) void useAppStore.getState().setUi({ jiraProjectKeys: next.length ? next : undefined })
  }

  const submit = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await connect({ kind, siteUrl: siteUrl.trim(), token: token.trim(), ...(kind === 'cloud' ? { email: email.trim() } : {}) })
      setToken('')
      saveKeys()
      close()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  const ready = !!siteUrl.trim() && !!token.trim() && (kind === 'server' || !!email.trim())

  return (
    <div className="modal-backdrop" onMouseDown={close}>
      <form
        ref={ref}
        className="add-dialog jira-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="jira-title"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === 'Escape' && close()}
        onSubmit={(e) => {
          e.preventDefault()
          if (ready && !busy) void submit()
        }}
      >
        <h2 id="jira-title" className="add-dialog__title">
          {t('jira.title')}
        </h2>

        {auth.connected ? (
          <div className="field">
            <div className="field__label">{t('jira.connected')}</div>
            <div className="jira-conn selectable">
              <strong>{auth.displayName || auth.email || ''}</strong>
              <span className="gh-muted mono">{auth.siteUrl}</span>
              <span className="gh-muted">{auth.kind === 'cloud' ? t('jira.kind.cloud') : t('jira.kind.server')}</span>
            </div>
            {auth.storageUnavailable && <div className="field__hint is-warn">{t('gh.auth.noStorage')}</div>}
            <div className="field__row">
              <button
                type="button"
                className="btn btn--sm"
                onClick={() => {
                  void disconnect().then(close, (err) => setError(errorMessage(err)))
                }}
              >
                {t('jira.disconnect')}
              </button>
            </div>
          </div>
        ) : (
          <>
            <div className="seg add-dialog__tabs" role="tablist">
              {(['cloud', 'server'] as const).map((k) => (
                <button key={k} type="button" role="tab" aria-selected={kind === k} className={`seg__btn${kind === k ? ' is-on' : ''}`} onClick={() => setKind(k)}>
                  {t(k === 'cloud' ? 'jira.kind.cloud' : 'jira.kind.server')}
                </button>
              ))}
            </div>
            <label className="field">
              <span className="field__label">{t('jira.site')}</span>
              <input
                className="jira-input mono"
                value={siteUrl}
                autoFocus
                spellCheck={false}
                placeholder={kind === 'cloud' ? 'https://your-team.atlassian.net' : 'https://jira.example.com'}
                onChange={(e) => setSiteUrl(e.target.value)}
              />
              <span className="field__hint">{t('jira.siteHint')}</span>
            </label>
            {kind === 'cloud' && (
              <label className="field">
                <span className="field__label">{t('jira.email')}</span>
                <input className="jira-input" type="email" value={email} autoComplete="off" spellCheck={false} placeholder="you@example.com" onChange={(e) => setEmail(e.target.value)} />
              </label>
            )}
            <label className="field">
              <span className="field__label">{kind === 'cloud' ? t('jira.apiToken') : t('jira.pat')}</span>
              <input className="jira-input mono" type="password" value={token} autoComplete="off" spellCheck={false} onChange={(e) => setToken(e.target.value)} />
              <span className="field__hint">{t(kind === 'cloud' ? 'jira.apiTokenHint' : 'jira.patHint')}</span>
            </label>
          </>
        )}

        <label className="field">
          <span className="field__label">{t('jira.projectKeys')}</span>
          <input className="jira-input mono" value={keys} spellCheck={false} placeholder="ABC, DEF" onChange={(e) => setKeys(e.target.value)} onBlur={saveKeys} />
          <span className="field__hint">{t('jira.projectKeysHint')}</span>
        </label>

        {error && <div className="field__hint is-warn jira-error">{error}</div>}
        <div className="jira-dialog__foot">
          <button type="button" className="btn" onClick={close}>
            {t('jira.close')}
          </button>
          {!auth.connected && (
            <button type="submit" className="btn btn--primary" disabled={!ready || busy}>
              {busy ? t('add.adding') : t('jira.connect')}
            </button>
          )}
        </div>
      </form>
    </div>
  )
}
