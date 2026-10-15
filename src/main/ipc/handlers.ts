import { BrowserWindow, dialog, ipcMain, type IpcMainInvokeEvent } from 'electron'
import type { IpcArgs, IpcChannel, IpcEnvelope, IpcResult } from '@shared/ipc'
import { AbortError, GitError } from '../executors/GitExecutor'
import type { ConfigStore } from '../services/ConfigStore'
import { StaleLogError, type RepoService } from '../services/RepoService'

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
            : err instanceof GitError
              ? 'GIT'
              : 'ERROR'
      return { ok: false, error: err instanceof Error ? err.message : String(err), code }
    }
  })
}

export function registerIpc(config: ConfigStore, repos: RepoService): void {
  handle('config:get', () => config.get())
  handle('config:setUi', (_e, patch) => config.setUi(patch))

  handle('repos:pickLocal', async (e) => {
    const win = BrowserWindow.fromWebContents(e.sender)
    const opts = { properties: ['openDirectory'] as Array<'openDirectory'>, title: 'Git 저장소 선택' }
    const res = win ? await dialog.showOpenDialog(win, opts) : await dialog.showOpenDialog(opts)
    const path = res.filePaths[0]
    if (res.canceled || !path) return null
    return repos.addLocal(path)
  })
  handle('repos:remove', (_e, repoId) => config.removeRepo(repoId))

  handle('git:refs', (_e, repoId) => repos.refs(repoId))
  handle('git:log', (_e, repoId, query, requestId) => repos.log(repoId, query, requestId))
  handle('git:search', (_e, repoId, query, ref, requestId) => repos.search(repoId, query, ref, requestId))
  handle('git:commit', (_e, repoId, hash, requestId) => repos.commit(repoId, hash, requestId))
  handle('git:fileDiff', (_e, repoId, req, requestId) => repos.fileDiff(repoId, req, requestId))
  handle('req:cancel', (_e, requestId) => repos.cancel(requestId))
}
