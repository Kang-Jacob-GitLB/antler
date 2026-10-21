import { BrowserWindow, dialog, ipcMain, type IpcMainInvokeEvent } from 'electron'
import type { IpcArgs, IpcChannel, IpcEnvelope, IpcResult } from '@shared/ipc'
import type { AddRepoInput, AddReposInput } from '@shared/types'
import { isWslUnc, wslFromUnc } from '../executors/wslPaths'
import { AbortError, GitError } from '../executors/GitExecutor'
import { listSshHosts } from '../executors/SshExecutor'
import { listWslDistros } from '../executors/WslExecutor'
import type { ConfigStore } from '../services/ConfigStore'
import type { FetchService } from '../services/FetchService'
import { display, formatItem, MAX_CONFIRM_ITEMS } from './confirmText'
import { StaleLogError, type RepoService } from '../services/RepoService'
import type { RepoWatcher } from '../services/RepoWatcher'
import { GitHubApiError } from '../github/client'
import type { GitHubService } from '../github/service'
import { registerExternalIpc } from './external'
import { registerGithubIpc } from './github'
import { JiraApiError } from '../jira/client'
import type { JiraService } from '../jira/service'
import { registerJiraIpc } from './jira'
import { applyTitleBarTheme } from '../titleBar'
import type { UpdateService } from '../update/UpdateService'

type Handler<C extends IpcChannel> = (e: IpcMainInvokeEvent, ...args: IpcArgs<C>) => Promise<IpcResult<C>> | IpcResult<C>

/** 결과를 IpcEnvelope 로 감싸 등록한다. 예외는 메시지와 코드만 renderer 로 넘긴다. */
function handle<C extends IpcChannel>(channel: C, fn: Handler<C>): void {
  ipcMain.handle(channel, async (e, ...args): Promise<IpcEnvelope<IpcResult<C>>> => {
    try {
      return { ok: true, data: await fn(e, ...(args as IpcArgs<C>)) }
    } catch (err) {
      const code =
        err instanceof AbortError
          ? 'ABORTED'
          : err instanceof StaleLogError
            ? 'STALE'
            : err instanceof GitHubApiError || err instanceof JiraApiError
              ? err.code
              : err instanceof GitError
                ? 'GIT'
                : 'ERROR'
      return { ok: false, error: err instanceof Error ? err.message : String(err), code }
    }
  })
}

const MAX_PATH_LEN = 4096
const MAX_HOST_LEN = 255
/** 한 번에 추가할 수 있는 저장소 수 */
const MAX_ADD_MANY = 500

const isUncPath = (p: string): boolean => /^[\\/]{2}/.test(p)

/**
 * renderer 가 보낸 환경·경로 입력 검증. renderer 는 샌드박스지만 IPC 인자는 신뢰하지 않는다(XSS 대비).
 * - 로컬 경로가 네트워크 경로(\\host\share)면 거부: git 이 SMB 로 접근하며 NTLM 해시가 새어 나갈 수 있다(WSL 공유는 허용)
 * - WSL 배포판은 실제 설치된 것만
 * - ~/.ssh/config 에 없는 SSH 대상은 main 이 띄우는 확인 창에서 사용자가 승인해야 접속한다
 */
function checkPath(kind: AddRepoInput['kind'], path: unknown): asserts path is string {
  if (typeof path !== 'string' || !path.trim() || path.length > MAX_PATH_LEN || path.includes('\0')) {
    throw new Error('잘못된 경로입니다')
  }
  // 서비스는 앞뒤 공백을 지운 경로를 쓴다 — 같은 값으로 검사해야 ' \\host\share' 가 빠져나가지 못한다
  const p = path.trim()
  if ((kind === 'local' || kind === 'wsl') && isUncPath(p) && !isWslUnc(p)) {
    throw new Error('네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)')
  }
}

async function checkEnvInput(e: IpcMainInvokeEvent, input: unknown, withPath: boolean): Promise<void> {
  const i = input as Partial<AddRepoInput> | null
  if (!i || typeof i !== 'object' || !['local', 'wsl', 'ssh'].includes(i.kind as string)) throw new Error('잘못된 입력입니다')
  if (withPath) {
    checkPath(i.kind as AddRepoInput['kind'], i.path)
    if (i.group !== undefined && (typeof i.group !== 'string' || i.group.length > 200)) throw new Error('잘못된 그룹 이름입니다')
  }
  if (i.kind === 'wsl') {
    const distros = await listWslDistros()
    // UNC 로 고른 경우에는 경로 안의 배포판 이름을 본다
    const unc = typeof i.path === 'string' ? wslFromUnc(i.path) : null
    const distro = unc?.distro ?? i.distro
    if (!distro || !distros.includes(distro)) throw new Error(`설치된 WSL 배포판이 아닙니다: ${distro ?? '(없음)'}`)
  }
  if (i.kind === 'ssh') {
    const host = typeof i.sshHost === 'string' ? i.sshHost.trim() : ''
    if (!host || host.length > MAX_HOST_LEN || /[\s\0]/.test(host) || host.startsWith('-')) throw new Error('잘못된 SSH 호스트입니다')
    const known = approvedSshHosts.has(host) || (await listSshHosts()).some((h) => h.alias === host)
    if (!known) {
      const win = BrowserWindow.fromWebContents(e.sender)
      const opts: Electron.MessageBoxOptions = {
        type: 'question',
        buttons: ['연결', '취소'],
        defaultId: 1,
        cancelId: 1,
        title: 'SSH 연결 확인',
        message: `~/.ssh/config 에 없는 호스트입니다.\n${host} 에 연결할까요?`,
        detail: '키 또는 ssh-agent 로 인증하며, 호스트 키가 known_hosts 에 등록돼 있어야 합니다.'
      }
      const res = win ? await dialog.showMessageBox(win, opts) : await dialog.showMessageBox(opts)
      if (res.response !== 0) throw new Error('연결을 취소했습니다')
      approvedSshHosts.add(host)
    }
  }
}

/**
 * 이번 실행 동안 사용자가 연결을 승인한 설정 밖 SSH 호스트. 저장소 추가 한 번에 폴더 탐색·찾기·추가가 이어지므로
 * 같은 호스트를 매번 다시 묻지 않는다(앱을 다시 켜면 다시 묻는다)
 */
const approvedSshHosts = new Set<string>()

/**
 * 저장소 설정에 외부 명령을 실행할 수 있는 항목이 있을 때 수동 fetch 전에 묻는다. 허용은 같은 설정(키·값 지문)에만
 * 유효하며, true 는 이번 실행 동안, 'always' 는 앱을 다시 켜도 유지된다. 기본 선택은 취소다.
 */
async function confirmDangerousConfig(
  e: IpcMainInvokeEvent,
  name: string,
  items: { key: string; value: string }[]
): Promise<boolean | 'always'> {
  const win = BrowserWindow.fromWebContents(e.sender)
  const show = (opts: Electron.MessageBoxOptions): Promise<Electron.MessageBoxReturnValue> =>
    win ? dialog.showMessageBox(win, opts) : dialog.showMessageBox(opts)
  // 저장소가 정한 문자열(이름·키·값)은 따옴표·이스케이프로 표시한다 — 문구를 꾸며 사용자를 속이지 못하게
  const title = `${display(name)} 저장소의 설정에 fetch 때 외부 프로그램을 실행하거나 연결을 가로챌 수 있는 항목이 있습니다.`
  if (items.length > MAX_CONFIRM_ITEMS) {
    // 전부 보여 줄 수 없으면 허용 여부를 물을 수 없다
    await show({
      type: 'warning',
      buttons: ['닫기'],
      defaultId: 0,
      cancelId: 0,
      title: 'fetch 차단',
      message: title,
      detail: `항목이 ${items.length}개로 너무 많아 모두 확인할 수 없어 fetch 하지 않습니다. 저장소의 .git/config 를 직접 확인하세요.\n\n${items
        .slice(0, MAX_CONFIRM_ITEMS)
        .map((i) => formatItem(i.key, i.value))
        .join('\n')}\n… 외 ${items.length - MAX_CONFIRM_ITEMS}개`
    })
    return false
  }
  const res = await show({
    type: 'warning',
    buttons: ['이번만 fetch', '이 저장소에서 항상 허용', '취소'],
    defaultId: 2,
    cancelId: 2,
    title: 'fetch 확인',
    message: title,
    detail: `${items.map((i) => formatItem(i.key, i.value)).join('\n')}\n\n직접 만든 설정이거나 신뢰하는 저장소일 때만 허용하세요. "이번만"은 이번 실행 동안, "항상"은 앱을 다시 켜도 적용되며, 어느 쪽이든 위 키·값이 바뀌면 다시 묻습니다. 값이 가리키는 스크립트·파일의 내용이 바뀌는 것은 확인하지 않습니다.`
  })
  return res.response === 0 ? true : res.response === 1 ? 'always' : false
}

export function registerIpc(
  config: ConfigStore,
  repos: RepoService,
  watcher: RepoWatcher,
  gh: GitHubService,
  jira: JiraService,
  fetcher: FetchService,
  updates: UpdateService
): void {
  handle('update:get', () => updates.get())
  handle('update:check', () => updates.check(true))
  handle('update:download', () => updates.download())
  handle('update:cancel', () => updates.cancel())
  handle('update:install', () => updates.install())
  handle('update:dismiss', (_e, action, version) => updates.dismiss(action, version))
  handle('update:releaseNotes', (_e, version) => updates.releaseNotes(version))
  handle('config:get', () => config.get())
  handle('config:setUi', (_e, patch) => {
    const next = config.setUi(patch)
    if (patch.autoFetchMinutes !== undefined) fetcher.onPrefsChanged()
    return next
  })
  registerExternalIpc(handle, config)

  handle('dialog:pickFolder', async (e, defaultPath) => {
    const win = BrowserWindow.fromWebContents(e.sender)
    // 시작 위치로 네트워크 경로(\\host\share)를 주면 대화상자가 SMB 인증을 시도한다 — WSL 공유만 허용
    const safeDefault = typeof defaultPath === 'string' && defaultPath && (!isUncPath(defaultPath) || isWslUnc(defaultPath))
    const opts: Electron.OpenDialogOptions = {
      properties: ['openDirectory'],
      title: 'Git 저장소 선택',
      ...(safeDefault ? { defaultPath } : {})
    }
    const res = win ? await dialog.showOpenDialog(win, opts) : await dialog.showOpenDialog(opts)
    const path = res.filePaths[0]
    return res.canceled || !path ? null : path
  })
  handle('repos:add', async (e, input) => {
    await checkEnvInput(e, input, true)
    return repos.addRepo(input)
  })
  handle('repos:addMany', async (e, input) => {
    const i = input as Partial<AddReposInput> | null
    if (!i || !Array.isArray(i.paths) || i.paths.length === 0 || i.paths.length > MAX_ADD_MANY) throw new Error('잘못된 입력입니다')
    const distro = wslFromUnc(String(i.paths[0]))?.distro
    for (const p of i.paths) {
      checkPath(i.kind as AddRepoInput['kind'], p)
      if (wslFromUnc(p)?.distro !== distro) throw new Error('한 번에 추가하는 저장소는 같은 환경이어야 합니다')
    }
    // 환경 검증(설정에 없는 SSH 호스트 확인 창 포함)은 한 번만 한다. 모든 경로가 한 폴더 아래라 첫 경로로 WSL 배포판을 본다
    await checkEnvInput(e, { kind: i.kind, distro: i.distro, sshHost: i.sshHost, group: i.group, path: i.paths[0] }, true)
    return repos.addRepos(i as AddReposInput)
  })
  handle('repos:scan', async (e, input) => {
    await checkEnvInput(e, input, true)
    return repos.scanRepos(input)
  })
  handle('repos:update', (_e, repoId, patch) => {
    repos.invalidateGhRemotes(repoId)
    return config.updateRepo(repoId, patch)
  })
  handle('repos:remove', async (_e, repoId) => {
    repos.invalidateGhRemotes(repoId)
    const next = config.removeRepo(repoId)
    fetcher.forget(repoId)
    if (watcher.current === repoId) void watcher.watch(null)
    await repos.releaseUnused()
    return next
  })
  handle('repos:watch', (_e, repoId) => {
    fetcher.setActive(repoId && config.repo(repoId) ? repoId : null)
    // 첫 로그 페이지가 나갈 때까지 백그라운드 조회(활동·WIP·감시 기준 등)를 미룬다
    if (repoId && config.repo(repoId)) repos.armFirstPage(repoId)
    return watcher.watch(repoId)
  })

  handle('env:wslDistros', () => listWslDistros())
  handle('env:sshHosts', () => listSshHosts())
  handle('env:test', async (e, input) => {
    await checkEnvInput(e, input, false)
    return repos.testEnv(input)
  })
  handle('env:listDir', async (e, input) => {
    const i = input as { sshHost?: unknown; path?: unknown } | null
    const path = i && typeof i.path === 'string' ? i.path : ''
    if (path.length > MAX_PATH_LEN || path.includes('\0')) throw new Error('잘못된 경로입니다')
    // 호스트 검증은 연결 테스트와 같다(설정에 없는 호스트면 사용자 확인)
    await checkEnvInput(e, { kind: 'ssh', sshHost: i?.sshHost }, false)
    return repos.listRemoteDir(String(i?.sshHost).trim(), path)
  })

  handle('git:refs', (_e, repoId) => repos.refs(repoId))
  handle('git:refsDetails', (_e, repoId) => repos.refsDetails(repoId))
  handle('git:log', (_e, repoId, query, requestId) => repos.log(repoId, query, requestId))
  handle('git:activity', (_e, repoId, scope, requestId) => repos.activity(repoId, scope, requestId))
  handle('git:search',(_e, repoId, query, scope, requestId) => repos.search(repoId, query, scope, requestId))
  handle('git:commit', (_e, repoId, hash, requestId) => repos.commit(repoId, hash, requestId))
  handle('git:compare', (_e, repoId, from, to, requestId) => repos.compare(repoId, from, to, requestId))
  handle('git:fileDiff', (_e, repoId, req, requestId) => repos.fileDiff(repoId, req, requestId))
  handle('git:fileHistory', (_e, repoId, req, requestId) => repos.fileHistory(repoId, req, requestId))
  handle('git:blame', (_e, repoId, req, requestId) => repos.blame(repoId, req, requestId))
  handle('git:repoExtras', (_e, repoId) => repos.repoExtras(repoId))
  handle('git:lfsPaths', (_e, repoId, req, requestId) => repos.lfsPaths(repoId, req, requestId))
  handle('git:wipStatus', (_e, repoId, requestId) => repos.wipStatus(repoId, requestId))
  handle('git:wipDiff', (_e, repoId, req, requestId) => repos.wipDiff(repoId, req, requestId))
  // manual 은 renderer 가 주장하는 값이다. 침해된 renderer 는 manual=true 로 수동 fetch 를 요청할 수 있지만(자동 fetch 의 자격 증명 도우미 제한·file 전송 제한을 피함),
  // 허용 목록 밖의 저장소 설정·userinfo URL 은 어느 쪽이든 main 의 확인 창에서 사용자가 직접 허용해야 하고 remote URL 거부는 확인 없이 적용된다.
  handle('git:fetch', (e, repoId, manual) =>
    fetcher.fetch(repoId, {
      manual: manual === true,
      confirm: (id, items) => confirmDangerousConfig(e, config.repo(id)?.name ?? id, items)
    })
  )
  registerGithubIpc(handle, gh, repos)
  registerJiraIpc(handle, jira)
  handle('req:cancel', (_e, requestId) => {
    repos.cancel(requestId)
    gh.cancel(requestId)
    jira.cancel(requestId)
  })
  handle('win:setTitleBarTheme', (e, theme) => {
    if (!theme || typeof theme !== 'object' || typeof theme.dark !== 'boolean') throw new Error('잘못된 입력입니다')
    if (theme.dim !== undefined && typeof theme.dim !== 'boolean') throw new Error('잘못된 입력입니다')
    const win = BrowserWindow.fromWebContents(e.sender)
    if (win) applyTitleBarTheme(win, theme.dark, theme.dim === true)
  })
}
