import { join } from 'node:path'
import { app, BrowserWindow, nativeTheme, safeStorage, session, shell } from 'electron'
import { GitHubService } from './github/service'
import { TokenStore } from './github/tokenStore'
import { registerIpc } from './ipc/handlers'
import { allowedExternalUrl } from './github/validate'
import { ConfigStore } from './services/ConfigStore'
import { RepoService } from './services/RepoService'
import { RepoWatcher } from './services/RepoWatcher'

app.setName('Antler')
// 테스트·자동화용으로 설정 저장 위치를 바꿀 수 있다(실사용 설정을 건드리지 않기 위해)
if (process.env['ANTLER_USER_DATA']) app.setPath('userData', process.env['ANTLER_USER_DATA'])
if (process.platform === 'win32') app.setAppUserModelId('com.antler.app')

// 하드웨어 가속은 기본으로 끈다. 이 앱은 Canvas 2D·텍스트 위주라 GPU 합성 이득이 없고, GPU 프로세스가
// 메모리를 크게 쓴다(10만 커밋 저장소 실측: private 506MB → 280MB, 스크롤 p95 16.8ms → 13.4ms).
// 드라이버 문제 등으로 비교가 필요하면 ANTLER_GPU=1 로 켤 수 있다.
if (process.env['ANTLER_GPU'] !== '1') app.disableHardwareAcceleration()

// 토큰 --bg-base 와 같은 값. 첫 페인트 전 흰 화면 깜빡임을 막는다.
const BG = { dark: '#14110F', light: '#F6F1E7' }

let config: ConfigStore
let repos: RepoService
let gh: GitHubService
let watcher: RepoWatcher | undefined

function createWindow(): BrowserWindow {
  const pref = config.get().ui.theme
  const dark = pref === 'system' ? nativeTheme.shouldUseDarkColors : pref === 'dark'
  const win = new BrowserWindow({
    title: 'Antler',
    width: 1440,
    height: 900,
    minWidth: 960,
    minHeight: 600,
    show: false,
    backgroundColor: dark ? BG.dark : BG.light,
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      spellcheck: false
    }
  })
  win.once('ready-to-show', () => win.show())
  // 창이 뒤에 있을 때는 원격 폴링을 늦추고, 다시 앞으로 오면 바로 확인한다
  win.on('focus', () => watcher?.setFocused(true))
  win.on('blur', () => watcher?.setFocused(false))

  // 외부 링크는 기본 브라우저로, 앱 내 탐색은 막는다
  // 새 창은 열지 않는다. github.com 링크만 기본 브라우저로 보낸다(gh:openExternal 과 같은 허용 목록)
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (allowedExternalUrl(url)) void shell.openExternal(url)
    return { action: 'deny' }
  })
  win.webContents.on('will-navigate', (e, url) => {
    if (url !== win.webContents.getURL()) e.preventDefault()
  })

  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (!app.isPackaged && devUrl) void win.loadURL(devUrl)
  else void win.loadFile(join(__dirname, '../renderer/index.html'))
  return win
}

app.whenReady().then(() => {
  config = new ConfigStore(join(app.getPath('userData'), 'antler.config.json'))
  repos = new RepoService(config)
  // 활성 저장소의 ref 가 바뀌면(외부에서 커밋·fetch·체크아웃) renderer 에 알린다
  watcher = new RepoWatcher(repos, (repoId) => {
    for (const w of BrowserWindow.getAllWindows()) w.webContents.send('repo:changed', { repoId })
  })
  gh = new GitHubService({ config, repos, store: new TokenStore(app.getPath('userData'), safeStorage) })
  registerIpc(config, repos, watcher, gh)

  // 카메라·마이크 등 어떤 권한도 주지 않는다
  session.defaultSession.setPermissionRequestHandler((_wc, _perm, cb) => cb(false))

  nativeTheme.on('updated', () => {
    for (const w of BrowserWindow.getAllWindows()) {
      w.webContents.send('theme:system', { dark: nativeTheme.shouldUseDarkColors })
    }
  })

  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})

app.on('before-quit', () => {
  watcher?.dispose()
  gh?.dispose()
  void repos?.dispose()
})
