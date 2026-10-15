import { buildGitArgs, GIT_ENV, type GitExecutor, type RunOpts, type RunResult } from './GitExecutor'
import { runProcess, streamProcess, type SpawnSpec } from './processRunner'

/** 앱이 도는 OS 에서 git 을 직접 실행한다(Windows/macOS/Linux 네이티브). */
export class LocalExecutor implements GitExecutor {
  readonly kind = 'local' as const

  constructor(private readonly gitBinary = 'git') {}

  private spec(repoPath: string, args: string[]): SpawnSpec {
    return { command: this.gitBinary, argv: buildGitArgs(repoPath, args), env: GIT_ENV, gitArgs: args }
  }

  run(repoPath: string, args: string[], opts?: RunOpts): Promise<RunResult> {
    return runProcess(this.spec(repoPath, args), opts)
  }

  stream(repoPath: string, args: string[], opts?: RunOpts): AsyncIterable<Buffer> {
    return streamProcess(this.spec(repoPath, args), opts)
  }

  async dispose(): Promise<void> {}
}
