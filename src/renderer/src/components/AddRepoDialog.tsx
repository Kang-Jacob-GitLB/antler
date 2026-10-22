import { useEffect, useMemo, useRef, useState } from 'react'
import type { RepoScanResult, SourceKind, SshHostInfo } from '@shared/types'
import { useT } from '../i18n'
import { api, errorMessage } from '../lib/api'
import { ghKey, registeredGhKeys } from '../lib/ghRepoList'
import { useAppStore } from '../stores/appStore'
import { IconCheck, IconFolder, IconGithub, IconMonitor, IconServer, IconTerminal } from './icons'
import { GithubRepoPicker } from './GithubRepoPicker'
import { useGhAccessibleRepos } from './useGhAccessibleRepos'
import { RemoteFolderPicker } from './RemoteFolderPicker'
import { GroupPicker } from './GroupChip'
import './AddRepoDialog.css'

const isWindows = window.antler.platform === 'win32'

/**
 * 로컬 · WSL · SSH 저장소 추가. 그 환경 안에서 git 최상위 경로를 확인한 뒤 저장한다.
 * 고른 폴더가 저장소가 아니고 바로 아래에 저장소들이 있으면 목록으로 보여 주고, 고른 것들을 한 번에 추가한다.
 * GitHub 탭은 로그인한 계정이 접근할 수 있는 저장소를 검색해 여러 개 골라, 클론 없이 이력만 받는 원격 전용 저장소로 추가한다.
 */
export function AddRepoDialog(): React.JSX.Element | null {
  const t = useT()
  const open = useAppStore((s) => s.addOpen)
  const config = useAppStore((s) => s.config)
  const { openAddDialog, addRepo, addRepos, addGithubRepos } = useAppStore.getState()

  const [kind, setKind] = useState<SourceKind>('local')
  const [path, setPath] = useState('')
  const [distro, setDistro] = useState('')
  const [distros, setDistros] = useState<string[] | null>(null)
  const [host, setHost] = useState('')
  const [hosts, setHosts] = useState<SshHostInfo[]>([])
  const [group, setGroup] = useState('')
  const [busy, setBusy] = useState<'add' | 'test' | 'scan' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [tested, setTested] = useState<string | null>(null)
  // SSH 원격 폴더 탐색 창
  const [remoteBrowse, setRemoteBrowse] = useState(false)
  // 고른 폴더 안의 저장소 찾기 결과(key = 환경+경로 — 하나라도 바뀌면 낡은 결과다)와 추가할 것으로 고른 경로
  const [scan, setScan] = useState<{ key: string; result: RepoScanResult } | null>(null)
  const [checked, setChecked] = useState<Set<string>>(() => new Set())
  // GitHub 탭: 추가할 것으로 고른 저장소(ghKey)와 접근 가능한 저장소 목록
  const [ghChecked, setGhChecked] = useState<Set<string>>(() => new Set())
  const gh = useGhAccessibleRepos(open, kind === 'github')
  const scanSeq = useRef(0)
  // 지금 화면의 환경+경로(찾기를 기다리는 동안 입력이 바뀌었는지 보려고)
  const latestKey = useRef('')

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
    setScan(null)
    setGhChecked(new Set())
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
  const keyFor = (p: string): string => [kind, distro, host.trim(), p.trim()].join('\n')
  latestKey.current = keyFor(path)
  const found = scan && scan.key === keyFor(path) && !scan.result.isRepo && scan.result.repos.length > 0 ? scan.result : null
  const picked = found ? found.repos.filter((r) => checked.has(r.path)) : []
  // GitHub 탭: 이미 추가된 것은 고를 수 없고, 목록에 없어진 것(새로 받은 목록)은 제외한다
  const ghRegistered = registeredGhKeys(config?.repos ?? [])
  const ghPicked = (gh.result?.repos ?? []).filter((r) => {
    const k = ghKey(r.owner, r.repo)
    return ghChecked.has(k) && !ghRegistered.has(k)
  })
  const canSubmit =
    kind === 'github'
      ? ghPicked.length > 0 && !busy
      : !!path.trim() && (kind !== 'wsl' || !!distro) && (kind !== 'ssh' || !!host.trim()) && !busy && (!found || picked.length > 0)

  // 이미 목록에 있는 저장소인가(그 환경의 소스·경로로 비교). 로컬 탭에서 고른 WSL 공유 경로는 WSL 저장소로 본다
  const isRegistered = (p: string): boolean => {
    const unc = /^[\\/]{2}(?:wsl\.localhost|wsl\$)[\\/]+([^\\/]+)(.*)$/i.exec(p)
    const sourceId = unc ? `wsl:${unc[1]}` : kind === 'local' ? 'local' : kind === 'wsl' ? `wsl:${distro}` : `ssh:${host.trim()}`
    const rp = unc ? '/' + (unc[2] ?? '').split(/[\\/]+/).filter(Boolean).join('/') : p
    // Windows 로컬 경로는 대소문자를 가리지 않는다
    const same = !unc && kind === 'local' && isWindows ? (a: string) => a.toLowerCase() === rp.toLowerCase() : (a: string) => a === rp
    return (config?.repos ?? []).some((r) => r.sourceId === sourceId && same(r.path))
  }

  /** 고른 폴더가 저장소인지, 아니면 바로 아래에 어떤 저장소가 있는지 찾는다. 그사이 창·찾기가 바뀌었으면 null */
  const runScan = async (p: string): Promise<{ result: RepoScanResult } | { error: unknown } | null> => {
    const s = session.current
    const seq = ++scanSeq.current
    setBusy('scan')
    setError(null)
    try {
      const result = await api.invoke('repos:scan', { kind, ...target, path: p.trim() })
      if (s !== session.current || seq !== scanSeq.current) return null
      setScan({ key: keyFor(p), result })
      setChecked(new Set(result.repos.filter((r) => !isRegistered(r.path)).map((r) => r.path)))
      return { result }
    } catch (error) {
      return s === session.current && seq === scanSeq.current ? { error } : null
    } finally {
      if (s === session.current && seq === scanSeq.current) setBusy(null)
    }
  }

  const browse = async (): Promise<void> => {
    if (kind === 'ssh') {
      if (host.trim()) setRemoteBrowse(true)
      return
    }
    // WSL 은 \\wsl.localhost\<배포판>\ 에서 시작한다 — 고른 경로는 main 이 WSL 경로로 바꿔 등록한다
    const start = kind === 'wsl' && distro ? `\\\\wsl.localhost\\${distro}\\home` : undefined
    const chosen = await api.invoke('dialog:pickFolder', start)
    if (chosen) {
      setPath(chosen)
      // 찾기 실패는 여기서 알리지 않는다 — 추가를 누르면 다시 확인해 이유를 보여 준다
      void runScan(chosen)
    }
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

  const submitMany = async (): Promise<void> => {
    const s = session.current
    setBusy('add')
    setError(null)
    try {
      const res = await addRepos({ kind, ...target, paths: picked.map((r) => r.path), ...(group.trim() ? { group: group.trim() } : {}) })
      if (s !== session.current) return
      if (res.failed.length === 0) return close()
      // 실패한 것만 남겨 다시 시도할 수 있게 한다(추가된 것은 '이미 추가됨' 으로 바뀐다)
      setChecked(new Set(res.failed.map((f) => f.path)))
      const nameOf = (p: string): string => found?.repos.find((r) => r.path === p)?.name ?? p
      setError([t('add.someFailed', { n: res.failed.length }), ...res.failed.map((f) => `${nameOf(f.path)}: ${f.error}`)].join('\n'))
      setBusy(null)
    } catch (err) {
      if (s !== session.current) return
      setError(errorMessage(err))
      setBusy(null)
    }
  }

  /** 고른 GitHub 저장소를 원격 전용 저장소로 추가한다(앱 캐시에 등록만 하고, 이력은 처음 열 때 받는다) */
  const submitGithub = async (): Promise<void> => {
    const s = session.current
    setBusy('add')
    setError(null)
    try {
      const res = await addGithubRepos({
        repos: ghPicked.map((r) => ({ owner: r.owner, repo: r.repo, defaultBranch: r.defaultBranch })),
        ...(group.trim() ? { group: group.trim() } : {})
      })
      if (s !== session.current) return
      if (res.failed.length === 0) return close()
      // 실패한 것만 남겨 다시 시도할 수 있게 한다(추가된 것은 '이미 추가됨' 으로 바뀐다)
      setGhChecked(new Set(res.failed.map((f) => f.fullName.toLowerCase())))
      setError([t('add.someFailed', { n: res.failed.length }), ...res.failed.map((f) => `${f.fullName}: ${f.error}`)].join('\n'))
      setBusy(null)
    } catch (err) {
      if (s !== session.current) return
      setError(errorMessage(err))
      setBusy(null)
    }
  }

  const submit = async (): Promise<void> => {
    if (!canSubmit) return
    if (kind === 'github') return submitGithub()
    if (found) return submitMany()
    const s = session.current
    const key = keyFor(path)
    // 아직 찾아보지 않은 경로(SSH 직접 입력, 찾기 실패 등)면 먼저 찾는다 — 저장소가 여럿 나오면 목록을 보여 주고 고르게 한다
    if (!(scan && scan.key === key)) {
      const r = await runScan(path)
      // 그사이 창을 다시 열었거나 환경·경로를 바꿨으면 이전 입력으로 추가하지 않는다
      if (!r || s !== session.current || latestKey.current !== key) return
      // 찾기가 실패하면(연결 취소·접속 실패·없는 폴더) 추가도 같은 이유로 실패한다 — 이유를 보여 주고 멈춘다
      if ('error' in r) return setError(errorMessage(r.error))
      if (!r.result.isRepo && r.result.repos.length > 0) return
    }
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
    { k: 'ssh', icon: <IconServer size={14} />, label: 'SSH' },
    { k: 'github', icon: <IconGithub size={14} />, label: t('add.gh.tab') }
  ]

  return (
    // 바깥을 누르면 닫는다. 원격 폴더 찾아보기가 떠 있으면 그것만 닫는다(입력한 주소·그룹은 남긴다) — Esc 와 같은 단계
    <div className="modal-backdrop" onMouseDown={() => (remoteBrowse ? setRemoteBrowse(false) : close())}>
      <form
        className={`add-dialog${kind === 'github' ? ' add-dialog--gh' : ''}`}
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
              void runScan(p)
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

        {kind === 'github' && (
          <>
            <span className="field__hint">{t('add.gh.hint')}</span>
            <GithubRepoPicker state={gh} registered={ghRegistered} checked={ghChecked} onChecked={setGhChecked} busy={busy === 'add'} />
          </>
        )}

        {kind !== 'github' && (
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
        )}

        {kind !== 'github' && found && (
          <div className="field">
            <div className="add-found__head">
              <span className="field__label">{t('add.found', { n: found.repos.length })}</span>
              <span className="add-dialog__spacer" />
              {(() => {
                const selectable = found.repos.filter((r) => !isRegistered(r.path))
                const all = selectable.length > 0 && selectable.every((r) => checked.has(r.path))
                return (
                  <button
                    type="button"
                    className="btn btn--sm"
                    disabled={selectable.length === 0 || !!busy}
                    onClick={() => setChecked(new Set(all ? [] : selectable.map((r) => r.path)))}
                  >
                    {all ? t('add.selectNone') : t('add.selectAll')}
                  </button>
                )
              })()}
            </div>
            <ul className="add-found" aria-label={t('add.found', { n: found.repos.length })}>
              {found.repos.map((r) => {
                const done = isRegistered(r.path)
                return (
                  <li key={r.path}>
                    <label className={`add-found__item${done ? ' is-done' : ''}`} title={r.path}>
                      <input
                        type="checkbox"
                        checked={!done && checked.has(r.path)}
                        disabled={done || !!busy}
                        onChange={(e) =>
                          setChecked((prev) => {
                            const next = new Set(prev)
                            if (e.target.checked) next.add(r.path)
                            else next.delete(r.path)
                            return next
                          })
                        }
                      />
                      <span className="add-found__name">{r.name}</span>
                      {done && <span className="add-found__tag">{t('add.already')}</span>}
                    </label>
                  </li>
                )
              })}
            </ul>
            <span className="field__hint">{found.truncated ? t('add.foundTruncated') : t('add.foundHint')}</span>
          </div>
        )}

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
          {(kind === 'wsl' || kind === 'ssh') && (
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
            {busy === 'add'
              ? t('add.adding')
              : busy === 'scan'
                ? t('add.scanning')
                : kind === 'github'
                  ? ghPicked.length > 0
                    ? t('add.submitMany', { n: ghPicked.length })
                    : t('add.submit')
                  : found
                    ? t('add.submitMany', { n: picked.length })
                    : t('add.submit')}
          </button>
        </div>
      </form>
    </div>
  )
}
