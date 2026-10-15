// git 조회 명령 모음. executor 위에서 동작하며 환경과 무관하다.
import { createHash } from 'node:crypto'
import type { CommitDetail, FileDiff, FileDiffRequest, HeadInfo, RefsSnapshot, SearchResult } from '@shared/types'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { parseChanges } from './parsers/changes'
import { COMMIT_FORMAT_ARGS, parseCommitMeta } from './parsers/commit'
import { DiffParser } from './parsers/diff'
import { LOG_FORMAT_ARGS } from './parsers/log'
import { NulRecordSplitter } from './parsers/nulRecords'
import { parseRefs, parseRemoteNames, REFS_ARGS } from './parsers/refs'
import { parseStashes, STASH_ARGS } from './parsers/stash'
import { parseWorktrees, WORKTREE_ARGS } from './parsers/worktree'

/** 검색 결과 상한. 넘으면 truncated 로 알린다 */
export const SEARCH_LIMIT = 10_000
/** 커밋 상세의 파일 목록 상한. 넘으면 truncated 로 알린다 */
export const COMMIT_FILES_LIMIT = 5000
/** 일반 diff 요청이 읽는 입력 바이트 상한 */
export const DIFF_MAX_BYTES = 8 * 1024 * 1024
/** "전체 보기"(full) 요청의 입력 바이트 상한 */
export const DIFF_FULL_MAX_BYTES = 64 * 1024 * 1024

const HEX_RE = /^[0-9a-f]{4,64}$/i

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

/**
 * 변경 감지용 시그니처. HEAD(브랜치·해시)와 모든 ref 가 가리키는 해시를 SHA-1 로 요약한다.
 * 커밋·체크아웃·fetch·브랜치 생성/삭제·스태시는 모두 이 값을 바꾼다. 작업 트리 파일 변경은 대상이 아니다(읽기 전용 뷰).
 */
export async function refsSignature(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<string> {
  const [refsOut, headRef, headHash] = await Promise.all([
    exec.run(repoPath, ['for-each-ref', '--format=%(objectname) %(refname)'], opts),
    exec.run(repoPath, ['symbolic-ref', '--quiet', 'HEAD'], opts),
    exec.run(repoPath, ['rev-parse', '--verify', '--quiet', 'HEAD'], opts)
  ])
  if (refsOut.code !== 0) throw new GitError(refsOut.stderr.trim() || 'for-each-ref failed', refsOut.code, refsOut.stderr, [])
  return createHash('sha1').update(refsOut.stdout).update('\0').update(headRef.stdout).update('\0').update(headHash.stdout).digest('hex')
}

/** git-dir 과 공통 디렉터리(절대 경로). 링크된 워크트리는 둘이 다르고, ref 는 공통 디렉터리에 있다 */
export async function gitDirs(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<string[]> {
  const out = await ok(exec, repoPath, ['rev-parse', '--path-format=absolute', '--git-dir', '--git-common-dir'], opts)
  return [...new Set(out.toString('utf8').split('\n').map((s) => s.trim()).filter(Boolean))]
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
 * 이력 범위(rev) 인자. renderer 가 보낸 ref 가 옵션으로 해석되지 않도록 --end-of-options(git 2.24+) 뒤에 둔다.
 * --ignore-missing: 아직 커밋이 없는 브랜치(orphan)에 HEAD 가 있어도 다른 브랜치들은 보여 준다.
 */
export function revArgs(ref: string | undefined): string[] {
  return ref ? ['--end-of-options', ref] : ['--ignore-missing', '--branches', '--remotes', '--tags', 'HEAD']
}

/**
 * 커밋 목록용 git log 인자. `--date-order` 는 자식이 항상 부모보다 먼저 나오도록 보장해
 * 레인 계산(M2)이 위에서 아래로 한 번에 끝난다.
 */
export function logArgs(ref: string | undefined, skip: number): string[] {
  return ['log', ...LOG_FORMAT_ARGS, '--date-order', ...(skip > 0 ? [`--skip=${skip}`] : []), ...revArgs(ref), '--']
}

/** 실패해도(스태시 없음·커밋 없음 등) 빈 결과로 취급해야 하는 조회용. 취소·실행 실패는 그대로 던진다 */
async function okOrEmpty(exec: GitExecutor, repoPath: string, args: string[], opts?: RunOpts): Promise<Buffer> {
  const r = await exec.run(repoPath, args, opts)
  return r.code === 0 ? r.stdout : Buffer.alloc(0)
}

/**
 * remote 이름과 fetch URL 목록. `git config --get-regexp` 는 remote 가 없으면 종료 코드 1 이라 빈 목록으로 본다.
 * 키는 `remote.<name>.url` — remote 이름에 점이 있을 수 있어(`my.fork`) 앞 `remote.` 와 마지막 `.url` 사이를 이름으로 쓴다.
 * -z 출력은 `key\nvalue\0` 레코드라 URL 에 공백이 있어도 안전하다. 같은 remote 에 url 이 여러 개면 첫 값만 쓴다.
 */
export async function remotes(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<{ name: string; url: string }[]> {
  const args = ['config', '--null', '--get-regexp', '^remote\\..*\\.url$']
  const r = await exec.run(repoPath, args, opts)
  if (r.code === 1) return []
  if (r.code !== 0) throw new GitError(r.stderr.trim() || `git config failed (${r.code})`, r.code, r.stderr, args)
  const seen = new Set<string>()
  const out: { name: string; url: string }[] = []
  for (const rec of r.stdout.toString('utf8').split('\0')) {
    const nl = rec.indexOf('\n')
    if (nl < 0) continue
    const key = rec.slice(0, nl)
    const name = key.slice('remote.'.length, key.length - '.url'.length)
    if (!name || seen.has(name)) continue
    seen.add(name)
    out.push({ name, url: rec.slice(nl + 1) })
  }
  return out
}

/** 사이드바용 ref 스냅샷. 다섯 조회를 동시에 돌린다. */
export async function refs(exec: GitExecutor, repoPath: string, opts?: RunOpts): Promise<RefsSnapshot> {
  const [refsOut, remoteOut, worktreeOut, stashOut, headInfo] = await Promise.all([
    ok(exec, repoPath, REFS_ARGS, opts),
    okOrEmpty(exec, repoPath, ['remote'], opts),
    okOrEmpty(exec, repoPath, WORKTREE_ARGS, opts),
    // 스태시가 없거나 reflog 가 없는 저장소는 빈 목록이다
    okOrEmpty(exec, repoPath, STASH_ARGS, opts),
    head(exec, repoPath, opts)
  ])
  const { local, remotes, tags } = parseRefs(refsOut, parseRemoteNames(remoteOut))
  return { head: headInfo, local, remotes, tags, worktrees: parseWorktrees(worktreeOut), stashes: parseStashes(stashOut) }
}

/**
 * 메시지·작성자·해시 검색. 결과는 그래프와 같은 --date-order 순서를 유지한 합집합이다.
 * 질의는 `--grep=<q>` 처럼 옵션에 붙여 넘기므로 '-' 로 시작해도 옵션으로 해석되지 않는다.
 * 해시 prefix 에 일치하는 커밋은 맨 앞에 둔다.
 */
export async function search(
  exec: GitExecutor,
  repoPath: string,
  query: string,
  ref: string | undefined,
  opts?: RunOpts
): Promise<SearchResult> {
  const q = query.trim()
  if (!q) return { hashes: [], truncated: false }

  const base = ['log', '--format=%H', '-z', '--date-order', '-i', '-F', `--max-count=${SEARCH_LIMIT + 1}`]
  const revs = [...revArgs(ref), '--']
  const run = async (filter: string): Promise<string[]> => {
    const r = await exec.run(repoPath, [...base, filter, ...revs], opts)
    // 커밋이 아직 없는 저장소는 일치 항목이 없는 것과 같다
    if (r.code !== 0 && isEmptyRepoError(r.stderr)) return []
    if (r.code !== 0) throw new GitError(r.stderr.trim() || `git log failed (${r.code})`, r.code, r.stderr, base)
    return r.stdout.toString('utf8').split('\0').filter((h) => h.length > 0)
  }
  const resolveHash = /^[0-9a-f]{4,40}$/i.test(q)
    ? // q 는 위 정규식으로 hex 만 허용하므로 옵션으로 해석될 수 없다(--end-of-options 불필요)
      exec.run(repoPath, ['rev-parse', '--verify', '--quiet', `${q}^{commit}`], opts)
    : Promise.resolve(null)

  const [byMessage, byAuthor, byHash] = await Promise.all([run(`--grep=${q}`), run(`--author=${q}`), resolveHash])

  const matched = await mergeInGraphOrder(exec, repoPath, ref, byMessage, byAuthor, opts)
  const hashes = new Set<string>()
  const resolved = byHash && byHash.code === 0 ? byHash.stdout.toString('utf8').trim() : ''
  // 스태시처럼 지금 보는 이력에서 닿지 않는 커밋은 그래프에서 찾아갈 수 없으므로 뺀다
  if (resolved && (await reachable(exec, repoPath, resolved, ref, opts))) hashes.add(resolved)
  for (const h of matched) hashes.add(h)
  const all = [...hashes]
  return all.length > SEARCH_LIMIT ? { hashes: all.slice(0, SEARCH_LIMIT), truncated: true } : { hashes: all, truncated: false }
}

/** 커밋이 보기 범위(revArgs)에서 도달 가능한지. hash 는 호출자가 rev-parse 로 해석한 전체 해시다 */
async function reachable(exec: GitExecutor, repoPath: string, hash: string, ref: string | undefined, opts?: RunOpts): Promise<boolean> {
  const r = await exec.run(repoPath, ['rev-list', '-n1', hash, '--not', ...revArgs(ref)], opts)
  return r.code === 0 && r.stdout.toString('utf8').trim() === ''
}

/**
 * 메시지 일치와 작성자 일치를 그래프(--date-order) 순서로 합친다.
 * 두 목록은 같은 순서의 부분 수열이지만 서로 다른 항목끼리의 선후는 알 수 없으므로,
 * 둘 다 비어 있지 않을 때만 전체 순서를 스트림으로 훑어 합집합을 다 찾는 순간 멈춘다.
 */
async function mergeInGraphOrder(
  exec: GitExecutor,
  repoPath: string,
  ref: string | undefined,
  a: string[],
  b: string[],
  opts?: RunOpts
): Promise<string[]> {
  if (a.length === 0) return b
  if (b.length === 0) return a
  const want = new Set([...a, ...b])
  const out: string[] = []
  const splitter = new NulRecordSplitter()
  const args = ['log', '--format=%H', '-z', '--date-order', ...revArgs(ref), '--']
  for await (const chunk of exec.stream(repoPath, args, opts)) {
    for (const h of splitter.push(chunk)) if (want.has(h)) out.push(h)
    if (out.length >= want.size) break
  }
  return out
}

function assertHex(value: string | undefined, what: string): asserts value is string {
  if (typeof value !== 'string' || !HEX_RE.test(value)) throw new Error(`invalid ${what}`)
}

/**
 * 커밋 상세: 메타데이터 + 첫 부모 대비 변경 파일 목록(+/- 통계).
 * 첫 부모를 알아야 하므로 메타데이터를 먼저 읽고, 이어서 name-status 와 numstat 을 동시에 돌린다.
 * 루트 커밋은 `--root`(빈 트리 대비), merge 는 첫 부모 대비다.
 */
export async function commitDetail(exec: GitExecutor, repoPath: string, hash: string, opts?: RunOpts): Promise<CommitDetail> {
  assertHex(hash, 'commit hash')
  // 해시는 위 정규식으로 hex 만 허용하므로 옵션으로 해석될 수 없다
  const metaOut = await ok(exec, repoPath, ['log', '-1', ...COMMIT_FORMAT_ARGS, hash, '--'], opts)
  const meta = parseCommitMeta(metaOut)
  const baseParent = meta.parents[0] ?? null

  const base = ['diff-tree', '-r', '-z', '-M', '--no-commit-id', '--no-ext-diff']
  const range = baseParent ? [baseParent, meta.hash] : ['--root', meta.hash]
  const [nameStatus, numstat] = await Promise.all([
    ok(exec, repoPath, [...base, '--name-status', ...range, '--'], opts),
    ok(exec, repoPath, [...base, '--numstat', ...range, '--'], opts)
  ])
  const all = parseChanges(nameStatus, numstat)
  let additions = 0
  let deletions = 0
  for (const f of all) {
    // 바이너리는 null 이라 합계에서 빠진다
    additions += f.additions ?? 0
    deletions += f.deletions ?? 0
  }
  const truncated = all.length > COMMIT_FILES_LIMIT
  return { ...meta, baseParent, files: truncated ? all.slice(0, COMMIT_FILES_LIMIT) : all, additions, deletions, truncated }
}

/**
 * 파일 하나의 diff. 출력은 스트림으로 읽어 줄 수 상한(기본 3000)까지만 객체로 만들고,
 * 나머지는 줄 수만 센다(최대 20만 줄 — 넘으면 totalLines 는 하한값). full 이면 줄 수 상한 없이 64MB 까지 읽는다.
 * 경로는 renderer 가 보낸 값이므로 `--` 뒤에 두고, GIT_LITERAL_PATHSPECS(GIT_ENV)로 pathspec magic 을 끈다.
 */
export async function fileDiff(exec: GitExecutor, repoPath: string, req: FileDiffRequest, opts?: RunOpts): Promise<FileDiff> {
  assertHex(req.hash, 'commit hash')
  if (req.parent !== undefined) assertHex(req.parent, 'parent hash')
  for (const p of [req.path, req.oldPath]) {
    if (p !== undefined && (typeof p !== 'string' || p === '' || p.includes('\0'))) throw new Error('invalid path')
  }
  const parent = req.parent ?? (await firstParent(exec, repoPath, req.hash, opts))
  const paths = req.oldPath !== undefined && req.oldPath !== req.path ? [req.oldPath, req.path] : [req.path]
  const args = [
    'diff-tree',
    '-p',
    '-M',
    '--no-color',
    '--no-ext-diff',
    '--no-textconv',
    '--no-commit-id',
    '-U3',
    ...(parent ? [parent, req.hash] : ['--root', req.hash]),
    '--',
    ...paths
  ]
  const parser = new DiffParser(req.path, req.oldPath, req.full ? { maxLines: Infinity, maxBytes: DIFF_FULL_MAX_BYTES } : { maxBytes: DIFF_MAX_BYTES })
  for await (const chunk of exec.stream(repoPath, args, opts)) {
    // 상한을 넘기고 줄 수 세기도 끝났으면 git 을 끊는다(break 하면 stream 이 프로세스를 정리한다)
    if (!parser.push(chunk)) break
  }
  return parser.finish()
}

/** 첫 부모 해시. 루트 커밋이면 null */
async function firstParent(exec: GitExecutor, repoPath: string, hash: string, opts?: RunOpts): Promise<string | null> {
  const out = await ok(exec, repoPath, ['rev-list', '--parents', '-n1', hash, '--'], opts)
  return out.toString('utf8').trim().split(' ')[1] ?? null
}
