// 원격 명령 문자열 구성. ssh exec 채널은 명령을 원격 로그인 셸에 문자열로 넘기므로 인용이 곧 보안 경계다.
//
// 인용 규칙(bash/zsh/sh/fish 공통으로 같은 뜻이 되는 교집합만 쓴다):
//  - 일반 문자열은 '…' 안에 둔다. 작은따옴표 안에서는 $ ` ! 개행 등 모두 리터럴이다.
//  - 작은따옴표 자체는 따옴표 밖에서 \' 로 쓴다( 'a'\''b' 형태).
//  - 백슬래시는 따옴표 밖에서 \\ 로 쓴다. fish 는 작은따옴표 안에서도 \\ 와 \' 를 해석하므로
//    '…\…' 처럼 따옴표 안에 백슬래시를 두면 셸마다 뜻이 갈린다 — 그래서 항상 따옴표 밖으로 뺀다.
// NUL 은 어떤 셸에서도 인자에 담을 수 없으므로 거부한다. csh 계열 로그인 셸은 지원하지 않는다.
import { buildGitArgs } from '../GitExecutor'

export function shQuote(s: string): string {
  if (s.includes('\0')) throw new Error('원격 명령 인자에 NUL 문자는 쓸 수 없습니다')
  if (s === '') return "''"
  let out = ''
  let run = ''
  const flush = (): void => {
    if (run) out += `'${run}'`
    run = ''
  }
  for (const ch of s) {
    if (ch === "'") {
      flush()
      out += "\\'"
    } else if (ch === '\\') {
      flush()
      out += '\\\\'
    } else {
      run += ch
    }
  }
  flush()
  return out
}

const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

/** `env K='V' … git -C '<path>' '-c' … '<arg>'` 형태의 명령 문자열 */
export function buildRemoteCommand(repoPath: string, args: string[], env: Record<string, string>): string {
  const parts = ['env']
  for (const [k, v] of Object.entries(env)) {
    if (!ENV_NAME.test(k)) throw new Error(`잘못된 환경변수 이름: ${k}`)
    parts.push(`${k}=${shQuote(v)}`)
  }
  parts.push('git', ...buildGitArgs(repoPath, args).map(shQuote))
  return parts.join(' ')
}
