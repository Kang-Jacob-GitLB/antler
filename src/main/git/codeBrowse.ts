// 코드 탐색기: 커밋 시점의 디렉터리 목록(ls-tree)과 파일 내용(cat-file blob). 읽기 전용이며 환경과 무관하다(executor 위에서 동작).
// 파일 내용은 `cat-file blob` 으로만 읽는다 — `git show`·diff 계열과 달리 textconv·필터·속성이 적용되지 않는다.
import type { CodeBlobRequest, CodeBlobResult, CodeEntryKind, CodeTreeEntry, CodeTreeRequest, CodeTreeResult } from '@shared/types'
import { parseLfsPointer } from '@shared/lfs'
import { isValidScopeRef } from '@shared/logScope'
import { GitError, type GitExecutor, type RunOpts } from '../executors/GitExecutor'
import { assertHex } from './commands'

/** 텍스트로 보여 줄 파일 크기 상한 */
export const CODE_TEXT_MAX_BYTES = 2 * 1024 * 1024
/** 이미지 미리보기 크기 상한 */
export const CODE_IMAGE_MAX_BYTES = 10 * 1024 * 1024
/** 읽은 내용이 상한을 조금 넘어도 실패하지 않도록 주는 여유 */
const BUFFER_SLACK = 64 * 1024
/** NUL 바이트로 바이너리를 판정할 앞부분 길이(git 의 휴리스틱과 같다) */
const BINARY_SNIFF_BYTES = 8000
const COMMIT_ID_RE = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i

const IMAGE_MIME: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  ico: 'image/x-icon',
  svg: 'image/svg+xml',
  avif: 'image/avif'
}

function imageMime(path: string): string | undefined {
  const base = path.slice(path.lastIndexOf('/') + 1)
  const dot = base.lastIndexOf('.')
  return dot < 0 ? undefined : IMAGE_MIME[base.slice(dot + 1).toLowerCase()]
}

/** 디렉터리 경로 정규화: '/' 앞뒤를 정리한다. '' 는 최상위. NUL·'.'·'..'·빈 구성 요소는 거부 */
function normalizeDir(dir: unknown): string {
  if (typeof dir !== 'string' || dir.includes('\0')) throw new Error('invalid path')
  const trimmed = dir.replace(/^\/+|\/+$/g, '')
  if (trimmed === '') return ''
  for (const seg of trimmed.split('/')) {
    if (seg === '' || seg === '.' || seg === '..') throw new Error('invalid path')
  }
  return trimmed
}

function normalizeFilePath(path: unknown): string {
  const p = normalizeDir(path)
  if (p === '') throw new Error('invalid path')
  return p
}

function isHexRef(ref: string): boolean {
  try {
    assertHex(ref, 'ref')
    return true
  } catch {
    return false
  }
}

/**
 * 'HEAD' · 전체 ref 이름 · 커밋 해시를 커밋 해시로 푼다. 단축 이름(main)·리비전 식(HEAD~1)·옵션처럼 보이는 값은 거부한다.
 * `--end-of-options` 로 ref 가 옵션으로 해석되는 일을 막고 `^{commit}` 으로 태그 객체 등을 벗긴다.
 */
export async function resolveRev(exec: GitExecutor, repoPath: string, ref: unknown, opts?: RunOpts): Promise<string> {
  if (typeof ref !== 'string') throw new Error('invalid ref')
  // 리모트 전체 패턴(refs/remotes/x/*)은 isValidScopeRef 를 통과하지만 하나의 ref 가 아니다
  if (ref !== 'HEAD' && !isHexRef(ref) && (!isValidScopeRef(ref) || ref.endsWith('/*'))) throw new Error('invalid ref')
  const r = await exec.run(repoPath, ['rev-parse', '--verify', '--end-of-options', `${ref}^{commit}`], opts)
  const hash = r.code === 0 ? r.stdout.toString('utf8').trim() : ''
  if (!COMMIT_ID_RE.test(hash)) throw new Error(`커밋을 찾을 수 없습니다: ${ref}`)
  return hash.toLowerCase()
}

function kindOf(mode: string, type: string): CodeEntryKind {
  if (type === 'tree' || mode === '040000') return 'dir'
  if (mode === '120000') return 'symlink'
  if (type === 'commit' || mode === '160000') return 'submodule'
  return 'file'
}

/** 대소문자를 무시하고(로캘과 무관하게 코드 유닛 순) 비교한 뒤 같으면 원문으로 비교한다 */
function compareNames(a: string, b: string): number {
  const la = a.toLowerCase()
  const lb = b.toLowerCase()
  if (la !== lb) return la < lb ? -1 : 1
  return a < b ? -1 : a > b ? 1 : 0
}

function parseTree(buf: Buffer, dir: string): CodeTreeEntry[] {
  const entries: CodeTreeEntry[] = []
  for (const rec of buf.toString('utf8').split('\0')) {
    if (!rec) continue
    const tab = rec.indexOf('\t')
    if (tab < 0) continue
    const [mode, type, oid] = rec.slice(0, tab).split(' ')
    const name = rec.slice(tab + 1)
    if (!mode || !type || !oid || !name) continue
    entries.push({ name, path: dir ? `${dir}/${name}` : name, kind: kindOf(mode, type), mode, oid })
  }
  return entries.sort((x, y) => (x.kind === 'dir' ? 0 : 1) - (y.kind === 'dir' ? 0 : 1) || compareNames(x.name, y.name))
}

/**
 * 커밋 시점 디렉터리 하나의 항목. `ls-tree -l` 은 쓰지 않는다(크기를 구하려고 blob 마다 lazy fetch 가 일어난다).
 * 경로가 없거나 트리가 아니면 missing. 객체가 로컬에 없는 경우(partial clone)도 같다.
 */
export async function listTree(exec: GitExecutor, repoPath: string, req: CodeTreeRequest, opts?: RunOpts): Promise<CodeTreeResult> {
  assertHex(req.rev, 'commit hash')
  const dir = normalizeDir(req.dir)
  const rev = req.rev
  const missing: CodeTreeResult = { rev, dir, entries: [], missing: true }
  // rev 는 hex 만 허용하므로 `rev:dir` 이 옵션으로 해석될 수 없다
  const spec = dir ? `${rev}:${dir}` : rev
  if (dir) {
    const t = await exec.run(repoPath, ['cat-file', '-t', spec], opts)
    if (t.code !== 0 || t.stdout.toString('utf8').trim() !== 'tree') return missing
  }
  const args = ['ls-tree', '-z', '--end-of-options', spec]
  const r = await exec.run(repoPath, args, opts)
  if (r.code !== 0) {
    // 커밋·트리 객체가 로컬에 없다(partial clone, 잘못된 해시)
    if (/not a valid object name|not a tree object|bad object|unable to read|could not (?:read|parse)|missing/i.test(r.stderr)) return missing
    throw new GitError(r.stderr.trim() || `git ls-tree failed (${r.code})`, r.code, r.stderr, args)
  }
  return { rev, dir, entries: parseTree(r.stdout, dir), missing: false }
}

/**
 * 커밋 시점 파일 내용. `cat-file -t/-s` 로 종류·크기를 먼저 보고 상한을 넘으면 내용을 읽지 않는다.
 * 판정 순서: 이미지 확장자 → NUL 이 있으면 바이너리 → LFS 포인터 → 텍스트.
 */
export async function readBlob(exec: GitExecutor, repoPath: string, req: CodeBlobRequest, opts?: RunOpts): Promise<CodeBlobResult> {
  assertHex(req.rev, 'commit hash')
  const path = normalizeFilePath(req.path)
  const rev = req.rev
  const spec = `${rev}:${path}`
  const none: CodeBlobResult = { rev, path, kind: 'missing', size: 0 }

  // 디렉터리(tree)·서브모듈(commit 객체는 이 저장소에 없다)·없는 경로·로컬에 없는 객체는 여기서 걸러진다
  const t = await exec.run(repoPath, ['cat-file', '-t', spec], opts)
  if (t.code !== 0 || t.stdout.toString('utf8').trim() !== 'blob') return none
  const s = await exec.run(repoPath, ['cat-file', '-s', spec], opts)
  if (s.code !== 0) return none
  const size = Number(s.stdout.toString('utf8').trim())
  if (!Number.isFinite(size) || size < 0) return none

  const mime = imageMime(path)
  if (size > (mime ? CODE_IMAGE_MAX_BYTES : CODE_TEXT_MAX_BYTES)) return { rev, path, kind: 'tooLarge', size }

  const r = await exec.run(repoPath, ['cat-file', 'blob', spec], { ...opts, maxBuffer: (mime ? CODE_IMAGE_MAX_BYTES : CODE_TEXT_MAX_BYTES) + BUFFER_SLACK })
  if (r.code !== 0) return none
  const buf = r.stdout

  if (mime) return { rev, path, kind: 'image', size, dataUrl: `data:${mime};base64,${buf.toString('base64')}` }
  if (buf.subarray(0, BINARY_SNIFF_BYTES).includes(0)) return { rev, path, kind: 'binary', size }

  // TextDecoder 는 기본적으로 UTF-8 BOM 을 벗긴다
  const text = new TextDecoder('utf-8', { fatal: false }).decode(buf)
  const lfs = parseLfsPointer(text)
  if (lfs) return { rev, path, kind: 'lfs', size, lfs: { oid: lfs.oid, size: lfs.size } }
  let invalidUtf8 = false
  try {
    new TextDecoder('utf-8', { fatal: true }).decode(buf)
  } catch {
    invalidUtf8 = true
  }
  return { rev, path, kind: 'text', size, text, ...(invalidUtf8 ? { invalidUtf8: true } : {}) }
}
