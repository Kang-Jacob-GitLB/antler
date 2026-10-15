// electron-updater 백엔드(Windows NSIS · Linux AppImage). GitHub 공급자는 electron-builder.yml 의 publish 설정이
// 만든 app-update.yml 에서 읽는다. 개발 테스트(ANTLER_UPDATE_DEV=1)에서는 ANTLER_UPDATE_FEED 로 generic 피드를 줄 수 있다.
import { CancellationToken, autoUpdater } from 'electron-updater'
import { isPrerelease } from './logic'
import type { AutoBackend } from './UpdateService'

export function createAutoBackend(opts: { currentVersion: string; packaged: boolean; feedUrl?: string }): AutoBackend {
  autoUpdater.autoDownload = false // 사용자가 [업데이트] 를 누를 때만 내려받는다
  autoUpdater.autoInstallOnAppQuit = false // 설치는 [다시 시작하여 설치] 로만
  autoUpdater.allowDowngrade = false
  autoUpdater.allowPrerelease = isPrerelease(opts.currentVersion)
  autoUpdater.logger = null
  if (!opts.packaged) autoUpdater.forceDevUpdateConfig = true
  if (!opts.packaged && opts.feedUrl && /^https?:\/\//.test(opts.feedUrl)) autoUpdater.setFeedURL({ provider: 'generic', url: opts.feedUrl })
  // 'error' 리스너가 없으면 EventEmitter 가 던진다 — 실제 오류는 check/download 의 reject 로 처리한다
  autoUpdater.on('error', () => {})

  let token: CancellationToken | null = null
  return {
    async check() {
      const r = await autoUpdater.checkForUpdates()
      // isUpdateAvailable: electron-updater 가 버전·채널 규칙으로 판단한 결과
      return r?.isUpdateAvailable ? r.updateInfo.version : null
    },
    async download(onProgress) {
      token = new CancellationToken()
      const onP = (p: { percent: number }): void => onProgress(p.percent)
      autoUpdater.on('download-progress', onP)
      try {
        await autoUpdater.downloadUpdate(token)
      } finally {
        autoUpdater.removeListener('download-progress', onP)
        token = null
      }
    },
    cancel() {
      token?.cancel()
    },
    quitAndInstall() {
      // 조용히 설치(기존 설치 경로 유지)하고 끝나면 앱을 다시 실행
      autoUpdater.quitAndInstall(true, true)
    }
  }
}
