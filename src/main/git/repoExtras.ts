// Git LFS · Gitflow 표시용 읽기 전용 조회. git-lfs 바이너리나 저장소 설정의 명령은 실행하지 않는다:
// LFS 여부는 속성(`git check-attr`, 속성만 읽는다)과 .gitattributes 텍스트로, gitflow 는 `git config` 값으로만 판단한다.
import type { GitflowConfig, LfsPathsRequest, RepoExtras } from '@shared/types'
import { gitattributesUsesLfs, parseCheckAttrLfs } from '@shared/lfs'
import { parseGitflowConfig } from '@shared/gitflow'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { assertHex } from './commands'

/** 한 번에 물어볼 수 있는 경로 수 상한(넘는 경로는 표시하지 않는다) */
export const LFS_PATHS_MAX = 1000
/** check-attr 한 번에 넘기는 인자 길이 합계(명령줄 길이 제한 안쪽) */
const CHUNK_CHARS = 6000
/** .gitattributes 를 읽는 상한. 이보다 크면 LFS 규칙을 찾지 않는다 */
const ATTRIBUTES_MAX_BYTES = 256 * 1024
const CONFIG_MAX_BYTES = 256 * 1024

/** 저장소 단위 부가 정보. 둘 다 부가 표시라 실패하면 없는 것으로 본다 */
export async function repoExtras(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<RepoExtras> {
  const [attrs, cfg] = await Promise.all([
    // 커밋이 없거나 .gitattributes 가 없으면 실패 코드로 끝난다 — LFS 를 쓰지 않는 것과 같다
    exec.run(repoPath, ['cat-file', 'blob', 'HEAD:.gitattributes'], { ...opts, maxBuffer: ATTRIBUTES_MAX_BYTES }).catch(() => null),
    gitflowConfig(exec, repoPath, opts)
  ])
  const lfs = !!attrs && attrs.code === 0 && gitattributesUsesLfs(attrs.stdout.toString('utf8'))
  return { lfs, gitflow: cfg }
}

/** `git config --get-regexp ^gitflow\.` — 값은 표시용 데이터로만 파싱한다. 설정이 없으면(종료 코드 1) null */
async function gitflowConfig(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<GitflowConfig | null> {
  const r = await exec.run(repoPath, ['config', '--null', '--get-regexp', '^gitflow\\.'], { ...opts, maxBuffer: CONFIG_MAX_BYTES }).catch(() => null)
  if (!r || r.code !== 0) return null
  return parseGitflowConfig(r.stdout.toString('utf8'))
}

/**
 * 주어진 경로 중 `filter=lfs` 속성인 것. `git check-attr` 는 속성 파일만 읽고 필터 명령을 실행하지 않는다.
 * rev 가 있으면 그 커밋 트리의 .gitattributes 기준(`--source`, git 2.40+; 더 옛 git 이면 현재 작업 트리 기준으로 대신한다).
 * 경로는 `--` 뒤에 둔다.
 */
export async function lfsPaths(exec: GitExecutor, repoPath: string, req: LfsPathsRequest, opts?: RunOpts): Promise<string[]> {
  if (req.rev !== undefined) assertHex(req.rev, 'commit hash')
  if (!Array.isArray(req.paths)) throw new Error('invalid paths')
  const paths = req.paths.slice(0, LFS_PATHS_MAX)
  for (const p of paths) if (typeof p !== 'string' || p === '' || p.includes('\0')) throw new Error('invalid path')

  const chunks: string[][] = []
  let cur: string[] = []
  let len = 0
  for (const p of paths) {
    if (cur.length > 0 && len + p.length > CHUNK_CHARS) {
      chunks.push(cur)
      cur = []
      len = 0
    }
    cur.push(p)
    len += p.length + 1
  }
  if (cur.length > 0) chunks.push(cur)

  let useSource = req.rev !== undefined
  const found: string[] = []
  for (const chunk of chunks) {
    const run = (withSource: boolean) =>
      exec.run(repoPath, ['check-attr', ...(withSource ? [`--source=${req.rev}`] : []), '-z', 'filter', '--', ...chunk], opts)
    let r = await run(useSource)
    if (r.code !== 0 && useSource) {
      // --source 를 모르는 옛 git. 이후 묶음도 같은 방식으로 건너뛴다
      useSource = false
      r = await run(false)
    }
    if (r.code !== 0) throw new GitError(r.stderr.trim() || `git check-attr failed (${r.code})`, r.code, r.stderr, ['check-attr'])
    found.push(...parseCheckAttrLfs(r.stdout.toString('utf8')))
  }
  return found
}
