import { execFile } from 'node:child_process'
import { buildGitArgs, GIT_ENV, type GitExecutor, type RunOpts, type RunResult } from './GitExecutor'
import { runProcess, streamProcess, type SpawnSpec } from './processRunner'

const LIST_TIMEOUT_MS = 8000

/**
 * WSL 배포판 안에서 git 을 실행한다. repoPath 는 리눅스 경로('/home/me/repo').
 *
 * `wsl.exe --exec` 는 리눅스 셸을 거치지 않으므로 인자 인용/인젝션 문제가 없다.
 * `--cd ~` 로 배포판 사용자 홈에서 시작하므로 repoPath 는 홈 기준 상대 경로('proj')도 된다.
 * Windows 환경변수는 WSLENV 없이는 WSL 로 넘어가지 않아 GIT_ENV 를 `env` 명령으로 전달한다.
 *
 * 취소: processRunner 의 taskkill /T 가 wsl.exe 를 죽이면 리눅스 쪽 git 도 끝나는지는
 * tests/wslExecutor.test.ts 에서 실측한다.
 */
export class WslExecutor implements GitExecutor {
  readonly kind = 'wsl' as const

  constructor(private readonly distro: string) {}

  private spec(repoPath: string, args: string[]): SpawnSpec {
    return { command: 'wsl.exe', argv: wslArgv(this.distro, repoPath, args), gitArgs: args }
  }

  run(repoPath: string, args: string[], opts?: RunOpts): Promise<RunResult> {
    return runProcess(this.spec(repoPath, args), opts)
  }

  stream(repoPath: string, args: string[], opts?: RunOpts): AsyncIterable<Buffer> {
    return streamProcess(this.spec(repoPath, args), opts)
  }

  async dispose(): Promise<void> {}
}

/** wsl.exe 에 넘길 argv (테스트용으로 분리) */
export function wslArgv(distro: string, repoPath: string, args: string[]): string[] {
  return [
    '-d',
    distro,
    '--cd',
    '~',
    '--exec',
    'env',
    ...Object.entries(GIT_ENV).map(([k, v]) => `${k}=${v}`),
    'git',
    ...buildGitArgs(repoPath, args)
  ]
}

/** `wsl.exe -l -q` 출력(UTF-16LE, BOM/\r/\0 섞임)에서 사용자 배포판 이름만 뽑는다. */
export function parseWslList(buf: Buffer): string[] {
  return buf
    .toString('utf16le')
    .replace(/[﻿\0\r]/g, '')
    .split('\n')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && !s.startsWith('docker-desktop'))
}

/** 설치된 WSL 배포판 목록. Windows 가 아니거나 wsl.exe 가 없거나 멈추면(8초) 빈 배열. */
export function listWslDistros(): Promise<string[]> {
  if (process.platform !== 'win32') return Promise.resolve([])
  return new Promise((resolve) => {
    execFile(
      'wsl.exe',
      ['-l', '-q'],
      { encoding: 'buffer', timeout: LIST_TIMEOUT_MS, windowsHide: true },
      (err, stdout) => resolve(err ? [] : parseWslList(stdout))
    )
  })
}
