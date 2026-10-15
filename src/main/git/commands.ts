// git 조회 명령 모음. executor 위에서 동작하며 환경과 무관하다.
import type { HeadInfo } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { LOG_FORMAT_ARGS } from './parsers/log'

async function ok(exec: GitExecutor, repoPath: string, args: string[], opts?: RunOpts): Promise<Buffer> {
  const r = await exec.run(repoPath, args, opts)
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git ${args[0]} failed (${r.code})`, r.code, r.stderr, args)
  return r.stdout
}

/** 커밋이 없는 새 저장소에서 HEAD 를 해석하지 못해 나는 에러인지 */
export function isEmptyRepoError(stderr: string): boolean {
  return /does not have any commits|bad default revision|bad revision 'HEAD'|ambiguous argument 'HEAD'/i.test(stderr)
}

/** 경로가 git 작업 트리인지 확인하고 최상위 경로를 돌려준다. */
export async function resolveTopLevel(exec: GitExecutor, path: string): Promise<string> {
  const out = await ok(exec, path, ['rev-parse', '--show-toplevel'])
  return out.toString('utf8').trim()
}

export async function head(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<HeadInfo> {
  const [branch, hash] = await Promise.all([
    exec.run(repoPath, ['symbolic-ref', '--quiet', '--short', 'HEAD'], opts),
    exec.run(repoPath, ['rev-parse', '--verify', '--quiet', 'HEAD'], opts)
  ])
  return {
    branch: branch.code === 0 ? branch.stdout.toString('utf8').trim() : null,
    hash: hash.code === 0 ? hash.stdout.toString('utf8').trim() : null
  }
}

/**
 * 커밋 목록용 git log 인자. `--date-order` 는 자식이 항상 부모보다 먼저 나오도록 보장해
 * 레인 계산(M2)이 위에서 아래로 한 번에 끝난다.
 */
export function logArgs(ref: string | undefined, skip: number): string[] {
  // renderer 가 보낸 ref 가 옵션으로 해석되지 않도록 --end-of-options 뒤에 둔다
  const revs = ref ? ['--end-of-options', ref] : ['--branches', '--remotes', '--tags', 'HEAD']
  return ['log', ...LOG_FORMAT_ARGS, '--date-order', ...(skip > 0 ? [`--skip=${skip}`] : []), ...revs, '--']
}
