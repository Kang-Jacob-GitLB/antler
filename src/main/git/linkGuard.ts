// 작업 트리 경로가 symlink·정션을 지나 저장소 밖(또는 .git 안)으로 새는지 로컬 파일 시스템에서 확인한다.
import { lstatSync, readlinkSync, realpath as realpathCb } from 'node:fs'
import { lstat } from 'node:fs/promises'
import { dirname, isAbsolute, join, parse, relative, resolve, sep } from 'node:path'
import { promisify } from 'node:util'
import { isWslUnc } from '../executors/wslPaths'

/** fs/promises 의 realpath 에는 native 판이 없다 — 8.3 짧은 이름을 긴 이름으로 펴는 건 native 판뿐이다 */
const realpathNative = promisify(realpathCb.native)

/** 링크 사슬(링크 → 링크 → …)과 한 경로 안의 링크 수를 합쳐 따라가 보는 상한 */
export const MAX_LINK_HOPS = 16

/**
 * 링크(심볼릭 링크·정션) 대상 문자열이 원격(SMB·WebDAV)을 가리키는가 — 순수 판정이다.
 * **허용 목록**이다 — 로컬로 알려진 모양만 로컬이고 나머지는 전부 원격으로 본다(차단 목록은 `\??\GLOBALROOT\Device\Mup\…` 같은 우회를 못 막는다).
 * 로컬: 드라이브 경로(`C:\`·`C:/`·`\\?\C:\`·`\??\C:\` — 정션의 일반 대상)·상대 경로·단일 `/` 시작 경로, WSL 공유(`\\wsl.localhost\…`·`\\wsl$\…`).
 * 원격: 그 밖의 `\\…`·`//…`(`\\host\share`·WebDAV `\\host@SSL\…`·장치 `\\.\…`)와 `\??\`·`\\?\` 뒤가 드라이브가 아닌 모든 것
 * (`UNC\`·`Volume{`·`GLOBALROOT\…`·`Device\…`).
 * Rust 판(`repos/scan.rs` 의 `is_remote_link_target`)과 같아야 한다 — 같은 사례표(tests/fixtures/repos/link-target.cases.json)로 둘 다 시험한다.
 */
export function isRemoteLinkTarget(target: string): boolean {
  const t = target.trim()
  // NT 네임스페이스(`\??\`)·확장 경로(`\\?\`): 뒤가 드라이브 문자 경로일 때만 로컬이다
  const ns = NT_PREFIX.exec(t)
  if (ns) return !/^[A-Za-z]:(?:[\\/]|$)/.test(t.slice(ns[0].length))
  if (!/^[\\/]{2}/.test(t)) return false
  return !isWslUnc(t)
}

const NT_PREFIX = /^(?:[\\/]\?\?[\\/]|[\\/]{2}\?[\\/])/

/** readlink 가 돌려준 NT 접두(`\??\C:\x`·`\\?\C:\x`)를 벗겨 일반 경로로 만든다(드라이브가 아닌 대상은 이미 원격으로 판정돼 여기 오지 않는다) */
const stripNtPrefix = (t: string): string => t.replace(NT_PREFIX, '')

/**
 * `path` 의 구성 요소를 루트부터 걸으며 lstat 으로 링크를 찾고, 링크마다 readlink 대상이 원격이 아닌지 본다(링크 대상에는 접속하지 않는다).
 * 로컬 대상은 그 경로도 같은 방식으로 이어서 확인하고, 링크를 합쳐 16개를 넘으면 거부한다. 원격이거나 읽을 수 없으면 false.
 * 없는 구성 요소(ENOENT)를 만나면 그 뒤는 없으니 true.
 * **realpath·stat·열기처럼 링크를 따라가는 호출 앞에서** 불러야 한다 — Windows 는 원격 UNC 를 따라가는 순간 SMB/WebDAV 인증을 시도해
 * NTLM 해시가 공격자 서버로 샌다(이후 "저장소 밖"으로 거부해도 늦다).
 */
export function linksStayLocal(path: string, isRemote: (target: string) => boolean = isRemoteLinkTarget): boolean {
  return walkLinks(path, isRemote, { hops: 0 })
}

function walkLinks(path: string, isRemote: (target: string) => boolean, budget: { hops: number }): boolean {
  const abs = resolve(path)
  const { root } = parse(abs)
  let cur = root
  for (const part of abs.slice(root.length).split(sep)) {
    if (part === '') continue
    cur = join(cur, part)
    try {
      if (!lstatSync(cur).isSymbolicLink()) continue
    } catch (err) {
      return isNotFound(err)
    }
    if (++budget.hops > MAX_LINK_HOPS) return false
    let target: string
    try {
      target = readlinkSync(cur)
    } catch {
      return false
    }
    if (isRemote(target)) return false
    if (!walkLinks(resolve(dirname(cur), stripNtPrefix(target)), isRemote, budget)) return false
  }
  return true
}

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
  // 아래 lstat 은 중간 구성 요소의 링크를 따라간다 — 저장소 경로 자체가 원격을 가리키면 그 전에 거부한다(realpath 전 확인)
  if (!linksStayLocal(repoPath)) bad()
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
export async function assertInsideRoot(
  repoPath: string,
  target: string,
  isRemote: (target: string) => boolean = isRemoteLinkTarget
): Promise<void> {
  // realpath 는 링크를 따라간다 — 원격 UNC 를 가리키는 링크가 있으면 접속(인증)하기 전에 거부한다
  if (!linksStayLocal(repoPath, isRemote) || !linksStayLocal(target, isRemote)) throw new Error('invalid path')
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
