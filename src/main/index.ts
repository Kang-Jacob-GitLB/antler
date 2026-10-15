import { join } from 'node:path'
import { app, BrowserWindow, nativeTheme, session, shell } from 'electron'
import { registerIpc } from './ipc/handlers'
import { ConfigStore } from './services/ConfigStore'
import { RepoService } from './services/RepoService'

app.setName('Antler')
// 테스트·자동화용으로 설정 저장 위치를 바꿀 수 있다(실사용 설정을 건드리지 않기 위해)
if (process.env['ANTLER_USER_DATA']) app.setPath('userData', process.env['ANTLER_USER_DATA'])
if (process.platform === 'win32') app.setAppUserModelId('com.antler.app')

// 토큰 --bg-base 와 같은 값. 첫 페인트 전 흰 화면 깜빡임을 막는다.
const BG = { dark: '#14110F', light: '#F6F1E7' }

let config: ConfigStore
let repos: RepoService

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

  // 외부 링크는 기본 브라우저로, 앱 내 탐색은 막는다
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https:\/\//.test(url)) void shell.openExternal(url)
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
  registerIpc(config, repos)

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
  void repos?.dispose()
})
