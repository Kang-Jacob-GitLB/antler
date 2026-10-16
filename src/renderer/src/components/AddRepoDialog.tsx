import { useEffect, useMemo, useRef, useState } from 'react'
import type { SourceKind, SshHostInfo } from '@shared/types'
import { useT } from '../i18n'
import { api, errorMessage } from '../lib/api'
import { useAppStore } from '../stores/appStore'
import { IconCheck, IconFolder, IconMonitor, IconServer, IconTerminal } from './icons'
import { RemoteFolderPicker } from './RemoteFolderPicker'
import { GroupPicker } from './GroupChip'
import './AddRepoDialog.css'

const isWindows = window.antler.platform === 'win32'

/** 로컬 · WSL · SSH 저장소 추가. 그 환경 안에서 git 최상위 경로를 확인한 뒤 저장한다 */
export function AddRepoDialog(): React.JSX.Element | null {
  const t = useT()
  const open = useAppStore((s) => s.addOpen)
  const config = useAppStore((s) => s.config)
  const { openAddDialog, addRepo } = useAppStore.getState()

  const [kind, setKind] = useState<SourceKind>('local')
  const [path, setPath] = useState('')
  const [distro, setDistro] = useState('')
  const [distros, setDistros] = useState<string[] | null>(null)
  const [host, setHost] = useState('')
  const [hosts, setHosts] = useState<SshHostInfo[]>([])
  const [group, setGroup] = useState('')
  const [busy, setBusy] = useState<'add' | 'test' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [tested, setTested] = useState<string | null>(null)
  // SSH 원격 폴더 탐색 창
  const [remoteBrowse, setRemoteBrowse] = useState(false)

  const groups = useMemo(
    () => [...new Set((config?.repos ?? []).map((r) => r.group).filter((g): g is string => !!g))].sort(),
    [config]
  )

  // 열 때마다 세션을 바꾼다. 창을 닫았다 다시 열면 이전 세션의 테스트·추가 결과는 새 창에 반영하지 않는다
  const session = useRef(0)

  // 열 때마다 초기화하고 WSL 배포판·SSH 호스트 목록을 불러온다
  useEffect(() => {
    if (!open) return
    session.current++
    setPath('')
    setGroup('')
    setError(null)
    setTested(null)
    setBusy(null)
    setRemoteBrowse(false)
    if (isWindows) {
      void api.invoke('env:wslDistros').then((list) => {
        setDistros(list)
        setDistro((d) => d || list[0] || '')
      })
    }
    void api.invoke('env:sshHosts').then(setHosts, () => setHosts([]))
  }, [open])

  // 환경을 바꾸면 이전 테스트 결과는 의미가 없다
  useEffect(() => {
    setTested(null)
    setError(null)
  }, [kind, distro, host])

  if (!open) return null

  const close = (): void => openAddDialog(false)
  const target = kind === 'wsl' ? { distro } : kind === 'ssh' ? { sshHost: host.trim() } : {}
  const canSubmit = !!path.trim() && (kind !== 'wsl' || !!distro) && (kind !== 'ssh' || !!host.trim()) && !busy

  const browse = async (): Promise<void> => {
    if (kind === 'ssh') {
      if (host.trim()) setRemoteBrowse(true)
      return
    }
    // WSL 은 \\wsl.localhost\<배포판>\ 에서 시작한다 — 고른 경로는 main 이 WSL 경로로 바꿔 등록한다
    const start = kind === 'wsl' && distro ? `\\\\wsl.localhost\\${distro}\\home` : undefined
    const picked = await api.invoke('dialog:pickFolder', start)
    if (picked) setPath(picked)
  }

  const test = async (): Promise<void> => {
    const s = session.current
    setBusy('test')
    setError(null)
    try {
      const version = await api.invoke('env:test', { kind, ...target })
      if (s !== session.current) return
      setTested(version)
    } catch (err) {
      if (s !== session.current) return
      setTested(null)
      setError(errorMessage(err))
    } finally {
      if (s === session.current) setBusy(null)
    }
  }

  const submit = async (): Promise<void> => {
    if (!canSubmit) return
    const s = session.current
    setBusy('add')
    setError(null)
    try {
      await addRepo({ kind, ...target, path: path.trim(), ...(group.trim() ? { group: group.trim() } : {}) })
      // 추가는 끝까지 반영하되(저장소 선택), 그사이 창을 다시 열었다면 새 창은 닫지 않는다
      if (s === session.current) close()
    } catch (err) {
      if (s !== session.current) return
      setError(errorMessage(err))
      setBusy(null)
    }
  }

  const tabs: { k: SourceKind; icon: React.JSX.Element; label: string }[] = [
    { k: 'local', icon: <IconMonitor size={14} />, label: t('add.local') },
    ...(isWindows ? [{ k: 'wsl' as const, icon: <IconTerminal size={14} />, label: 'WSL' }] : []),
    { k: 'ssh', icon: <IconServer size={14} />, label: 'SSH' }
  ]

  return (
    <div className="modal-backdrop" onMouseDown={close}>
      <form
        className="add-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="add-title"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === 'Escape' && close()}
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        {remoteBrowse && (
          <RemoteFolderPicker
            host={host.trim()}
            initialPath={path.trim()}
            onClose={() => setRemoteBrowse(false)}
            onPick={(p) => {
              setPath(p)
              setRemoteBrowse(false)
            }}
          />
        )}
        <h2 id="add-title" className="add-dialog__title">
          {t('add.title')}
        </h2>

        <div className="seg add-dialog__tabs" role="tablist">
          {tabs.map(({ k, icon, label }) => (
            <button
              key={k}
              type="button"
              role="tab"
              aria-selected={kind === k}
              className={`seg__btn${kind === k ? ' is-on' : ''}`}
              onClick={() => setKind(k)}
            >
              {icon}
              {label}
            </button>
          ))}
        </div>

        {kind === 'wsl' && (
          <label className="field">
            <span className="field__label">{t('add.distro')}</span>
            {distros && distros.length === 0 ? (
              <span className="field__hint is-warn">{t('add.noDistros')}</span>
            ) : (
              <select className="field__input" value={distro} onChange={(e) => setDistro(e.target.value)}>
                {(distros ?? []).map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
              </select>
            )}
          </label>
        )}

        {kind === 'ssh' && (
          <label className="field">
            <span className="field__label">{t('add.host')}</span>
            <input
              className="field__input mono"
              list="ssh-hosts"
              value={host}
              placeholder={t('add.hostPlaceholder')}
              onChange={(e) => setHost(e.target.value)}
              autoFocus
            />
            <datalist id="ssh-hosts">
              {hosts.map((h) => (
                <option key={h.alias} value={h.alias}>
                  {`${h.user ? `${h.user}@` : ''}${h.hostName}${h.port !== 22 ? `:${h.port}` : ''}${h.proxyJump ? ` via ${h.proxyJump}` : ''}`}
                </option>
              ))}
            </datalist>
            <span className="field__hint">{t('add.sshHint')}</span>
          </label>
        )}

        <label className="field">
          <span className="field__label">{t('add.path')}</span>
          <div className="field__row">
            {kind === 'ssh' ? (
              // SSH 는 SFTP 를 막은 서버도 있어 직접 입력을 남겨 둔다(찾아보기는 원격 폴더 탐색)
              <input
                className="field__input mono"
                value={path}
                placeholder="~/work/project"
                onChange={(e) => setPath(e.target.value)}
              />
            ) : (
              // 로컬·WSL 은 OS 폴더 선택 창으로 고른다 — 칸을 눌러도 열린다
              <button
                type="button"
                className={`field__input field__picker mono${path ? '' : ' is-empty'}`}
                title={path || t('add.pathPick')}
                onClick={() => void browse()}
                autoFocus
              >
                {path || t('add.pathPick')}
              </button>
            )}
            <button type="button" className="btn" disabled={kind === 'ssh' && !host.trim()} onClick={() => void browse()}>
              <IconFolder size={14} />
              {t('add.browse')}
            </button>
          </div>
          {kind !== 'local' && <span className="field__hint">{t('add.remotePathHint')}</span>}
        </label>

        <div className="field">
          <label className="field__label" htmlFor="add-repo-group">
            {t('add.group')}
          </label>
          <GroupPicker
            inputId="add-repo-group"
            inputClassName="field__input"
            groups={groups}
            value={group}
            placeholder={t('repoMenu.groupPlaceholder')}
            onChange={setGroup}
          />
        </div>

        {error && (
          <div className="add-dialog__msg is-error" role="alert">
            {error}
          </div>
        )}
        {tested && !error && (
          <div className="add-dialog__msg is-ok">
            <IconCheck size={13} /> {tested}
          </div>
        )}

        <div className="add-dialog__foot">
          {kind !== 'local' && (
            <button
              type="button"
              className="btn"
              disabled={!!busy || (kind === 'ssh' ? !host.trim() : !distro)}
              onClick={() => void test()}
            >
              {busy === 'test' ? t('add.testing') : t('add.test')}
            </button>
          )}
          <span className="add-dialog__spacer" />
          <button type="button" className="btn" onClick={close}>
            {t('add.cancel')}
          </button>
          <button type="submit" className="btn btn--primary" disabled={!canSubmit}>
            {busy === 'add' ? t('add.adding') : t('add.submit')}
          </button>
        </div>
      </form>
    </div>
  )
}
