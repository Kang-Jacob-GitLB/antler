import { useEffect, useRef, useState } from 'react'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { useAppStore } from '../../stores/appStore'
import { useGithubStore } from '../../stores/githubStore'
import { Avatar } from './GhBits'
import './GhAuthButton.css'

/** OAuth App 등록 화면(Device Flow 용 client ID 를 만드는 곳) */
const NEW_OAUTH_APP_URL = 'https://github.com/settings/applications/new'

/** GitHub 마크(단색, currentColor) */
const GhMark = ({ size = 16 }: { size?: number }): React.JSX.Element => (
  <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
    <path
      fill="currentColor"
      d="M8 0C3.58 0 0 3.58 0 8a8 8 0 0 0 5.47 7.59c.4.07.55-.17.55-.38v-1.33c-2.23.48-2.7-1.07-2.7-1.07-.36-.92-.89-1.17-.89-1.17-.73-.5.06-.49.06-.49.8.06 1.23.83 1.23.83.72 1.22 1.87.87 2.33.67.07-.52.28-.87.5-1.07-1.78-.2-3.65-.89-3.65-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48v2.2c0 .21.15.46.55.38A8 8 0 0 0 16 8c0-4.42-3.58-8-8-8z"
    />
  </svg>
)

/** 툴바의 GitHub 로그인 상태 버튼 + 팝오버(PAT / Device Flow / 로그아웃 / client ID 설정) */
export function GhAuthButton(): React.JSX.Element {
  const t = useT()
  const { auth, device, devicePending, authError, refreshAuth, loginPat, importCli, startDevice, cancelDevice, logout } = useGithubStore()
  const open = useGithubStore((s) => s.authPopoverOpen)
  const setOpen = useGithubStore((s) => s.setAuthPopoverOpen)
  const clientId = useAppStore((s) => s.config?.ui.githubClientId ?? '')
  const setUi = useAppStore((s) => s.setUi)
  const [pat, setPat] = useState('')
  const [busy, setBusy] = useState(false)
  const [cliBusy, setCliBusy] = useState(false)
  const [cid, setCid] = useState(clientId)
  // 고급(client ID) 접힘 상태. client ID 없이 '브라우저로 로그인'을 누르면 펼쳐서 안내한다
  const [advOpen, setAdvOpen] = useState(false)
  const [needCid, setNeedCid] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const cidRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    void refreshAuth()
  }, [refreshAuth])
  useEffect(() => setCid(clientId), [clientId])
  useEffect(() => {
    if (clientId) setNeedCid(false)
  }, [clientId])

  // client ID 가 없을 때 버튼을 비활성으로 두면 눌러도 반응이 없어 보인다 — 대신 설정 칸을 펼쳐 안내한다
  const onDevice = (): void => {
    if (clientId) {
      void startDevice()
      return
    }
    setNeedCid(true)
    setAdvOpen(true)
    requestAnimationFrame(() => cidRef.current?.focus())
  }
  const saveCid = (): void => void setUi({ githubClientId: cid.trim() || undefined })

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open, setOpen])

  const submitPat = async (): Promise<void> => {
    if (!pat.trim()) return
    setBusy(true)
    try {
      await loginPat(pat.trim())
      setPat('')
    } catch {
      /* 오류는 authError 로 보인다 */
    } finally {
      setBusy(false)
    }
  }

  const submitCli = async (): Promise<void> => {
    setCliBusy(true)
    try {
      await importCli()
    } catch {
      /* 오류는 authError 로 보인다 */
    } finally {
      setCliBusy(false)
    }
  }

  const resetMin = auth.rateReset ? Math.max(0, Math.ceil((auth.rateReset * 1000 - Date.now()) / 60000)) : null

  return (
    <div
      className="gh-auth"
      ref={ref}
      // 팝오버가 열려 있으면 Esc 는 팝오버만 닫는다(뒤의 GitHub 화면까지 닫히지 않게 전파를 막는다)
      onKeyDown={(e) => {
        if (open && e.key === 'Escape') {
          e.stopPropagation()
          setOpen(false)
        }
      }}
    >
      <button
        className={`icon-btn gh-auth__btn${auth.signedIn ? ' is-on' : ''}`}
        title={auth.signedIn ? t('gh.auth.signedInAs', { login: auth.user?.login ?? '' }) : t('gh.auth.signIn')}
        aria-pressed={open}
        onClick={() => setOpen(!open)}
      >
        {auth.signedIn && auth.user ? <Avatar user={auth.user} size={18} /> : <GhMark />}
      </button>
      {open && (
        <div className="gh-auth__pop" role="dialog" onKeyDown={(e) => e.key === 'Escape' && setOpen(false)}>
          {auth.signedIn ? (
            <>
              <div className="gh-auth__who">
                {auth.user && <Avatar user={auth.user} size={28} />}
                <div>
                  <div className="gh-auth__login">{auth.user?.login}</div>
                  <div className="gh-auth__sub">{t(auth.method === 'device' ? 'gh.auth.viaDevice' : auth.method === 'gh-cli' ? 'gh.auth.viaGhCli' : 'gh.auth.viaPat')}</div>
                </div>
              </div>
              {auth.rateLimit !== undefined && (
                <div className="gh-auth__sub">
                  {t('gh.auth.rate', { left: auth.rateRemaining ?? 0, limit: auth.rateLimit })}
                  {resetMin !== null && ` · ${t('gh.auth.reset', { min: resetMin })}`}
                </div>
              )}
              {auth.storageUnavailable && <div className="gh-auth__warn">{t('gh.auth.noStorage')}</div>}
              <button className="btn btn--sm" onClick={() => void logout()}>
                {t('gh.auth.signOut')}
              </button>
            </>
          ) : device ? (
            <>
              <div className="gh-auth__title">{t('gh.auth.deviceTitle')}</div>
              <div className="gh-auth__code mono selectable">{device.userCode}</div>
              <div className="gh-auth__sub">{t('gh.auth.deviceHint', { url: device.verificationUri })}</div>
              <div className="gh-auth__row">
                <button className="btn btn--sm" onClick={() => void navigator.clipboard.writeText(device.userCode)}>
                  {t('detail.copy')}
                </button>
                <button className="btn btn--sm" onClick={cancelDevice}>
                  {t('add.cancel')}
                </button>
              </div>
            </>
          ) : (
            <>
              <div className="gh-auth__title">{t('gh.auth.signIn')}</div>
              <label className="gh-auth__label" htmlFor="gh-pat">
                {t('gh.auth.pat')}
              </label>
              <div className="gh-auth__row">
                <input
                  id="gh-pat"
                  className="gh-auth__input mono"
                  type="password"
                  autoComplete="off"
                  spellCheck={false}
                  value={pat}
                  placeholder="github_pat_…"
                  onChange={(e) => setPat(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && void submitPat()}
                />
                <button className="btn btn--sm btn--primary" disabled={busy || !pat.trim()} onClick={() => void submitPat()}>
                  {busy ? t('add.adding') : t('gh.auth.signInShort')}
                </button>
              </div>
              <div className="gh-auth__sub">{t('gh.auth.patHint')}</div>
              <div className="gh-auth__sep" />
              {auth.cliAvailable && (
                <>
                  <button className="btn btn--sm" disabled={cliBusy} onClick={() => void submitCli()}>
                    {cliBusy ? t('add.adding') : t('gh.auth.importCli')}
                  </button>
                  <div className="gh-auth__sub">{t('gh.auth.importCliHint')}</div>
                </>
              )}
              <button className="btn btn--sm" disabled={!!devicePending} aria-describedby={needCid ? 'gh-need-cid' : undefined} onClick={onDevice}>
                {t('gh.auth.device')}
              </button>
              {needCid && (
                <div id="gh-need-cid" className="gh-auth__warn" role="status">
                  {t('gh.auth.needClientId')}
                </div>
              )}
              <details className="gh-auth__adv" open={advOpen} onToggle={(e) => setAdvOpen(e.currentTarget.open)}>
                <summary>{t('gh.auth.advanced')}</summary>
                <label className="gh-auth__label" htmlFor="gh-cid">
                  OAuth App client ID
                </label>
                <div className="gh-auth__row">
                  <input
                    id="gh-cid"
                    ref={cidRef}
                    className="gh-auth__input mono"
                    value={cid}
                    spellCheck={false}
                    placeholder="Ov23li…"
                    onChange={(e) => setCid(e.target.value)}
                    onKeyDown={(e) => e.key === 'Enter' && saveCid()}
                  />
                  <button className="btn btn--sm" onClick={saveCid}>
                    {t('repoMenu.save')}
                  </button>
                </div>
                <div className="gh-auth__sub">{t('gh.auth.clientIdHint')}</div>
                <button className="btn btn--sm" onClick={() => void api.invoke('gh:openExternal', NEW_OAUTH_APP_URL)}>
                  {t('gh.auth.createApp')}
                </button>
              </details>
            </>
          )}
          {authError && <div className="gh-auth__err">{authError}</div>}
        </div>
      )}
    </div>
  )
}
