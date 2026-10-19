import { spawn } from 'node:child_process'
import { lstatSync, realpathSync, statSync } from 'node:fs'
import { BrowserWindow, dialog, shell } from 'electron'
import type { ExternalAction } from '@shared/types'
import { ExternalOpener, checkChosenExe, type OpenerDeps } from '../external/opener'
import type { Target } from '../external/plan'
import type { ConfigStore } from '../services/ConfigStore'
import type { IpcHandle } from './github'

const ACTIONS: readonly string[] = ['fileManager', 'editor', 'terminal'] satisfies ExternalAction[]

const realDeps = (): OpenerDeps => ({
  platform: process.platform,
  env: process.env,
  fs: { realpathSync: (p) => realpathSync(p), statSync: (p) => statSync(p), lstatSync: (p) => lstatSync(p) },
  spawn: (file, args, opts) => spawn(file, args, opts),
  shell: { openPath: (p) => shell.openPath(p), showItemInFolder: (p) => shell.showItemInFolder(p) }
})

/**
 * external:* 채널 등록. renderer 는 저장소 id·동작·저장소 안의 상대 경로만 보낸다 —
 * 저장소 루트·환경(WSL 배포판·SSH 호스트)은 저장된 설정에서, 실행 파일은 앱이 찾은 위치나 사용자가 고른 파일에서만 정한다.
 */
export function registerExternalIpc(handle: IpcHandle, config: ConfigStore, opener: ExternalOpener = new ExternalOpener(realDeps())): void {
  handle('external:open', async (_e, repoId, req) => {
    if (!req || typeof req !== 'object' || !ACTIONS.includes(req.action) || (req.path !== undefined && typeof req.path !== 'string')) {
      throw new Error('잘못된 입력입니다')
    }
    const repo = typeof repoId === 'string' ? config.repo(repoId) : undefined
    const source = repo && config.source(repo.sourceId)
    if (!repo || !source) throw new Error('저장소를 찾을 수 없습니다')
    const target: Target =
      source.kind === 'wsl'
        ? { kind: 'wsl', root: repo.path, distro: source.distro ?? '' }
        : source.kind === 'ssh'
          ? { kind: 'ssh', root: repo.path, host: source.sshHost ?? '' }
          : { kind: 'local', root: repo.path }
    await opener.open(target, req.action, req.path, config.get().ui)
  })

  handle('external:pickEditorExe', async (e) => {
    const win = BrowserWindow.fromWebContents(e.sender)
    const opts: Electron.OpenDialogOptions = {
      properties: ['openFile'],
      title: '에디터 실행 파일 선택',
      ...(process.platform === 'win32' ? { filters: [{ name: '실행 파일', extensions: ['exe', 'com'] }] } : {})
    }
    const res = win ? await dialog.showOpenDialog(win, opts) : await dialog.showOpenDialog(opts)
    const path = res.filePaths[0]
    if (res.canceled || !path) return null
    return config.setEditorExePath(checkChosenExe(path, process.platform, realDeps()))
  })
}
