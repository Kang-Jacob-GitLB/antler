// Git LFS · Gitflow 표시용 읽기 전용 조회. git-lfs 바이너리나 저장소 설정의 명령은 실행하지 않는다:
// LFS 여부는 속성(`git check-attr`, 속성만 읽는다)과 .gitattributes 텍스트로, gitflow 는 `git config` 값으로만 판단한다.
import type { GitflowConfig, LfsPathsRequest, RepoExtras } from '@shared/types'
import { addedLines, gitattributesUsesLfs, parseCheckAttrLfs } from '@shared/lfs'
import { parseGitflowConfig } from '@shared/gitflow'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { assertHex } from './commands'

/** 한 번에 물어볼 수 있는 경로 수 상한(넘는 경로는 표시하지 않는다) */
export const LFS_PATHS_MAX = 1000
/** check-attr 한 번에 넘기는 인자 길이 합계(명령줄 길이 제한 안쪽) */
const CHUNK_CHARS = 6000
/** 속성 파일 하나를 읽는 상한. 이보다 크면 LFS 규칙을 찾지 않는다 */
const ATTRIBUTES_MAX_BYTES = 256 * 1024
/** HEAD 트리에서 살펴보는 .gitattributes 파일 수 상한 */
const ATTRIBUTES_MAX_FILES = 20
/** HEAD 트리 목록 출력 상한 */
const TREE_LIST_MAX_BYTES = 16 * 1024 * 1024
const CONFIG_MAX_BYTES = 256 * 1024
/** check-attr 가 --source 를 모르는 옛 git 의 에러 */
const OLD_GIT_RE = /unknown option|unrecognized option|usage: git check-attr/i

/** 저장소 단위 부가 정보. 둘 다 부가 표시라 실패하면 없는 것으로 본다 */
export async function repoExtras(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<RepoExtras> {
  const [lfs, cfg] = await Promise.all([usesLfs(exec, repoPath, opts), gitflowConfig(exec, repoPath, opts)])
  return { lfs, gitflow: cfg }
}

/**
 * 저장소가 LFS 를 쓰는지: HEAD 트리의 모든 .gitattributes, 작업 트리 루트 .gitattributes, .git/info/attributes 중
 * 하나라도 filter=lfs 규칙이 있으면 true. 하나가 읽히지 않아도 나머지로 판단한다.
 */
async function usesLfs(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<boolean> {
  const checks = [
    committedAttributesUseLfs(exec, repoPath, opts),
    fileUsesLfs(exec, repoPath, '.gitattributes', opts),
    infoAttributesUseLfs(exec, repoPath, opts)
  ]
  return (await Promise.all(checks.map((c) => c.catch(() => false)))).some(Boolean)
}

async function committedAttributesUseLfs(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<boolean> {
  // 커밋이 없는 저장소는 실패 코드로 끝난다 — LFS 를 쓰지 않는 것과 같다
  const ls = await exec.run(repoPath, ['ls-tree', '-r', '-z', '--name-only', '--full-tree', 'HEAD'], { ...opts, maxBuffer: TREE_LIST_MAX_BYTES })
  if (ls.code !== 0) return false
  const names = ls.stdout
    .toString('utf8')
    .split('\0')
    .filter((n) => n === '.gitattributes' || n.endsWith('/.gitattributes'))
    .slice(0, ATTRIBUTES_MAX_FILES)
  const found = await Promise.all(
    names.map(async (n) => {
      const r = await exec.run(repoPath, ['cat-file', 'blob', `HEAD:${n}`], { ...opts, maxBuffer: ATTRIBUTES_MAX_BYTES }).catch(() => null)
      return !!r && r.code === 0 && gitattributesUsesLfs(r.stdout.toString('utf8'))
    })
  )
  return found.some(Boolean)
}

async function infoAttributesUseLfs(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<boolean> {
  const r = await exec.run(repoPath, ['rev-parse', '--path-format=absolute', '--git-path', 'info/attributes'], opts)
  const path = r.code === 0 ? (r.stdout.toString('utf8').split('\n')[0]?.replace(/\r$/, '') ?? '') : ''
  return path ? fileUsesLfs(exec, repoPath, path, opts) : false
}

/**
 * 작업 트리·git-dir 의 파일 내용을 읽는다. executor 로는 git 만 실행할 수 있어 `git diff --no-index /dev/null <file>` 의
 * 추가 줄로 읽는다(외부 diff·textconv 는 끈다). 파일이 없거나 상한을 넘으면 false.
 */
async function fileUsesLfs(exec: GitExecutor, repoPath: string, path: string, opts?: RunOpts): Promise<boolean> {
  const args = ['diff', '--no-index', '--no-ext-diff', '--no-textconv', '--no-color', '--', '/dev/null', path]
  const r = await exec.run(repoPath, args, { ...opts, maxBuffer: ATTRIBUTES_MAX_BYTES * 2 })
  // --no-index 는 차이가 있으면 종료 코드 1 이다
  if (r.code !== 1) return false
  return gitattributesUsesLfs(addedLines(r.stdout.toString('utf8')))
}

/**
 * `git config --local --get-regexp ^gitflow\.` — 값은 표시용 데이터로만 파싱한다.
 * 저장소 설정만 읽어 전역·시스템 설정이 섞이지 않게 한다. 설정이 없으면(종료 코드 1) null
 */
async function gitflowConfig(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<GitflowConfig | null> {
  const args = ['config', '--local', '--null', '--get-regexp', '^gitflow\\.']
  const r = await exec.run(repoPath, args, { ...opts, maxBuffer: CONFIG_MAX_BYTES }).catch(() => null)
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
    if (r.code !== 0 && useSource && OLD_GIT_RE.test(r.stderr)) {
      // --source 를 모르는 옛 git(그 외 실패는 폴백하지 않고 그대로 던진다). 이후 묶음도 같은 방식으로 처리한다
      useSource = false
      r = await run(false)
    }
    if (r.code !== 0) throw new GitError(r.stderr.trim() || `git check-attr failed (${r.code})`, r.code, r.stderr, ['check-attr'])
    found.push(...parseCheckAttrLfs(r.stdout.toString('utf8')))
  }
  return found
}
