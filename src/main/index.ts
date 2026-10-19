import { readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { app, BrowserWindow, nativeTheme, safeStorage, session, shell } from 'electron'
import { AVATAR_CACHE_FILE } from './github/avatars'
import { createDevFakeFetch } from './github/devFake'
import { GitHubService } from './github/service'
import { createGhCli } from './github/ghCli'
import { TokenStore } from './github/tokenStore'
import { JiraService, JIRA_META_FILE, JIRA_TOKEN_FILE } from './jira/service'
import { registerIpc } from './ipc/handlers'
import { allowedExternalUrl } from './github/validate'
import { ConfigStore } from './services/ConfigStore'
import { FetchService } from './services/FetchService'
import { RepoService } from './services/RepoService'
import { RepoWatcher } from './services/RepoWatcher'
import { createAutoBackend } from './update/autoBackend'
import { UpdateService } from './update/UpdateService'
import { TOOLBAR_HEIGHT, titleBarOverlay } from './titleBar'

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
let jira: JiraService
let watcher: RepoWatcher | undefined
let updates: UpdateService | undefined
let fetcher: FetchService | undefined

/**
 * 창 아이콘. 패키지된 Windows·macOS 는 실행 파일·번들 아이콘을 쓰므로 지정하지 않는다.
 * Linux 는 실행 파일 아이콘이 없어(.desktop 없이 AppImage 를 바로 띄우는 경우 등) extraResources 의 icon.png 를 준다.
 */
function windowIcon(): { icon?: string } {
  if (!app.isPackaged) return { icon: join(__dirname, '../../resources/icon.png') }
  if (process.platform === 'linux') return { icon: join(process.resourcesPath, 'icon.png') }
  return {}
}

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
    ...windowIcon(),
    autoHideMenuBar: true,
    // 앱 툴바가 제목 표시줄을 겸한다. mac 은 신호등 버튼을 툴바 세로 중앙에, 나머지는 창 컨트롤 오버레이
    ...(process.platform === 'darwin'
      ? { titleBarStyle: 'hidden' as const, trafficLightPosition: { x: 16, y: (TOOLBAR_HEIGHT - 14) / 2 } }
      : { titleBarStyle: 'hidden' as const, titleBarOverlay: titleBarOverlay(dark) }),
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
  win.on('focus', () => {
    watcher?.setFocused(true)
    fetcher?.onFocus()
  })
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
  fetcher = new FetchService(
    config,
    repos,
    (status) => {
      for (const w of BrowserWindow.getAllWindows()) w.webContents.send('fetch:status', status)
    },
    (repoId) => {
      for (const w of BrowserWindow.getAllWindows()) w.webContents.send('repo:changed', { repoId })
    },
    // 로컬 git 의 core.hooksPath 로 쓸 앱 소유의 빈 디렉터리(Windows 에서 /dev/null 은 드라이브 루트의 \dev\null 로 풀린다)
    join(app.getPath('userData'), 'no-hooks')
  )
  // 화면 검증용 가짜 GitHub: 패키징되지 않은 빌드에서만 켜진다. 토큰은 메모리에만 두고, 저장소 경로도 존재하지 않는 임시 경로라
  // 실제 userData 의 토큰 파일은 읽지도(load) 지우지도(clear) 않는다
  const devFake = !app.isPackaged ? process.env['ANTLER_GH_FAKE'] : undefined
  const ghStore = devFake
    ? new TokenStore(join(tmpdir(), `antler-gh-fake-${process.pid}`), { isEncryptionAvailable: () => false, encryptString: () => Buffer.alloc(0), decryptString: () => '' })
    : new TokenStore(app.getPath('userData'), safeStorage)
  if (devFake && process.env['ANTLER_GH_FAKE_SIGNEDOUT'] !== '1') ghStore.save('ghp_devfaketoken', 'pat')
  gh = new GitHubService({
    config,
    repos,
    store: ghStore,
    ...(devFake ? { fetchImpl: createDevFakeFetch(devFake) } : {}),
    cli: createGhCli(),
    ...(devFake ? {} : { avatarCacheFile: join(app.getPath('userData'), AVATAR_CACHE_FILE) }),
    ...fakeGithubApi()
  })
  jira = new JiraService({
    store: new TokenStore(app.getPath('userData'), safeStorage, process.platform, { token: JIRA_TOKEN_FILE, meta: JIRA_META_FILE }),
    // 가짜 Jira 서버(테스트·화면 확인)에 한해 루프백 http 를 허용한다. 패키지된 앱에서는 환경 변수가 있어도 무시한다
    allowInsecureLoopback: !app.isPackaged && process.env['ANTLER_JIRA_ALLOW_LOOPBACK'] === '1'
  })
  updates = new UpdateService({
    currentVersion: __APP_VERSION__,
    platform: process.platform,
    arch: process.arch,
    env: process.env,
    isPackaged: app.isPackaged,
    probe: { execPath: process.execPath, ...packageTypeProbe() },
    getSkipped: () => config.get().ui.skippedUpdateVersion,
    setSkipped: (v) => void config.setSkippedUpdateVersion(v),
    send: (s) => {
      for (const w of BrowserWindow.getAllWindows()) w.webContents.send('update:state', s)
    },
    createAutoBackend: () =>
      createAutoBackend({
        currentVersion: __APP_VERSION__,
        packaged: app.isPackaged,
        ...(process.env['ANTLER_UPDATE_FEED'] ? { feedUrl: process.env['ANTLER_UPDATE_FEED'] } : {})
      }),
    downloadsDir: () => join(app.getPath('userData'), 'updates'),
    openPath: (p) => shell.openPath(p),
    showItem: (p) => shell.showItemInFolder(p)
  })
  registerIpc(config, repos, watcher, gh, jira, fetcher, updates)
  updates.start()

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
  fetcher?.dispose()
  gh?.dispose()
  jira?.dispose()
  updates?.dispose()
  void repos?.dispose()
})

/**
 * 화면 확인용 가짜 GitHub API: 패키지되지 않은 실행에서 ANTLER_GH_API_BASE 가 루프백 주소(http://127.0.0.1:포트)일 때만
 * api.github.com 요청을 그쪽으로 돌린다. 패키지된 앱에서는 환경 변수가 있어도 무시한다(실제 GitHub 만 쓴다).
 */
function fakeGithubApi(): { fetchImpl?: typeof fetch } {
  const base = process.env['ANTLER_GH_API_BASE']
  if (app.isPackaged || !base || !/^http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):\d+$/.test(base)) return {}
  return { fetchImpl: (input, init) => fetch(String(input).replace(/^https:\/\/api\.github\.com(?=\/)/, base), init) }
}

/** resources/package-type(deb·rpm·pacman 패키지에만 있다)을 읽는다. 없으면 빈 객체 */
function packageTypeProbe(): { packageType?: string } {
  if (process.platform !== "linux" || !app.isPackaged) return {}
  try {
    return { packageType: readFileSync(join(process.resourcesPath, "package-type"), "utf8") }
  } catch {
    return {}
  }
}
