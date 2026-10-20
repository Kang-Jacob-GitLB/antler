// 저장소 추가 창의 원격 폴더 탐색: SFTP 로 디렉터리 목록만 읽는다(읽기 전용, 셸 명령을 보내지 않는다).
import { posix } from 'node:path'
import type { Client, SFTPWrapper, Stats } from 'ssh2'
import type { RemoteDirListing } from '@shared/types'

/** 하위 폴더마다 .git 을 확인하는 상한(그 뒤 항목은 저장소 여부를 표시하지 않는다) */
const REPO_PROBE_LIMIT = 200
/** 목록 상한 — 아주 큰 폴더(예: node_modules)에서 창이 굳지 않게 */
const MAX_ENTRIES = 2000

function call<T>(fn: (cb: (err: Error | null | undefined, v: T) => void) => void): Promise<T> {
  return new Promise<T>((resolve, reject) => fn((err, v) => (err ? reject(err) : resolve(v))))
}

/** '~', '~/x', '' 를 SFTP 기준(시작 디렉터리 = 홈)으로 바꾼다 */
function toSftpPath(p: string): string {
  const s = p.trim()
  if (!s || s === '~') return '.'
  if (s.startsWith('~/')) return `.${s.slice(1)}`
  return s
}

/** 서버가 SFTP 를 막았다 — 폴더 탐색 대신 경로를 직접 입력해야 한다 */
export class SftpUnavailableError extends Error {}

export async function listRemoteDirs(client: Client, path: string): Promise<RemoteDirListing> {
  const sftp = await call<SFTPWrapper>((cb) => client.sftp(cb)).catch((err: Error) => {
    throw new SftpUnavailableError(`이 서버는 SFTP 를 쓸 수 없어 폴더를 둘러볼 수 없습니다. 경로를 직접 입력하세요 (${err.message})`)
  })
  try {
    const base = await call<string>((cb) => sftp.realpath(toSftpPath(path), cb))
    const entries = await call<{ filename: string; attrs: Stats }[]>((cb) => sftp.readdir(base, cb))
    const isRepo = entries.some((e) => e.filename === '.git')
    const dirs: { name: string; isRepo?: boolean }[] = []
    for (const e of entries) {
      if (dirs.length >= MAX_ENTRIES) break
      if (e.filename === '.' || e.filename === '..' || e.filename.startsWith('.')) continue
      let dir = e.attrs.isDirectory()
      // 심볼릭 링크는 따라가 폴더인지 본다
      if (!dir && e.attrs.isSymbolicLink()) {
        dir = await call<Stats>((cb) => sftp.stat(posix.join(base, e.filename), cb))
          .then((s) => s.isDirectory())
          .catch(() => false)
      }
      if (dir) dirs.push({ name: e.filename })
    }
    dirs.sort((a, b) => a.name.localeCompare(b.name))
    await Promise.all(
      dirs.slice(0, REPO_PROBE_LIMIT).map(async (d) => {
        d.isRepo = await call<Stats>((cb) => sftp.stat(posix.join(base, d.name, '.git'), cb))
          .then(() => true)
          .catch(() => false)
      })
    )
    return {
      path: base,
      parent: base === '/' ? null : posix.dirname(base),
      isRepo,
      dirs,
      truncated: entries.length > MAX_ENTRIES
    }
  } finally {
    sftp.end()
  }
}
