// 작업 트리 경로가 symlink·정션을 지나 저장소 밖(또는 .git 안)으로 새는지 로컬 파일 시스템에서 확인한다.
import { realpath as realpathCb } from 'node:fs'
import { lstat } from 'node:fs/promises'
import { isAbsolute, join, relative, sep } from 'node:path'
import { promisify } from 'node:util'

/** fs/promises 의 realpath 에는 native 판이 없다 — 8.3 짧은 이름을 긴 이름으로 펴는 건 native 판뿐이다 */
const realpathNative = promisify(realpathCb.native)

/**
 * 저장소 루트 아래 `rel` 이 실제로 저장소 안의 일반 경로인지 확인한다. 아니면 `Error('invalid path')`.
 * Git for Windows 의 status -uall 은 정션(`mklink /J j .git`)을 디렉터리처럼 따라 들어가 `j/config` 를 보고하고,
 * `diff --no-index`·작업 트리 diff 는 그 경로로 `.git/config` 나 저장소 밖 파일을 읽는다 — 그래서 열기 직전에 확인한다.
 *
 * 1) 루트부터 구성 요소마다 lstat: symlink(Node 는 Windows 정션도 isSymbolicLink 로 보고한다)면 거부.
 *    마지막 구성 요소는 allowLeafLink 일 때만 허용한다(추적 중인 symlink 는 git 이 링크 대상 문자열만 비교해 내용을 읽지 않는다).
 * 2) realpath.native 결과(8.3 이름이 긴 이름으로 펴진다)가 저장소 루트 실제 경로 안이고 `.git` 구성 요소가 없어야 한다.
 * 없는 경로(ENOENT)는 여기서 막지 않는다 — 읽을 파일이 없으니 git 이 스스로 실패하거나 삭제로 보여 준다.
 * 확인과 git 의 읽기 사이의 경쟁(TOCTOU)은 막지 못한다: 저장소 안에 쓸 수 있는 공격자가 그 틈에 바꾸는 경우는 범위 밖이다.
 * WSL·SSH 실행기에서는 호출하지 않는다(Node 가 그 파일 시스템을 못 본다).
 */
export async function assertNoLinkEscape(repoPath: string, rel: string, allowLeafLink = false): Promise<void> {
  const bad = (): never => {
    throw new Error('invalid path')
  }
  const parts = rel.split(/[\\/]/).filter((p) => p !== '')
  let cur = repoPath
  let leafIsLink = false
  try {
    for (let i = 0; i < parts.length; i++) {
      cur = join(cur, parts[i]!)
      const st = await lstat(cur)
      if (st.isSymbolicLink()) {
        if (allowLeafLink && i === parts.length - 1) leafIsLink = true
        else bad()
      }
    }
  } catch (err) {
    if (isNotFound(err)) return
    if (err instanceof Error && err.message === 'invalid path') throw err
    return bad()
  }
  await assertInsideRoot(repoPath, leafIsLink ? join(...[repoPath, ...parts.slice(0, -1)]) : cur)
}

/** `target`(저장소 안이라고 주장하는 경로)의 실제 경로가 저장소 루트 실제 경로 안이고 `.git` 구성 요소가 없는지 확인한다(ENOENT 는 통과). */
export async function assertInsideRoot(repoPath: string, target: string): Promise<void> {
  try {
    const [root, real] = await Promise.all([realpathNative(repoPath), realpathNative(target)])
    const rel = relative(root, real)
    if (rel === '') return
    if (rel === '..' || rel.startsWith(`..${sep}`) || isAbsolute(rel)) throw new Error('invalid path')
    if (rel.split(/[\\/]/).some((seg) => seg.toLowerCase() === '.git')) throw new Error('invalid path')
  } catch (err) {
    if (isNotFound(err)) return
    if (err instanceof Error && err.message === 'invalid path') throw err
    throw new Error('invalid path')
  }
}

export function isNotFound(err: unknown): boolean {
  const code = (err as NodeJS.ErrnoException | undefined)?.code
  return code === 'ENOENT' || code === 'ENOTDIR'
}
