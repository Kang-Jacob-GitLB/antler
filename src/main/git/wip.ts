// 작업 트리(WIP) 조회: 상태와 파일 diff. 읽기 전용 — 스테이지·폐기·커밋 같은 쓰기 명령은 쓰지 않는다.
import type { FileDiff, WipDiffRequest, WipStatus } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { DIFF_FULL_MAX_BYTES, DIFF_MAX_BYTES } from './commands'
import { assertNoLinkEscape } from './linkGuard'
import { DiffParser } from './parsers/diff'
import { WIP_STATUS_ARGS, WipStatusParser } from './parsers/wipStatus'

const AREAS = new Set(['staged', 'unstaged', 'untracked'])

/** 덮어쓸 수 없는 필터 드라이버가 있어 작업 트리 조회를 거부할 때의 메시지 */
export const UNSAFE_FILTER_MSG =
  '이 저장소의 filter 드라이버(이름에 "=" 나 잘못된 UTF-8 이 있거나 개수가 너무 많음)를 안전하게 무력화할 수 없습니다. 작업 트리 변경은 표시하지 않습니다.'

/**
 * 드라이버 수 상한. 드라이버마다 `-c` 4쌍(약 90자 + 이름 길이 ×4)이 붙는데 Windows 명령줄은 32767자까지라
 * 256개면 이름이 짧아도 한계에 가깝다. 정상 저장소의 드라이버는 한두 개다. 총 길이 상한(MAX_OVERRIDE_CHARS)도 함께 둔다.
 */
const MAX_FILTER_DRIVERS = 256
const MAX_OVERRIDE_CHARS = 24000

const unsafeFilter = (): GitError => new GitError(UNSAFE_FILTER_MSG, 1, UNSAFE_FILTER_MSG, ['config'])

/**
 * `git config -z --get-regexp '^filter\.'` 출력(바이트)에서 덮어써야 할 드라이버 이름을 뽑는다.
 * 키를 바이트 단위로 UTF-8 검증한다: 잘못된 바이트(`\xff`)가 든 이름은 문자열로 바꾸면 U+FFFD 가 돼 `-c` 가 실제 드라이버를 덮지 못하고,
 * Windows 는 argv 가 UTF-16 이라 원래 바이트로 `-c` 를 만들 수도 없다 — 그런 이름은 무력화할 수 없으니 던진다.
 * 접미사(.clean/.smudge/.process) 판정은 git 정규식(로캘 의존)이 아니라 여기서 한다.
 */
export function parseFilterDrivers(stdout: Buffer): string[] {
  const strict = new TextDecoder('utf-8', { fatal: true })
  const drivers = new Set<string>()
  let start = 0
  while (start < stdout.length) {
    let end = stdout.indexOf(0, start)
    if (end < 0) end = stdout.length
    const entry = stdout.subarray(start, end)
    start = end + 1
    if (entry.length === 0) continue
    const nl = entry.indexOf(0x0a)
    const keyBytes = nl < 0 ? entry : entry.subarray(0, nl)
    let key: string
    try {
      key = strict.decode(keyBytes)
    } catch {
      throw unsafeFilter()
    }
    if (key.includes('�')) throw unsafeFilter()
    const m = /^filter\.(.+)\.(clean|smudge|process)$/s.exec(key)
    if (!m) continue
    const value = nl < 0 ? '' : entry.subarray(nl + 1).toString('utf8')
    if (SAFE_FILTER_CMDS.has(value.trim())) continue
    const d = m[1]!
    // git 은 `-c` 인자를 첫 '=' 에서 키와 값으로 자른다. 이름에 '=' 가 든 드라이버(gitattributes 에서 `filter=a=b` 로 쓸 수 있다)는
    // `-c filter.a=b.clean=` 이 `filter.a` = `b.clean=` 으로 읽혀 덮어쓰기가 조용히 실패한다 — 그런 저장소는 작업 트리를 읽지 않는다
    if (d.includes('=')) throw unsafeFilter()
    drivers.add(d)
    if (drivers.size > MAX_FILTER_DRIVERS) throw unsafeFilter()
  }
  return [...drivers]
}

/** 8.3 짧은 이름 모양(`GIT~1`) 구성 요소인가. NTFS 는 끝의 `.`·공백을 떼므로 `GIT~1.`·`GIT~1 ` 도 같은 이름이다 */
export function hasShortNameSegment(p: string): boolean {
  return p.split(/[\\/]/).some((seg) => /~\d+$/.test(seg.replace(/[. ]+$/, '')))
}

/** 그대로 실행을 허용하는 필터 명령(git-lfs 기본 설치값과 정확히 같을 때만) */
const SAFE_FILTER_CMDS = new Set(['git-lfs clean -- %f', 'git-lfs smudge -- %f', 'git-lfs smudge --skip -- %f', 'git-lfs filter-process', 'git-lfs filter-process --skip'])

/**
 * 작업 트리를 읽는 status·diff 는 .gitattributes 의 filter 드라이버(clean/process)를 실행한다. 저장소 설정
 * (.git/config·include)에 심은 명령이면 저장소를 열기만 해도 임의 명령이 돈다(악성 저장소 방어 — core.fsmonitor 와 같은 부류).
 * git-lfs 기본 명령이 아닌 드라이버는 이번 실행에서만 빈 명령으로 덮어 쓴다(빈 명령 = 필터 없음, 원본 내용 그대로).
 * 그 파일은 "수정됨"으로 보일 수 있지만 명령을 실행하는 것보다 낫다.
 */
export async function filterOverrides(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<string[]> {
  // 정규식은 git 의 로캘·바이트 처리에 기대지 않도록 `^filter\.` 로만 좁히고 접미사는 parseFilterDrivers 가 판정한다
  const r = await exec.run(repoPath, ['config', '-z', '--get-regexp', '^filter\\.'], opts)
  // 1 = 해당 키 없음
  if (r.code === 1) return []
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git config exited with ${r.code}`, r.code, r.stderr, ['config'])
  return filterOverrideArgs(r.stdout)
}

/** `git config -z --get-regexp '^filter\.'` 의 stdout 을 덮어쓰기 `-c` 인자로 바꾼다(순수 함수 — 골든이 이것을 부른다). 덮을 수 없으면 던진다 */
export function filterOverrideArgs(stdout: Buffer): string[] {
  const out: string[] = []
  for (const d of parseFilterDrivers(stdout)) {
    // required=true 면 빈 명령을 "필터 없음 → 실패"로 본다. 이번 실행에서만 끈다
    out.push('-c', `filter.${d}.clean=`, '-c', `filter.${d}.smudge=`, '-c', `filter.${d}.process=`, '-c', `filter.${d}.required=false`)
  }
  if (out.reduce((n, a) => n + a.length + 1, 0) > MAX_OVERRIDE_CHARS) throw unsafeFilter()
  return out
}

/**
 * 작업 트리 상태. 출력은 스트림으로 읽어 추적 안 되는 파일을 상한까지만 담는다(수십만 개여도 메모리는 일정).
 * 저장소에 커밋이 없어도 동작한다(전부 스테이지됨/추적 안 됨으로 나온다).
 */
export async function wipStatus(exec: GitExecutor, repoPath: string, opts?: RunOpts, untrackedLimit?: number): Promise<WipStatus> {
  const parser = new WipStatusParser(untrackedLimit)
  const guard = await filterOverrides(exec, repoPath, opts)
  for await (const chunk of exec.stream(repoPath, [...guard, ...WIP_STATUS_ARGS], opts)) parser.push(chunk)
  return parser.finish()
}

/**
 * renderer 가 보낸 저장소 상대 경로 검증. 절대 경로·상위 경로(..)·.git 안쪽은 거부한다 —
 * 추적 안 되는 파일 diff 는 `--no-index` 로 파일을 직접 읽으므로 저장소 밖 파일을 읽는 통로가 되면 안 된다.
 */
export function assertRelPath(p: unknown): asserts p is string {
  if (typeof p !== 'string' || p === '' || p.length > 4096 || p.includes('\0')) throw new Error('invalid path')
  if (/^([\\/]|[a-zA-Z]:)/.test(p)) throw new Error('invalid path')
  for (const seg of p.split(/[\\/]/)) {
    if (seg === '..' || seg.toLowerCase() === '.git') throw new Error('invalid path')
  }
}

/**
 * 작업 트리 파일 하나의 diff. FileDiff 형식은 커밋 diff 와 같아 같은 뷰어가 그린다.
 * untrackedAllow: 마지막 wipStatus 가 git 출력으로 보고한 추적 안 되는 경로 집합. area 가 untracked 면 **정확히 일치**하는 경로만 받는다
 * (없으면 거부 — 닫힌 기본값). assertRelPath 의 문자열 비교는 Windows NTFS 8.3 짧은 이름(`GIT~1/config`)을 `.git` 으로 알아보지 못해
 * `--no-index` 가 .git/config 를 읽게 되므로, 이름 모양을 막는 대신 git 이 실제로 보고한 경로만 허용한다.
 * -uall 이라 폴더는 파일 단위로 나오고, 폴더 항목(`dir/`)은 중첩 저장소뿐이라 접두사 허용은 필요 없다(하위 구성 요소 우회를 다시 열지 않는다).
 */
export async function wipFileDiff(
  exec: GitExecutor,
  repoPath: string,
  req: WipDiffRequest,
  opts?: RunOpts,
  untrackedAllow?: ReadonlySet<string>
): Promise<FileDiff> {
  if (!req || !AREAS.has(req.area)) throw new Error('invalid area')
  assertRelPath(req.path)
  if (req.oldPath !== undefined) assertRelPath(req.oldPath)
  if (req.area === 'untracked' && !untrackedAllow?.has(req.path)) throw new Error('invalid path')

  const parser = new DiffParser(req.path, req.oldPath, req.full ? { maxLines: Infinity, maxBytes: DIFF_FULL_MAX_BYTES } : { maxBytes: DIFF_MAX_BYTES })
  // 추적 안 되는 폴더(중첩 저장소)는 파일이 아니라 내용을 비교할 수 없다
  if (req.area === 'untracked' && /[\\/]$/.test(req.path)) return parser.finish()

  if (req.area !== 'staged') {
    // unstaged: 조작된 index 의 `GIT~1/config` 항목이 작업 트리 diff 로 .git/config 를 읽게 하는 8.3 짧은 이름을 구성 요소 모양(`~숫자`)으로 거부한다.
    // 정상 파일 이름에도 드물게 있는 형태(`foo~1`)라 오탐을 허용한다 — .git 안을 읽는 쪽이 더 나쁘다. (untracked 는 허용 목록이 이미 막는다)
    if (req.area === 'unstaged' && hasShortNameSegment(req.path)) throw new Error('invalid path')
    // 정션·symlink 로 .git 이나 저장소 밖을 읽는 우회: 로컬 파일 시스템일 때만 열기 직전에 확인한다.
    // WSL·SSH 는 Node 가 파일 시스템을 볼 수 없어 하지 않는다 — Linux git 의 status 는 디렉터리 symlink 를 따라 들어가지 않고 링크 자체를
    // 하나의 항목으로 보고하므로 `j/config` 형태의 경로가 허용 목록에 들어오지 않는다고 본다(미확인: 실측하지 않았다. docs/security.md 참고).
    if (exec.kind === 'local') await assertNoLinkEscape(repoPath, req.path, req.area === 'unstaged')
  }

  // --no-index 는 저장소 속성을 읽지 않지만, 작업 트리·인덱스 diff 는 clean 필터를 돈다
  const guard = req.area === 'untracked' ? [] : await filterOverrides(exec, repoPath, opts)
  const common = [...guard, 'diff', '--no-color', '--no-ext-diff', '--no-textconv', '-U3']
  const paths = req.oldPath !== undefined && req.oldPath !== req.path ? [req.oldPath, req.path] : [req.path]
  const args =
    req.area === 'staged'
      ? [...common, '--cached', '-M', '--', ...paths]
      : req.area === 'unstaged'
        ? [...common, '--no-renames', '--', req.path]
        : // 추적 안 되는 파일은 빈 파일과의 비교로 "전부 추가" diff 를 만든다. /dev/null 은 git 이 모든 OS 에서 직접 해석한다
          [...common, '--no-index', '--', '/dev/null', req.path]
  let produced = false
  try {
    for await (const chunk of exec.stream(repoPath, args, opts)) {
      if (chunk.length > 0) produced = true
      // 상한을 넘기고 줄 수 세기도 끝났으면 git 을 끊는다(break 하면 stream 이 프로세스를 정리한다)
      if (!parser.push(chunk)) break
    }
  } catch (err) {
    // --no-index 는 차이가 있으면 종료 코드 1 로 끝난다 — 오류가 아니다. 단 출력이 하나도 없으면 차이가 아니라
    // 파일을 못 연 것("Could not access")이라 빈 diff 로 가리지 않고 오류로 올린다(정상 diff 는 rc 1 + 출력 있음, 빈 파일은 rc 0)
    if (!(req.area === 'untracked' && err instanceof GitError && err.code === 1 && produced)) throw err
  }
  return parser.finish()
}
