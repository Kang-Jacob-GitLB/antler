// `ANTLER_RUST_GIT=0` 되돌리기 전용 조립: Rust 가 설정을 소유하지 않는 실행(env ANTLER_RUST_CONFIG 없음)에서 사이드카가 src/main 의 서비스 전부를 만들어 모든 채널을 처리한다.
// Rust 가 설정을 소유하는 실행(Phase 6b PR 9 부터 기본)에서는 이 모듈을 부르지 않는다 — 사이드카는 UpdateService 만 조립한다(PR 10). 되돌리기가 사라지는 6c 에서 이 파일째 지운다.
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
import type { UpdateService } from '../main/update/UpdateService'
import { FetchPrefsSender } from './fetchPrefs'
import { MirrorSender } from './mirror'
import type { FromSidecar } from './protocol'

export interface LegacyServices {
  /** 창 포커스(src/main/index.ts 의 win.on('focus'|'blur') 와 같다) */
  setFocused(focused: boolean): void
  /** 서비스를 정리한다. 끝나면(저장소 서비스가 닫히면) 이행된다 */
  dispose(): Promise<void>
}

export function assembleLegacy(
  userData: string,
  out: (msg: FromSidecar) => void,
  emit: (name: string, payload: unknown) => void,
  updates: UpdateService
): LegacyServices {
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

  let netConfig: readonly NetworkConfigEntry[] = []
  const netReady = readNetworkConfig().then((c) => {
    netConfig = c
  })
  // 안전한 암호화 저장소가 없으므로 토큰은 메모리에만 둔다(디스크에 쓰지 않는다)
  const noSafeStorage = { isEncryptionAvailable: () => false, encryptString: () => Buffer.alloc(0), decryptString: () => '' }
  const repos = new RepoService(config, { cacheDir, githubToken: () => gh.gitToken(), noHooksDir, netConfig: () => netConfig, netReady })
  const watcher = new RepoWatcher(repos, (repoId) => emit('repo:changed', { repoId }))
  const fetcher = new FetchService(
    config,
    repos,
    (status) => emit('fetch:status', status),
    (repoId) => emit('repo:changed', { repoId }),
    noHooksDir
  )
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
  registerIpc(config, repos, watcher, gh, jira, fetcher, updates)
  repos.cleanupGithubCache().catch((err) => console.error('원격 저장소 캐시 정리 실패:', err))

  return {
    setFocused(focused) {
      watcher.setFocused(focused)
      if (focused) fetcher.onFocus()
    },
    dispose() {
      watcher.dispose()
      fetcher.dispose()
      gh.dispose()
      jira.dispose()
      return repos.dispose()
    }
  }
}
