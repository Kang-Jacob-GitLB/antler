import { execFile } from 'node:child_process'
import { buildGitArgs, GIT_ENV, GitError, type GitExecutor, type RunOpts, type RunResult } from './GitExecutor'
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
    return { command: 'wsl.exe', argv: wslArgv(this.distro, repoPath, args), env: WSL_SPAWN_ENV, gitArgs: args }
  }

  async run(repoPath: string, args: string[], opts?: RunOpts): Promise<RunResult> {
    const r = await runProcess(this.spec(repoPath, args), opts)
    // wsl.exe 자체가 실패하면(배포판 없음·WSL 서비스 중지) 메시지를 stdout 으로 내고 stderr 는 비어 있다
    if (isWslFailure(r.code) && !r.stderr.trim()) {
      return { stdout: Buffer.alloc(0), stderr: wslMessage(r.stdout), code: r.code }
    }
    return r
  }

  async *stream(repoPath: string, args: string[], opts?: RunOpts): AsyncIterable<Buffer> {
    // 실패했을 때 보여 줄 수 있게 앞부분만 기억해 둔다
    let head = Buffer.alloc(0)
    try {
      for await (const chunk of streamProcess(this.spec(repoPath, args), opts)) {
        if (head.length < 4096) head = Buffer.concat([head, chunk]).subarray(0, 4096)
        yield chunk
      }
    } catch (e) {
      if (e instanceof GitError && isWslFailure(e.code) && !e.stderr.trim()) {
        const msg = wslMessage(head)
        throw new GitError(msg, e.code, msg, args)
      }
      throw e
    }
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
  return decodeWslText(buf)
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
      { encoding: 'buffer', timeout: LIST_TIMEOUT_MS, windowsHide: true, env: { ...process.env, ...WSL_SPAWN_ENV } },
      (err, stdout) => resolve(err ? [] : parseWslList(stdout))
    )
  })
}

/**
 * wsl.exe 가 내보내는 자체 메시지의 인코딩을 UTF-8 로 고정한다. 기본은 UTF-16LE 지만 사용자가 WSL_UTF8=1 을
 * 설정해 두었을 수도 있어, 어느 쪽인지 추측하지 않도록 항상 지정한다(파싱은 두 인코딩 모두 받아들인다).
 */
const WSL_SPAWN_ENV: Record<string, string> = { WSL_UTF8: '1' }

/** wsl.exe 자체 실패 종료 코드(0xFFFFFFFF). Node 는 플랫폼에 따라 -1 로 줄 수도 있다 */
function isWslFailure(code: number): boolean {
  return code === 0xffffffff || code === -1
}

/** UTF-16LE(짝수 바이트 뒤 NUL 이 많은 경우)인지 대략 판별해 디코딩한다 */
function decodeWslText(buf: Buffer): string {
  let zeros = 0
  for (let i = 1; i < Math.min(buf.length, 64); i += 2) if (buf[i] === 0) zeros++
  const utf16 = buf.length >= 4 && zeros >= Math.min(buf.length, 64) / 4
  return (utf16 ? buf.toString('utf16le') : buf.toString('utf8')).replace(/[﻿\0\r]/g, '')
}

function wslMessage(buf: Buffer): string {
  const text = decodeWslText(buf).trim()
  return text ? `WSL: ${text}` : 'WSL 실행에 실패했습니다(배포판 이름과 WSL 상태를 확인하세요)'
}
