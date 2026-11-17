// Tauri 용 개발 전용 Node 사이드카. src/main 의 서비스 조립(src/main/index.ts)을 electron 없이 재현하고,
// registerIpc 를 그대로 재사용해 Rust 가 stdin 으로 넘기는 채널 호출을 처리한다. 프로토콜은 protocol.ts.
import { createInterface } from 'node:readline'
import { inspect } from 'node:util'
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
import { rustOwnsApi, sidecarSkips } from './apiOwns'
import { handleFetchApprove } from './fetchApprove'
import { fetchOwner, parseRustFetchKinds } from './fetchOwns'
import { FetchPrefsSender } from './fetchPrefs'
import { GhTokenMirror, handleGhToken } from './ghToken'
import { MirrorSender } from './mirror'
import { handleNote } from './note'
import { RepoQueryHandler } from './repoQuery'
import { UNDEFINED_ARG, type FromSidecar, type ToSidecar } from './protocol'
import { IPC_CHANNELS } from '@shared/ipc'

// stdout 은 프로토콜 전용이다 — 서비스가 console.log/info 를 써도 프로토콜 줄이 깨지지 않게 stderr 로 돌린다
console.log = console.error
console.info = console.error
// Node 의 console.debug·dir 도 stdout 으로 쓴다 — 프로토콜 줄({t:'repos'} 등)을 위조할 수 있는 길이 되지 않게 함께 돌린다
console.debug = console.error
console.dir = (item, options) => void process.stderr.write(`${inspect(item, options)}\n`)

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
// Rust 라우터가 쓰는 저장소 미러. 시작 때(ready 전) 한 번, 이후 목록이 바뀔 때마다 보낸다. update 안에서 동기로 불리므로
// 저장소를 바꾸는 호출의 res 줄보다 repos 줄이 먼저 나가 renderer 의 다음 호출은 새 미러를 본다
const mirror = new MirrorSender(out)
// fetch 설정(자동 주기·승인 지문)은 미러와 따로 보낸다. 반드시 미러 다음에(저장소가 사라진 설정을 승인 맵이 앞서가지 않게), 시작 때도 ready 전에
const fetchPrefs = new FetchPrefsSender(out)
config.onChange = (c) => {
  mirror.push(c)
  fetchPrefs.push(c)
}
mirror.push(config.get())
fetchPrefs.push(config.get())
// Rust 가 jira:*·gh:*(gh:repo 제외)를 소유한다(env ANTLER_RUST_API=1). 라우터가 가로채므로 그 채널은 여기까지 오지 않아야 한다 — 오면 처리하지 않는다
const rustApi = rustOwnsApi(process.env['ANTLER_RUST_API'])
// rustApi 일 때 GitHub 토큰은 Rust 가 stdin(`ghToken`)으로 보낸 메모리 사본이다. 이 사이드카의 GitHubService 는 로그인하지 않는다(gh:* 를 받지 않는다)
const ghMirror = new GhTokenMirror()
let netConfig: readonly NetworkConfigEntry[] = []
const netReady = readNetworkConfig().then((c) => {
  netConfig = c
})

const repos = new RepoService(config, { cacheDir, githubToken: () => (rustApi ? ghMirror.get() : gh.gitToken()), noHooksDir, netConfig: () => netConfig, netReady })
const watcher = new RepoWatcher(repos, (repoId) => emit('repo:changed', { repoId }))
const fetcher = new FetchService(
  config,
  repos,
  (status) => emit('fetch:status', status),
  (repoId) => emit('repo:changed', { repoId }),
  noHooksDir,
  // Rust 가 fetch 를 맡는 환경(env 가 있을 때만)의 저장소는 이 서비스가 fetch·자동 예약하지 않는다
  fetchOwner(() => config.get(), parseRustFetchKinds(process.env['ANTLER_RUST_FETCH_KINDS']))
)

// 안전한 암호화 저장소가 없으므로 토큰은 메모리에만 둔다(디스크에 쓰지 않는다)
const noSafeStorage = { isEncryptionAvailable: () => false, encryptString: () => Buffer.alloc(0), decryptString: () => '' }
const gh: GitHubService = new GitHubService({
  config,
  repos,
  store: new TokenStore(userData, noSafeStorage),
  cli: createGhCli(),
  onAuthChanged: () => fetcher.resumeGithubAuth(),
  // Rust 가 GitHub 를 처리하면 아바타 캐시 파일도 Rust 의 것이다 — 이 서비스는 파일을 쓰지 않는다
  ...(rustApi ? {} : { avatarCacheFile: join(userData, AVATAR_CACHE_FILE) })
})
// Rust 의 GitHub 서비스가 저장소 정보를 물으면(`repoq`) 여기서 답한다
const repoQuery = new RepoQueryHandler(repos, config, (id, env) => out({ t: 'res', id, env }))
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

/** bridge 의 encodeArgs 를 되돌린다 — 표식 객체(키가 정확히 그것 하나)만 undefined 로 바꾼다 */
function decodeArgs(args: unknown[]): unknown[] {
  return args.map((a) =>
    a !== null && typeof a === 'object' && !Array.isArray(a) && Object.keys(a).length === 1 && (a as Record<string, unknown>)[UNDEFINED_ARG] === true
      ? undefined
      : a
  )
}

async function onRequest(id: number, ch: string, rawArgs: unknown[]): Promise<void> {
  const args = decodeArgs(rawArgs)
  const owned = sidecarSkips(ch, rustApi)
  const pending = allowedChannels.has(ch) && !owned ? invokeHandler(ch, args) : undefined
  const env = pending ? await pending : { ok: false, error: owned ? `Rust 가 처리하는 채널입니다: ${ch}` : `알 수 없는 채널: ${ch}`, code: 'ERROR' }
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
  } else if (msg.t === 'note') {
    handleNote(repos, msg)
  } else if (msg.t === 'fetchApprove') {
    // 승인을 쓰는 유일한 길. req(채널) 경로로는 오지 않는다 — Rust 의 fetch 서비스만 이 줄을 만든다
    // 저장 실패(백신·OneDrive 가 설정 파일을 잡음, 디스크 가득 참)는 "다음에 다시 묻는다"로 끝낸다(Electron FetchService 와 같다).
    // 감싸지 않으면 readline 처리기의 예외로 사이드카가 죽어 Rust 가 맡지 않은 모든 채널이 멈춘다(보안 리뷰)
    try {
      handleFetchApprove(config, msg)
    } catch (err) {
      console.error('fetch 승인 저장 실패:', err)
    }
  } else if (msg.t === 'repoq') {
    // Rust 의 GitHub 서비스가 묻는 저장소 정보. Rust 가 GitHub 를 처리할 때(ANTLER_RUST_API=1)만 온다 — 아니면 무시한다
    if (rustApi) repoQuery.handle(msg)
  } else if (msg.t === 'repoqCancel') {
    if (rustApi) repoQuery.cancel(msg.id)
  } else if (msg.t === 'ghToken') {
    // 토큰의 메모리 사본. 값은 어디에도 적지 않는다(로그·오류 포함). 바뀌면 원격 전용 저장소의 인증 실패 정지를 푼다(TS 의 onAuthChanged)
    if (rustApi) handleGhToken(ghMirror, msg, () => fetcher.resumeGithubAuth())
  } else if (msg.t === 'focus') {
    // src/main/index.ts 의 win.on('focus'|'blur') 와 같다
    watcher.setFocused(msg.focused)
    if (msg.focused) fetcher.onFocus()
  } else if (msg.t === 'hostres') {
    resolveHost(msg.id, msg.ok, msg.value)
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
