// 파일 단위 조회: 히스토리(`git log --follow`)와 blame. 읽기 전용이며 환경과 무관하다(executor 위에서 동작).
import type { BlameRequest, BlameResult, FileHistoryPage, FileHistoryRequest } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { assertHex, isEmptyRepoError, revArgs } from './commands'
import { parseBlame } from './parsers/blame'
import { FILE_HISTORY_FORMAT_ARGS, parseFileHistory } from './parsers/fileHistory'

/** 한 번에 받을 수 있는 히스토리 항목 수 상한 */
export const FILE_HISTORY_MAX_COUNT = 500
/** 건너뛸 수 있는 최대 항목 수(skip 만큼 앞쪽을 다시 읽으므로 상한을 둔다) */
export const FILE_HISTORY_MAX_SKIP = 100_000
/** 이보다 큰 파일은 blame 하지 않는다(줄마다 머리를 받아 출력이 크게 부푼다) */
export const BLAME_MAX_BYTES = 512 * 1024
const BLAME_MAX_BUFFER = 128 * 1024 * 1024

function assertPath(path: unknown): asserts path is string {
  if (typeof path !== 'string' || path === '' || path.includes('\0')) throw new Error('invalid path')
}

/**
 * 파일을 바꾼 커밋 목록. rev(커밋 해시)부터 거슬러 올라가며 이름 변경을 따라간다(`--follow`는 경로 하나만 받는다).
 * skip+count+1 개를 요청해 다음 페이지가 있는지 알아낸다(앞쪽 페이지는 다시 읽지만 git 은 거기서 멈추므로 이력 전체를 훑지 않는다). 경로는 `--` 뒤에 두고 GIT_LITERAL_PATHSPECS 로 pathspec magic 을 끈다.
 */
export async function fileHistory(exec: GitExecutor, repoPath: string, req: FileHistoryRequest, opts?: RunOpts): Promise<FileHistoryPage> {
  assertPath(req.path)
  if (req.rev !== undefined) assertHex(req.rev, 'commit hash')
  const skip = Number.isInteger(req.skip) && req.skip > 0 ? Math.min(req.skip, FILE_HISTORY_MAX_SKIP) : 0
  const count = Math.min(FILE_HISTORY_MAX_COUNT, Math.max(1, Number.isInteger(req.count) ? req.count : 100))
  const args = [
    'log',
    '--follow',
    '--no-color',
    '--no-ext-diff',
    ...FILE_HISTORY_FORMAT_ARGS,
    // --follow 와 함께 쓰면 --skip 이 무시된다(이름 변경을 따라가는 필터가 skip 뒤에 적용되지 않음). 앞쪽까지 읽고 직접 자른다
    `--max-count=${skip + count + 1}`,
    ...revArgs(req.rev ?? 'HEAD'),
    '--',
    req.path
  ]
  const r = await exec.run(repoPath, args, opts)
  // 커밋이 아직 없는 저장소는 이력이 없는 것과 같다
  if (r.code !== 0 && isEmptyRepoError(r.stderr)) return { entries: [], hasMore: false }
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git log failed (${r.code})`, r.code, r.stderr, args)
  const all = parseFileHistory(r.stdout, req.path)
  return { entries: all.slice(skip, skip + count), hasMore: all.length > skip + count }
}

/**
 * blame 은 textconv 드라이버(diff.<이름>.textconv)를 실행한다. 저장소 설정에 심은 명령이면 열람만으로 임의 명령이 돌므로
 * (fileDiff 가 --no-textconv 를 쓰는 것과 같은 이유) 이번 실행에서만 `cat` 으로 덮어 쓴다 — 내용은 그대로 나온다.
 */
async function textconvOverrides(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<string[]> {
  const args = ['config', '-z', '--get-regexp', '^diff\\..+\\.textconv$']
  const r = await exec.run(repoPath, args, opts)
  // 1 = 해당 키 없음
  if (r.code === 1) return []
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git config failed (${r.code})`, r.code, r.stderr, args)
  const names = new Set<string>()
  for (const entry of r.stdout.toString('utf8').split('\0')) {
    const nl = entry.indexOf('\n')
    const m = /^diff\.(.+)\.textconv$/s.exec(nl < 0 ? entry : entry.slice(0, nl))
    if (m) names.add(m[1]!)
  }
  return [...names].flatMap((n) => ['-c', `diff.${n}.textconv=cat`, '-c', `diff.${n}.cachetextconv=false`])
}

/**
 * 지정 커밋(생략하면 HEAD) 시점 파일의 줄별 blame(`git blame --porcelain`).
 * 먼저 `cat-file -s` 로 크기를 보고 상한을 넘으면 blame 하지 않는다. 바이너리는 출력에 NUL 이 있는 것으로 판별한다.
 */
export async function fileBlame(exec: GitExecutor, repoPath: string, req: BlameRequest, opts?: RunOpts): Promise<BlameResult> {
  assertPath(req.path)
  let rev: string
  if (req.rev !== undefined) {
    assertHex(req.rev, 'commit hash')
    rev = req.rev
  } else {
    const h = await exec.run(repoPath, ['rev-parse', '--verify', '--quiet', 'HEAD'], opts)
    rev = h.code === 0 ? h.stdout.toString('utf8').trim() : ''
    if (!rev) throw new Error('no commits yet')
  }
  // rev 는 hex 만 허용하므로 `rev:path` 가 옵션으로 해석될 수 없다
  const sizeArgs = ['cat-file', '-s', `${rev}:${req.path}`]
  const sz = await exec.run(repoPath, sizeArgs, opts)
  if (sz.code !== 0) throw new GitError(sz.stderr.trim() || `file not found at ${rev.slice(0, 7)}`, sz.code, sz.stderr, sizeArgs)
  const size = Number(sz.stdout.toString('utf8').trim()) || 0
  const empty = { rev, path: req.path, size, commits: {}, groups: [], lines: [] }
  if (size > BLAME_MAX_BYTES) return { ...empty, binary: false, tooLarge: true }

  const guard = await textconvOverrides(exec, repoPath, opts)
  const args = [...guard, 'blame', '--porcelain', rev, '--', req.path]
  const r = await exec.run(repoPath, args, { ...opts, maxBuffer: BLAME_MAX_BUFFER })
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git blame failed (${r.code})`, r.code, r.stderr, args)
  const parsed = parseBlame(r.stdout)
  if (parsed.binary) return { ...empty, binary: true, tooLarge: false }
  return { rev, path: req.path, binary: false, tooLarge: false, size, commits: parsed.commits, groups: parsed.groups, lines: parsed.lines }
}
