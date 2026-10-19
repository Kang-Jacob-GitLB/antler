// 작업 트리(WIP) 조회: 상태와 파일 diff. 읽기 전용 — 스테이지·폐기·커밋 같은 쓰기 명령은 쓰지 않는다.
import type { FileDiff, WipDiffRequest, WipStatus } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { DIFF_FULL_MAX_BYTES, DIFF_MAX_BYTES } from './commands'
import { DiffParser } from './parsers/diff'
import { WIP_STATUS_ARGS, WipStatusParser } from './parsers/wipStatus'

const AREAS = new Set(['staged', 'unstaged', 'untracked'])

/** 덮어쓸 수 없는 필터 드라이버가 있어 작업 트리 조회를 거부할 때의 메시지 */
export const UNSAFE_FILTER_MSG = '이 저장소의 filter 드라이버 이름에 "=" 가 있어 안전하게 무력화할 수 없습니다. 작업 트리 변경은 표시하지 않습니다.'

/** 그대로 실행을 허용하는 필터 명령(git-lfs 기본 설치값과 정확히 같을 때만) */
const SAFE_FILTER_CMDS = new Set(['git-lfs clean -- %f', 'git-lfs smudge -- %f', 'git-lfs smudge --skip -- %f', 'git-lfs filter-process', 'git-lfs filter-process --skip'])

/**
 * 작업 트리를 읽는 status·diff 는 .gitattributes 의 filter 드라이버(clean/process)를 실행한다. 저장소 설정
 * (.git/config·include)에 심은 명령이면 저장소를 열기만 해도 임의 명령이 돈다(악성 저장소 방어 — core.fsmonitor 와 같은 부류).
 * git-lfs 기본 명령이 아닌 드라이버는 이번 실행에서만 빈 명령으로 덮어 쓴다(빈 명령 = 필터 없음, 원본 내용 그대로).
 * 그 파일은 "수정됨"으로 보일 수 있지만 명령을 실행하는 것보다 낫다.
 */
export async function filterOverrides(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<string[]> {
  const r = await exec.run(repoPath, ['config', '-z', '--get-regexp', '^filter\\..+\\.(clean|smudge|process)$'], opts)
  // 1 = 해당 키 없음
  if (r.code === 1) return []
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git config exited with ${r.code}`, r.code, r.stderr, ['config'])
  const drivers = new Set<string>()
  for (const entry of r.stdout.toString('utf8').split('\0')) {
    if (!entry) continue
    const nl = entry.indexOf('\n')
    const key = nl < 0 ? entry : entry.slice(0, nl)
    const value = nl < 0 ? '' : entry.slice(nl + 1)
    const m = /^filter\.(.+)\.(clean|smudge|process)$/s.exec(key)
    if (m && !SAFE_FILTER_CMDS.has(value.trim())) drivers.add(m[1]!)
  }
  const out: string[] = []
  for (const d of drivers) {
    // git 은 `-c` 인자를 첫 '=' 에서 키와 값으로 자른다. 이름에 '=' 가 든 드라이버(gitattributes 에서 `filter=a=b` 로 쓸 수 있다)는
    // `-c filter.a=b.clean=` 이 `filter.a` = `b.clean=` 으로 읽혀 덮어쓰기가 조용히 실패한다 — 그런 저장소는 작업 트리를 읽지 않는다
    if (d.includes('=')) throw new GitError(UNSAFE_FILTER_MSG, 1, UNSAFE_FILTER_MSG, ['config'])
    // required=true 면 빈 명령을 "필터 없음 → 실패"로 본다. 이번 실행에서만 끈다
    out.push('-c', `filter.${d}.clean=`, '-c', `filter.${d}.smudge=`, '-c', `filter.${d}.process=`, '-c', `filter.${d}.required=false`)
  }
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
function assertRelPath(p: unknown): asserts p is string {
  if (typeof p !== 'string' || p === '' || p.length > 4096 || p.includes('\0')) throw new Error('invalid path')
  if (/^([\\/]|[a-zA-Z]:)/.test(p)) throw new Error('invalid path')
  for (const seg of p.split(/[\\/]/)) {
    if (seg === '..' || seg.toLowerCase() === '.git') throw new Error('invalid path')
  }
}

/** 작업 트리 파일 하나의 diff. FileDiff 형식은 커밋 diff 와 같아 같은 뷰어가 그린다 */
export async function wipFileDiff(exec: GitExecutor, repoPath: string, req: WipDiffRequest, opts?: RunOpts): Promise<FileDiff> {
  if (!req || !AREAS.has(req.area)) throw new Error('invalid area')
  assertRelPath(req.path)
  if (req.oldPath !== undefined) assertRelPath(req.oldPath)

  const parser = new DiffParser(req.path, req.oldPath, req.full ? { maxLines: Infinity, maxBytes: DIFF_FULL_MAX_BYTES } : { maxBytes: DIFF_MAX_BYTES })
  // 추적 안 되는 폴더(중첩 저장소)는 파일이 아니라 내용을 비교할 수 없다
  if (req.area === 'untracked' && /[\\/]$/.test(req.path)) return parser.finish()

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
  try {
    for await (const chunk of exec.stream(repoPath, args, opts)) {
      // 상한을 넘기고 줄 수 세기도 끝났으면 git 을 끊는다(break 하면 stream 이 프로세스를 정리한다)
      if (!parser.push(chunk)) break
    }
  } catch (err) {
    // --no-index 는 차이가 있으면 종료 코드 1 로 끝난다 — 오류가 아니다
    if (!(req.area === 'untracked' && err instanceof GitError && err.code === 1)) throw err
  }
  return parser.finish()
}
