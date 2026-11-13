// Tauri 용 개발 전용 Node 사이드카. src/main 의 서비스 조립(src/main/index.ts)을 electron 없이 재현하고,
// registerIpc 를 그대로 재사용해 Rust 가 stdin 으로 넘기는 채널 호출을 처리한다. 프로토콜은 protocol.ts.
import { createInterface } from 'node:readline'
import { join } from 'node:path'
import { GitHubService } from '../main/github/service'
import { createGhCli } from '../main/github/ghCli'
import { AVATAR_CACHE_FILE } from '../main/github/avatars'
import { TokenStore } from '../main/github/tokenStore'
import { JiraService, JIRA_META_FILE, JIRA_TOKEN_FILE } from '../main/jira/service'
import { registerIpc } from '../main/ipc/handlers'
import { ConfigStore } from '../main/services/ConfigStore'
import { FetchService } from '../main/services/FetchService'
import { readNetworkConfig, type NetworkConfigEntry } from '../main/git/githubCache'
import { RepoService } from '../main/services/RepoService'
import { RepoWatcher } from '../main/services/RepoWatcher'
import { UpdateService } from '../main/update/UpdateService'
import { bindHost, invokeHandler, resolveHost } from './electronShim'
import type { FromSidecar, ToSidecar } from './protocol'
import { IPC_CHANNELS } from '@shared/ipc'

// stdout 은 프로토콜 전용이다 — 서비스가 console.log/info 를 써도 프로토콜 줄이 깨지지 않게 stderr 로 돌린다
console.log = console.error
console.info = console.error

const userData = process.env['ANTLER_USER_DATA']
if (!userData) {
  console.error('ANTLER_USER_DATA 가 필요합니다')
  process.exit(1)
}

// 짝 없는 서로게이트(.slice 가 이모지 가운데를 자른 문자열 등)는 JSON.stringify 가 "\ud83d" 로 내보내는데,
// Rust serde_json 은 이를 거부해 그 응답이 버려지고 호출이 영원히 끝나지 않는다. U+FFFD 로 바꿔 보낸다
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g
const wellFormed = (_key: string, v: unknown): unknown => (typeof v === 'string' ? v.replace(LONE_SURROGATE, '�') : v)

const out = (msg: FromSidecar): void => {
  process.stdout.write(`${JSON.stringify(msg, wellFormed)}\n`)
}
bindHost(out)
const emit = (name: string, payload: unknown): void => out({ t: 'evt', name, payload })

// GitHub 원격 전용 저장소의 앱 캐시·빈 hooks 디렉터리(src/main/index.ts 와 같은 위치 규칙)
const cacheDir = join(userData, 'remote-repos')
const noHooksDir = join(userData, 'no-hooks')
const config = new ConfigStore(join(userData, 'antler.config.json'), { cacheDir })
let netConfig: readonly NetworkConfigEntry[] = []
const netReady = readNetworkConfig().then((c) => {
  netConfig = c
})

const repos = new RepoService(config, { cacheDir, githubToken: () => gh.gitToken(), noHooksDir, netConfig: () => netConfig, netReady })
const watcher = new RepoWatcher(repos, (repoId) => emit('repo:changed', { repoId }))
const fetcher = new FetchService(
  config,
  repos,
  (status) => emit('fetch:status', status),
  (repoId) => emit('repo:changed', { repoId }),
  noHooksDir
)

// 안전한 암호화 저장소가 없으므로 토큰은 메모리에만 둔다(디스크에 쓰지 않는다)
const noSafeStorage = { isEncryptionAvailable: () => false, encryptString: () => Buffer.alloc(0), decryptString: () => '' }
const gh: GitHubService = new GitHubService({
  config,
  repos,
  store: new TokenStore(userData, noSafeStorage),
  cli: createGhCli(),
  onAuthChanged: () => fetcher.resumeGithubAuth(),
  avatarCacheFile: join(userData, AVATAR_CACHE_FILE)
})
const jira = new JiraService({
  store: new TokenStore(userData, noSafeStorage, process.platform, { token: JIRA_TOKEN_FILE, meta: JIRA_META_FILE }),
  allowInsecureLoopback: false
})
// 업데이트는 비활성(isPackaged=false, ANTLER_UPDATE_DEV 없음)
const updates = new UpdateService({
  currentVersion: '0.0.0-tauri-poc',
  platform: process.platform,
  arch: process.arch,
  env: {},
  isPackaged: false,
  getSkipped: () => undefined,
  setSkipped: () => {},
  send: (s) => emit('update:state', s),
  downloadsDir: () => join(userData, 'updates'),
  openPath: () => Promise.resolve('사이드카(PoC): openPath 미지원'),
  showItem: () => {}
})
registerIpc(config, repos, watcher, gh, jira, fetcher, updates)
updates.start()
repos.cleanupGithubCache().catch((err) => console.error('원격 저장소 캐시 정리 실패:', err))

// preload 의 허용 목록과 같은 검사. renderer 의 bridge 검사는 __TAURI__ 로 우회할 수 있으므로 여기서 다시 막는다
const allowedChannels = new Set<string>(IPC_CHANNELS)

async function onRequest(id: number, ch: string, args: unknown[]): Promise<void> {
  const pending = allowedChannels.has(ch) ? invokeHandler(ch, args) : undefined
  const env = pending ? await pending : { ok: false, error: `알 수 없는 채널: ${ch}`, code: 'ERROR' }
  out({ t: 'res', id, env: env as Extract<FromSidecar, { t: 'res' }>['env'] })
}

function onLine(line: string): void {
  let msg: ToSidecar
  try {
    msg = JSON.parse(line) as ToSidecar
  } catch {
    console.error('JSON 이 아닌 입력 줄 무시')
    return
  }
  if (msg.t === 'req') {
    void onRequest(msg.id, msg.ch, Array.isArray(msg.args) ? msg.args : []).catch((err) =>
      out({ t: 'res', id: msg.id, env: { ok: false, error: err instanceof Error ? err.message : String(err), code: 'ERROR' } })
    )
  } else if (msg.t === 'focus') {
    // src/main/index.ts 의 win.on('focus'|'blur') 와 같다
    watcher.setFocused(msg.focused)
    if (msg.focused) fetcher.onFocus()
  } else if (msg.t === 'hostres') {
    resolveHost(msg.id, msg.ok)
  }
}

function shutdown(): void {
  watcher.dispose()
  fetcher.dispose()
  gh.dispose()
  jira.dispose()
  updates.dispose()
  void repos.dispose().finally(() => process.exit(0))
}

const rl = createInterface({ input: process.stdin })
rl.on('line', onLine)
// stdin 이 닫히면(Rust 종료) 정리하고 끝낸다
rl.on('close', shutdown)
out({ t: 'ready' })
