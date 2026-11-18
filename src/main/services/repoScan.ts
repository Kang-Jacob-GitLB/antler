// 저장소 추가 창의 "폴더 안 저장소 찾기": 고른 폴더 바로 아래 하위 폴더 중 .git 이 있는 것을 고른다(읽기 전용).
// 로컬과 WSL(\\wsl.localhost\… 공유 경로)은 같은 파일 시스템 API 로 읽는다. 진짜 저장소인지는 추가할 때 git 이 확인한다.
import { readdir, stat } from 'node:fs/promises'
import { join, posix } from 'node:path'
import type { RemoteDirListing, RepoScanResult } from '@shared/types'
import { isRemoteLinkTarget, linksStayLocal } from '../git/linkGuard'

/** 하위 폴더 상한 — 아주 큰 폴더(예: node_modules)에서 창이 굳지 않게 */
const MAX_DIRS = 2000
/** 동시에 띄우는 stat 수. WSL 공유(9P)가 느릴 때 libuv 스레드 풀을 다 차지해 다른 파일 작업을 막지 않게 */
const PROBE_CONCURRENCY = 8

/** stat 은 링크를 따라간다 — 원격 UNC 를 가리키는 링크는 접속(SMB·WebDAV 인증)하기 전에 건너뛴다(없는 것으로 본다) */
const exists = (p: string, isRemote: (t: string) => boolean): Promise<boolean> =>
  linksStayLocal(p, isRemote)
    ? stat(p).then(
        () => true,
        () => false
      )
    : Promise.resolve(false)

/** items 를 limit 개씩 동시에 돌려 순서대로 결과를 모은다 */
async function mapLimit<T, R>(items: T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const out = new Array<R>(items.length)
  let next = 0
  const worker = async (): Promise<void> => {
    while (next < items.length) {
      const i = next++
      out[i] = await fn(items[i] as T)
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker))
  return out
}

/** 로컬·WSL 공유 경로: 고른 폴더 바로 아래에서 .git 이 있는 폴더를 찾는다(고른 폴더가 저장소가 아님은 호출자가 확인했다) */
export async function scanLocalRepos(root: string, isRemote: (target: string) => boolean = isRemoteLinkTarget): Promise<RepoScanResult> {
  const entries = await readdir(root, { withFileTypes: true })
  const dirs: string[] = []
  let truncated = false
  for (const e of entries) {
    if (e.name.startsWith('.')) continue
    // 심볼릭 링크(정션)는 따라가 폴더인지 본다 — 단 원격 UNC 를 가리키면 따라가지 않고 건너뛴다
    const dir =
      e.isDirectory() ||
      (e.isSymbolicLink() && linksStayLocal(join(root, e.name), isRemote) && (await stat(join(root, e.name)).then((s) => s.isDirectory(), () => false)))
    if (!dir) continue
    if (dirs.length >= MAX_DIRS) {
      truncated = true
      break
    }
    dirs.push(e.name)
  }
  const flags = await mapLimit(dirs, PROBE_CONCURRENCY, (d) => exists(join(root, d, '.git'), isRemote))
  const repos = dirs
    .filter((_, i) => flags[i])
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({ name, path: join(root, name) }))
  return { path: root, isRepo: false, repos, truncated }
}

/** SSH: 원격 폴더 목록(SFTP)에서 저장소 표시가 된 하위 폴더를 고른다 */
export function scanFromListing(listing: RemoteDirListing): RepoScanResult {
  return {
    path: listing.path,
    isRepo: listing.isRepo,
    repos: listing.isRepo
      ? []
      : listing.dirs.filter((d) => d.isRepo).map((d) => ({ name: d.name, path: posix.join(listing.path, d.name) })),
    // 목록이 잘렸거나, 앞쪽 일부만 저장소 여부를 확인했다(listRemoteDirs 의 확인 상한)
    truncated: listing.truncated || listing.dirs.some((d) => d.isRepo === undefined)
  }
}
