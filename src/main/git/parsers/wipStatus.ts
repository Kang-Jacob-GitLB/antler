import type { FileChange, FileStatus, WipStatus } from '@shared/types'
import { NulRecordSplitter } from './nulRecords'

/** 추적 안 되는 파일 목록 상한. 넘는 항목은 개수만 센다 */
export const WIP_UNTRACKED_LIMIT = 5000

/**
 * 작업 트리 상태 조회 인자. `-uall` 로 추적 안 되는 폴더 안의 파일을 하나씩 받는다.
 * --ignore-submodules=all: 서브모듈마다 git 을 하나씩 더 띄우지 않게 한다.
 * 안전: 이 명령은 인덱스를 읽는다 — core.fsmonitor 는 GIT_GLOBAL_ARGS 가 꺼 두었고, GIT_OPTIONAL_LOCKS=0 이라 인덱스를 쓰지 않는다.
 */
export const WIP_STATUS_ARGS = ['status', '--porcelain=v2', '-z', '--untracked-files=all', '--renames', '--ignore-submodules=all', '--']

const KNOWN = new Set<string>(['A', 'M', 'D', 'R', 'C', 'T'])
/** porcelain v2 의 XY 한 글자 → 파일 상태. '.'(변경 없음)은 undefined */
function toStatus(c: string): FileStatus | undefined {
  return KNOWN.has(c) ? (c as FileStatus) : undefined
}

/** 앞쪽 공백 구분 필드 n 개를 건너뛴 나머지(경로 — 공백이 들어 있을 수 있다) */
function afterFields(rec: string, n: number): string {
  let i = 0
  for (let k = 0; k < n; k++) {
    i = rec.indexOf(' ', i)
    if (i < 0) return ''
    i++
  }
  return rec.slice(i)
}

const file = (status: FileStatus, path: string, oldPath?: string): FileChange => ({
  status,
  path,
  ...(oldPath !== undefined ? { oldPath } : {}),
  additions: null,
  deletions: null,
  binary: false
})

/**
 * `git status --porcelain=v2 -z` 출력을 청크 단위로 받아 WipStatus 로 만든다.
 * 레코드: `1 XY sub mH mI mW hH hI path` · `2 XY sub mH mI mW hH hI Xscore path\0origPath`(이름 변경: 새 경로가 먼저)
 * · `u XY sub m1 m2 m3 mW h1 h2 h3 path`(충돌) · `? path`. X 는 인덱스(스테이지), Y 는 작업 트리 열이라 `MM` 은 양쪽 목록에 모두 들어간다.
 * git 은 추적 중인 항목을 모두 낸 뒤에 `?` 항목을 내므로, 추적 안 되는 파일은 상한까지만 담고 나머지는 세기만 한다.
 */
export class WipStatusParser {
  private readonly splitter = new NulRecordSplitter()
  private readonly staged: FileChange[] = []
  private readonly unstaged: FileChange[] = []
  private readonly untracked: string[] = []
  private untrackedTotal = 0
  private conflicted = 0
  /** 이름 변경 레코드 뒤에 오는 원래 경로를 기다리는 중인 항목 */
  private renameOf: FileChange[] | null = null
  private readonly modifiedPaths = new Set<string>()
  private readonly addedPaths = new Set<string>()

  constructor(private readonly untrackedLimit = WIP_UNTRACKED_LIMIT) {}

  push(chunk: Buffer): void {
    for (const rec of this.splitter.push(chunk)) this.record(rec)
  }

  finish(): WipStatus {
    for (const rec of this.splitter.end()) this.record(rec)
    const added = this.addedPaths.size + this.untrackedTotal
    return {
      staged: this.staged,
      unstaged: this.unstaged,
      untracked: this.untracked,
      untrackedMore: this.untrackedTotal - this.untracked.length,
      conflicted: this.conflicted,
      counts: {
        modified: this.modifiedPaths.size,
        added,
        staged: this.staged.length,
        unstaged: this.unstaged.length,
        untracked: this.untrackedTotal
      }
    }
  }

  private record(rec: string): void {
    if (this.renameOf) {
      // 직전 `2` 레코드의 원래 경로
      for (const f of this.renameOf) f.oldPath = rec
      this.renameOf = null
      return
    }
    const kind = rec.charAt(0)
    if (kind === '?') {
      this.untrackedTotal++
      if (this.untracked.length < this.untrackedLimit) this.untracked.push(rec.slice(2))
    } else if (kind === '1' || kind === '2') {
      const path = afterFields(rec, kind === '1' ? 8 : 9)
      if (!path) return
      const x = toStatus(rec.charAt(2))
      const y = toStatus(rec.charAt(3))
      const made: FileChange[] = []
      if (x) {
        const f = file(x, path)
        this.staged.push(f)
        made.push(f)
        if (x === 'A') this.addedPaths.add(path)
        else this.modifiedPaths.add(path)
      }
      if (y) {
        const f = file(y, path)
        this.unstaged.push(f)
        made.push(f)
        if (!this.addedPaths.has(path)) this.modifiedPaths.add(path)
      }
      if (kind === '2') this.renameOf = made
    } else if (kind === 'u') {
      const path = afterFields(rec, 10)
      if (!path) return
      this.conflicted++
      this.unstaged.push(file('U', path))
      this.modifiedPaths.add(path)
    }
  }
}

/** 이미 메모리에 있는 출력(Buffer)을 한 번에 파싱한다 */
export function parseWipStatus(buf: Buffer, untrackedLimit?: number): WipStatus {
  const p = new WipStatusParser(untrackedLimit)
  p.push(buf)
  return p.finish()
}
