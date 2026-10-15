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

/**
 * 모든 git 호출에 공통으로 붙는 전역 옵션.
 * core.fsmonitor=false: 저장소 설정의 fsmonitor 명령은 index 를 읽는 명령에서 실행된다(악성 저장소 RCE 경로).
 * 지금은 index 를 읽지 않지만 앞으로 status 등을 더해도 안전하도록 미리 끈다.
 */
export const GIT_GLOBAL_ARGS = [
  '-c',
  'core.quotepath=false',
  '-c',
  'color.ui=false',
  '-c',
  'log.showSignature=false',
  '-c',
  'core.fsmonitor=false'
]

/**
 * 모든 git 호출에 공통으로 넣는 환경변수.
 *
 * 보안: 읽기 전용 명령이라도 partial clone(promisor) 저장소에서 없는 객체를 만나면 git 이 원격에서 객체를
 * 가져오려(lazy fetch) 하고, 그때 저장소 설정의 core.sshCommand · remote.*.uploadpack 을 실행한다.
 * 악성 저장소를 여는 것만으로 코드가 실행되는 경로이므로(tests/hostileRepo.test.ts 로 재현) 두 겹으로 막는다.
 *  - GIT_NO_LAZY_FETCH=1   lazy fetch 자체를 끈다(git 2.45.1+)
 *  - GIT_ALLOW_PROTOCOL=none 모든 전송 프로토콜을 금지한다. 저장소 설정의 protocol.*.allow 보다 우선하고
 *    옛 git(WSL·원격 서버)에서도 동작한다. Antler 는 fetch/clone 을 하지 않으므로 잃는 기능이 없다.
 */
export const GIT_ENV: Record<string, string> = {
  GIT_OPTIONAL_LOCKS: '0',
  GIT_TERMINAL_PROMPT: '0',
  // renderer 가 보낸 경로가 pathspec magic(:(top), *, :! 등)으로 해석되지 않게 한다
  GIT_LITERAL_PATHSPECS: '1',
  GIT_NO_LAZY_FETCH: '1',
  // refs/replace 로 이력을 위조해 보여 주지 못하게 한다(fetch 가 아니어도 저장소에 심어 둘 수 있다)
  GIT_NO_REPLACE_OBJECTS: '1',
  GIT_ALLOW_PROTOCOL: 'none',
  GIT_PAGER: 'cat',
  LANG: 'C.UTF-8',
  LC_ALL: 'C.UTF-8'
}

export const DEFAULT_MAX_BUFFER = 64 * 1024 * 1024

export function buildGitArgs(repoPath: string, args: string[]): string[] {
  return ['-C', repoPath, ...GIT_GLOBAL_ARGS, ...args]
}
