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
  const skip = Number.isInteger(req.skip) && req.skip > 0 ? req.skip : 0
  // 상한을 넘는 쪽은 더 읽지 않는다(hasMore=false 라 renderer 가 다시 요청하지 않는다)
  if (skip >= FILE_HISTORY_MAX_SKIP) return { entries: [], hasMore: false }
  const count = Math.min(FILE_HISTORY_MAX_COUNT, Math.max(1, Number.isInteger(req.count) ? req.count : 100))
  // rev 를 생략하면 HEAD 를 지금 해시로 풀어 돌려준다 — renderer 가 이후 페이지에 이 해시를 써서 브랜치가 바뀌어도 이력이 섞이지 않는다
  let rev = req.rev
  if (rev === undefined) {
    const h = await exec.run(repoPath, ['rev-parse', '--verify', '--quiet', 'HEAD'], opts)
    rev = h.code === 0 ? h.stdout.toString('utf8').trim() : ''
    // 커밋이 아직 없는 저장소는 이력이 없는 것과 같다
    if (!rev) return { entries: [], hasMore: false }
  }
  const args = [
    'log',
    '--follow',
    '--no-color',
    '--no-ext-diff',
    ...FILE_HISTORY_FORMAT_ARGS,
    // --follow 와 함께 쓰면 --skip 이 무시된다(이름 변경을 따라가는 필터가 skip 뒤에 적용되지 않음). 앞쪽까지 읽고 직접 자른다
    `--max-count=${skip + count + 1}`,
    ...revArgs(rev),
    '--',
    req.path
  ]
  const r = await exec.run(repoPath, args, opts)
  // 커밋이 아직 없는 저장소는 이력이 없는 것과 같다
  if (r.code !== 0 && isEmptyRepoError(r.stderr)) return { entries: [], hasMore: false }
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git log failed (${r.code})`, r.code, r.stderr, args)
  const all = parseFileHistory(r.stdout, req.path)
  const entries = all.slice(skip, skip + count)
  // 이번에 새 항목이 하나도 없으면 더 받을 게 없다. skip 상한에 닿는 페이지도 마지막으로 친다
  const hasMore = entries.length > 0 && all.length > skip + count && skip + count < FILE_HISTORY_MAX_SKIP
  return { entries, hasMore, rev }
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
  const empty = { rev, path: req.path, size: 0, commits: {}, groups: [], lines: [] }
  // 그 시점에 경로가 없거나(작업 트리에서 새로 add 한 파일 등) 파일이 아닌 객체(서브모듈)면 blame 할 게 없다
  if (sz.code !== 0) return { ...empty, binary: false, tooLarge: false, missing: true }
  const size = Number(sz.stdout.toString('utf8').trim()) || 0
  if (size > BLAME_MAX_BYTES) return { ...empty, size, binary: false, tooLarge: true, missing: false }

  // --no-textconv: 저장소 설정의 diff.<이름>.textconv 명령을 실행하지 않는다(드라이버 이름을 열거해 덮어쓰는 방식은 이름에 '=' 가 있으면 뚫린다)
  const base = ['blame', '--no-textconv', '--porcelain', rev, '--', req.path]
  const run = (args: string[]) => exec.run(repoPath, args, { ...opts, maxBuffer: BLAME_MAX_BUFFER })
  let r = await run(base)
  // blame.ignoreRevsFile 이 없는 파일을 가리키면 git 이 blame 자체를 거부한다 — 이번 실행에서만 설정된 목록을 비운다
  // (-c blame.ignoreRevsFile= 는 빈 항목을 더할 뿐 비우지 못해서 --no-ignore-revs-file 을 쓴다)
  if (r.code !== 0 && /could not open object name list/i.test(r.stderr)) r = await run(['blame', '--no-ignore-revs-file', ...base.slice(1)])
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git blame failed (${r.code})`, r.code, r.stderr, base)
  const parsed = parseBlame(r.stdout)
  if (parsed.binary) return { ...empty, size, binary: true, tooLarge: false, missing: false }
  return { rev, path: req.path, binary: false, tooLarge: false, missing: false, size, commits: parsed.commits, groups: parsed.groups, lines: parsed.lines }
}
