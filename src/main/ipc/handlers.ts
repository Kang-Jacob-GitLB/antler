import { BrowserWindow, dialog, ipcMain, type IpcMainInvokeEvent } from 'electron'
import type { IpcArgs, IpcChannel, IpcEnvelope, IpcResult } from '@shared/ipc'
import type { AddRepoInput } from '@shared/types'
import { isWslUnc, wslFromUnc } from '../executors/wslPaths'
import { AbortError, GitError } from '../executors/GitExecutor'
import { listSshHosts } from '../executors/SshExecutor'
import { listWslDistros } from '../executors/WslExecutor'
import type { ConfigStore } from '../services/ConfigStore'
import { StaleLogError, type RepoService } from '../services/RepoService'
import type { RepoWatcher } from '../services/RepoWatcher'
import { GitHubApiError } from '../github/client'
import type { GitHubService } from '../github/service'
import { registerGithubIpc } from './github'

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
            : err instanceof GitHubApiError
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

const isUncPath = (p: string): boolean => /^[\\/]{2}/.test(p)

/**
 * renderer 가 보낸 환경·경로 입력 검증. renderer 는 샌드박스지만 IPC 인자는 신뢰하지 않는다(XSS 대비).
 * - 로컬 경로가 네트워크 경로(\\host\share)면 거부: git 이 SMB 로 접근하며 NTLM 해시가 새어 나갈 수 있다(WSL 공유는 허용)
 * - WSL 배포판은 실제 설치된 것만
 * - ~/.ssh/config 에 없는 SSH 대상은 main 이 띄우는 확인 창에서 사용자가 승인해야 접속한다
 */
async function checkEnvInput(e: IpcMainInvokeEvent, input: unknown, withPath: boolean): Promise<void> {
  const i = input as Partial<AddRepoInput> | null
  if (!i || typeof i !== 'object' || !['local', 'wsl', 'ssh'].includes(i.kind as string)) throw new Error('잘못된 입력입니다')
  if (withPath) {
    if (typeof i.path !== 'string' || !i.path.trim() || i.path.length > MAX_PATH_LEN || i.path.includes('\0')) {
      throw new Error('잘못된 경로입니다')
    }
    if (i.group !== undefined && (typeof i.group !== 'string' || i.group.length > 200)) throw new Error('잘못된 그룹 이름입니다')
    if ((i.kind === 'local' || i.kind === 'wsl') && isUncPath(i.path) && !isWslUnc(i.path)) {
      throw new Error('네트워크 공유 경로는 지원하지 않습니다(WSL 경로는 \\\\wsl.localhost\\… 로 고르세요)')
    }
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
    const known = (await listSshHosts()).some((h) => h.alias === host)
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
    }
  }
}

export function registerIpc(config: ConfigStore, repos: RepoService, watcher: RepoWatcher, gh: GitHubService): void {
  handle('config:get', () => config.get())
  handle('config:setUi', (_e, patch) => config.setUi(patch))

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
  handle('repos:update', (_e, repoId, patch) => config.updateRepo(repoId, patch))
  handle('repos:remove', async (_e, repoId) => {
    const next = config.removeRepo(repoId)
    if (watcher.current === repoId) void watcher.watch(null)
    await repos.releaseUnused()
    return next
  })
  handle('repos:watch', (_e, repoId) => watcher.watch(repoId))

  handle('env:wslDistros', () => listWslDistros())
  handle('env:sshHosts', () => listSshHosts())
  handle('env:test', async (e, input) => {
    await checkEnvInput(e, input, false)
    return repos.testEnv(input)
  })

  handle('git:refs', (_e, repoId) => repos.refs(repoId))
  handle('git:log', (_e, repoId, query, requestId) => repos.log(repoId, query, requestId))
  handle('git:search', (_e, repoId, query, ref, requestId) => repos.search(repoId, query, ref, requestId))
  handle('git:commit', (_e, repoId, hash, requestId) => repos.commit(repoId, hash, requestId))
  handle('git:fileDiff', (_e, repoId, req, requestId) => repos.fileDiff(repoId, req, requestId))
  registerGithubIpc(handle, gh, repos)
  handle('req:cancel', (_e, requestId) => {
    repos.cancel(requestId)
    gh.cancel(requestId)
  })
}
