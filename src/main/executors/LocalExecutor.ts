import { buildGitArgs, GIT_ENV, type GitExecutor, type RunOpts, type RunResult } from './GitExecutor'
import { runProcess, streamProcess, type SpawnSpec } from './processRunner'

/** 저장소 경로별로 환경을 정하는 함수(GitHub 원격 전용 저장소가 캐시 경로 안에서만 lazy fetch·https 를 열 때 쓴다) */
export type EnvFor = (repoPath: string) => Record<string, string>

/** 앱이 도는 OS 에서 git 을 직접 실행한다(Windows/macOS/Linux 네이티브). */
export class LocalExecutor implements GitExecutor {
  readonly kind = 'local' as const

  constructor(
    private readonly gitBinary = 'git',
    private readonly envFor: EnvFor = () => GIT_ENV
  ) {}

  private spec(repoPath: string, args: string[]): SpawnSpec {
    return { command: this.gitBinary, argv: buildGitArgs(repoPath, args), env: this.envFor(repoPath), gitArgs: args }
  }

  run(repoPath: string, args: string[], opts?: RunOpts): Promise<RunResult> {
    return runProcess(this.spec(repoPath, args), opts)
  }

  stream(repoPath: string, args: string[], opts?: RunOpts): AsyncIterable<Buffer> {
    return streamProcess(this.spec(repoPath, args), opts)
  }

  async dispose(): Promise<void> {}
}
