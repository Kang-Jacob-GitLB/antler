// GitHub CLI(gh)에서 로그인 토큰을 가져온다. 셸을 쓰지 않고(execFile) 필요한 환경 변수만 넘기며,
// 토큰은 호출한 쪽(GitHubService)에게만 돌려준다 — 로그·에러 메시지에 stdout/stderr 를 싣지 않는다.
import { execFile } from 'node:child_process'

const TIMEOUT_MS = 10_000
const MAX_BUFFER = 64 * 1024

/** 사용자에게 그대로 보여 줘도 되는 한국어 문장을 가진 에러 */
export class GhCliError extends Error {
  constructor(
    message: string,
    readonly code: 'GH_CLI_MISSING' | 'GH_CLI_NOT_LOGGED_IN' | 'GH_CLI_FAILED'
  ) {
    super(message)
    this.name = 'GhCliError'
  }
}

export interface GhCli {
  /** gh 를 실행할 수 있는지(결과는 캐시된다) */
  available(): Promise<boolean>
  /** github.com 에 로그인된 gh 의 토큰. 실패하면 GhCliError */
  token(): Promise<string>
}

/** execFile 대체 — 테스트에서 가짜 gh 를 끼운다 */
export type ExecFn = (file: string, args: string[], opts: { env: NodeJS.ProcessEnv; timeout: number; maxBuffer: number }) => Promise<{ stdout: string }>

export const realExec: ExecFn = (file, args, opts) =>
  new Promise((resolve, reject) => {
    // shell 없이, stdin 은 쓰지 않으며 콘솔 창을 띄우지 않는다
    const child = execFile(file, args, { ...opts, windowsHide: true, encoding: 'utf8', shell: false }, (err, stdout) => {
      if (err) reject(err)
      else resolve({ stdout })
    })
    child.stdin?.end()
  })

/** gh 에 넘기는 최소 환경: 실행 파일 탐색과 gh 설정/자격 증명 위치를 찾는 데 필요한 것만 */
export function ghEnv(src: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  // Windows 는 변수 이름의 대소문자를 구분하지 않아 PATH/Path 가 함께 보일 수 있다 — 이름당 하나만 넘겨 중복 항목을 피한다
  const keep = new Set(
    [
      'PATH', 'PATHEXT', 'SystemRoot', 'ComSpec', 'TEMP', 'TMP',
      'HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME',
      'GH_CONFIG_DIR', 'GH_HOST'
    ].map((k) => k.toLowerCase())
  )
  const env: NodeJS.ProcessEnv = {}
  const seen = new Set<string>()
  for (const k of Object.keys(src)) {
    const lk = k.toLowerCase()
    if (!keep.has(lk) || src[k] === undefined) continue
    if (seen.has(lk)) continue
    seen.add(lk)
    env[k] = src[k]
  }
  // 대화형 프롬프트·색 코드가 출력에 섞이지 않게 한다
  env['GH_PROMPT_DISABLED'] = '1'
  env['NO_COLOR'] = '1'
  return env
}

export function createGhCli(exec: ExecFn = realExec, env: () => NodeJS.ProcessEnv = () => ghEnv(), bin = 'gh'): GhCli {
  let avail: Promise<boolean> | null = null
  const run = (args: string[]): Promise<{ stdout: string }> => exec(bin, args, { env: env(), timeout: TIMEOUT_MS, maxBuffer: MAX_BUFFER })
  return {
    available() {
      avail ??= run(['--version']).then(
        () => true,
        () => false
      )
      return avail
    },
    async token() {
      let out: string
      try {
        out = (await run(['auth', 'token', '--hostname', 'github.com'])).stdout
      } catch (err) {
        const e = err as NodeJS.ErrnoException & { killed?: boolean }
        if (e?.code === 'ENOENT') throw new GhCliError('GitHub CLI(gh)가 설치되어 있지 않습니다', 'GH_CLI_MISSING')
        if (e?.killed || e?.code === 'ETIMEDOUT') throw new GhCliError('GitHub CLI 가 10초 안에 응답하지 않았습니다', 'GH_CLI_FAILED')
        // 종료 코드가 0 이 아니면 로그인되어 있지 않은 것(stderr 는 보여 주지 않는다)
        throw new GhCliError('GitHub CLI 에 로그인되어 있지 않습니다. 터미널에서 `gh auth login` 을 먼저 실행하세요', 'GH_CLI_NOT_LOGGED_IN')
      }
      const token = out.trim()
      if (!token) throw new GhCliError('GitHub CLI 에 로그인되어 있지 않습니다. 터미널에서 `gh auth login` 을 먼저 실행하세요', 'GH_CLI_NOT_LOGGED_IN')
      return token
    }
  }
}
