import type { SourceKind } from '@shared/types'

export interface RunOpts {
  signal?: AbortSignal
  env?: Record<string, string>
  /** 이 바이트 수를 넘으면 실패시킨다(기본 64MB) */
  maxBuffer?: number
}

export interface RunResult {
  stdout: Buffer
  stderr: string
  code: number
}

/**
 * 환경별 git 실행 추상화. 구현체는 저장소가 존재하는 환경 안에서 `git -C <repoPath> ...` 를 실행한다.
 * repoPath 는 그 환경의 원래 경로 형식이며 구현체 사이에서 변환하지 않는다.
 */
export interface GitExecutor {
  readonly kind: SourceKind
  run(repoPath: string, args: string[], opts?: RunOpts): Promise<RunResult>
  /** stdout 을 청크 단위로 흘려준다. 비정상 종료 시 마지막에 GitError 를 던진다. */
  stream(repoPath: string, args: string[], opts?: RunOpts): AsyncIterable<Buffer>
  dispose(): Promise<void>
}

export class GitError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly stderr: string,
    readonly args: string[]
  ) {
    super(message)
    this.name = 'GitError'
  }
}

export class AbortError extends Error {
  constructor() {
    super('aborted')
    this.name = 'AbortError'
  }
}

/** 모든 git 호출에 공통으로 붙는 전역 옵션 */
export const GIT_GLOBAL_ARGS = ['-c', 'core.quotepath=false', '-c', 'color.ui=false', '-c', 'log.showSignature=false']

/** 모든 git 호출에 공통으로 넣는 환경변수 */
export const GIT_ENV: Record<string, string> = {
  GIT_OPTIONAL_LOCKS: '0',
  GIT_TERMINAL_PROMPT: '0',
  // renderer 가 보낸 경로가 pathspec magic(:(top), *, :! 등)으로 해석되지 않게 한다
  GIT_LITERAL_PATHSPECS: '1',
  GIT_PAGER: 'cat',
  LANG: 'C.UTF-8',
  LC_ALL: 'C.UTF-8'
}

export const DEFAULT_MAX_BUFFER = 64 * 1024 * 1024

export function buildGitArgs(repoPath: string, args: string[]): string[] {
  return ['-C', repoPath, ...GIT_GLOBAL_ARGS, ...args]
}
