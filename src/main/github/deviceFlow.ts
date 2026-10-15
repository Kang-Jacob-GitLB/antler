// GitHub OAuth Device Flow. 데스크톱 앱은 client secret 을 안전하게 둘 수 없어 secret 이 필요 없는 이 방식을 쓴다.
// 사용자가 브라우저에서 user_code 를 입력해 승인하면 폴링으로 access_token 을 받는다.
import type { GhDeviceCode } from '@shared/types'
import { AbortError } from '../executors/GitExecutor'

const DEVICE_CODE_URL = 'https://github.com/login/device/code'
const ACCESS_TOKEN_URL = 'https://github.com/login/oauth/access_token'
const SCOPE = 'repo read:org'
const REQUEST_TIMEOUT_MS = 15_000
/** 일시 오류 뒤 다음 시도까지 간격에 더하는 대기(회당 늘리고 상한을 둔다) */
const TRANSIENT_BACKOFF_STEP_MS = 2000
const TRANSIENT_BACKOFF_MAX_MS = 20_000
/** slow_down 응답을 받으면 간격에 더하는 초 */
const SLOW_DOWN_STEP_S = 5

export const MISSING_CLIENT_ID_MESSAGE =
  'GitHub OAuth Client ID 가 설정되지 않았습니다. GitHub 설정(Developer settings → OAuth Apps)에서 OAuth App 을 만들고 ' +
  '"Enable Device Flow" 를 켠 뒤 Client ID 를 설정에 입력하세요. 또는 개인 액세스 토큰(PAT)으로 로그인할 수 있습니다.'

/** 폴링에 필요한 서버 쪽 값. device_code 는 main 에만 둔다(renderer 에는 GhDeviceCode 만 넘긴다) */
export interface DeviceSession {
  deviceCode: string
  /** 초 */
  interval: number
  /** epoch ms */
  expiresAt: number
}

export type SleepFn = (ms: number, signal?: AbortSignal) => Promise<void>

/** 취소할 수 있는 sleep */
export const abortableSleep: SleepFn = (ms, signal) =>
  new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new AbortError())
    const onAbort = (): void => {
      clearTimeout(timer)
      reject(new AbortError())
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, ms)
    signal?.addEventListener('abort', onAbort, { once: true })
  })

/** 재시도할 만한 일시 오류(네트워크·시간 초과·5xx·JSON 이 아닌 본문) */
class TransientError extends Error {}

interface TokenResponse {
  access_token?: string
  error?: string
  error_description?: string
  interval?: number
}

export class DeviceFlow {
  constructor(
    private readonly fetchImpl: typeof fetch = fetch,
    private readonly sleep: SleepFn = abortableSleep,
    private readonly now: () => number = Date.now
  ) {}

  /** 사용자 코드를 발급받는다 */
  async start(clientId: string, signal?: AbortSignal): Promise<{ code: GhDeviceCode; session: DeviceSession }> {
    if (!clientId) throw new Error(MISSING_CLIENT_ID_MESSAGE)
    const body = await this.post<{ device_code?: string; user_code?: string; verification_uri?: string; expires_in?: number; interval?: number; error?: string; error_description?: string }>(
      DEVICE_CODE_URL,
      { client_id: clientId, scope: SCOPE },
      signal
    )
    if (body.error === 'device_flow_disabled') {
      throw new Error('이 OAuth App 은 Device Flow 가 꺼져 있습니다. GitHub 의 OAuth App 설정에서 "Enable Device Flow" 를 켜세요')
    }
    if (body.error === 'incorrect_client_credentials' || body.error === 'unauthorized_client') {
      throw new Error('Client ID 가 올바르지 않습니다. 설정에 입력한 값을 확인하세요')
    }
    if (!body.device_code || !body.user_code || !body.verification_uri) {
      throw new Error(`GitHub 로그인을 시작하지 못했습니다${body.error_description ? `: ${body.error_description}` : ''}`)
    }
    const expiresIn = body.expires_in ?? 900
    const interval = Math.max(1, body.interval ?? 5)
    return {
      code: { userCode: body.user_code, verificationUri: body.verification_uri, expiresIn, interval },
      session: { deviceCode: body.device_code, interval, expiresAt: this.now() + expiresIn * 1000 }
    }
  }

  /**
   * 승인될 때까지 폴링해 access token 을 돌려준다. 간격을 지키고(첫 요청도 interval 뒤),
   * authorization_pending 은 계속, slow_down 은 간격 +5초, 만료·거부는 한국어 에러로 끝낸다.
   * 일시 오류(네트워크·시간 초과·5xx·JSON 아닌 본문)는 세션이 만료될 때까지 조금씩 늘려 가며 다시 시도한다.
   */
  async poll(clientId: string, session: DeviceSession, signal?: AbortSignal): Promise<string> {
    let interval = session.interval
    let failures = 0
    for (;;) {
      const backoff = Math.min(failures * TRANSIENT_BACKOFF_STEP_MS, TRANSIENT_BACKOFF_MAX_MS)
      await this.sleep(interval * 1000 + backoff, signal)
      if (this.now() >= session.expiresAt) throw new Error('인증 시간이 만료되었습니다. 다시 시도하세요')
      let body: TokenResponse
      try {
        body = await this.post<TokenResponse>(
          ACCESS_TOKEN_URL,
          { client_id: clientId, device_code: session.deviceCode, grant_type: 'urn:ietf:params:oauth:grant-type:device_code' },
          signal
        )
      } catch (err) {
        if (!(err instanceof TransientError)) throw err
        failures++
        continue
      }
      failures = 0
      if (body.access_token) return body.access_token
      switch (body.error) {
        case 'authorization_pending':
          break
        case 'slow_down':
          interval = body.interval && body.interval > interval ? body.interval : interval + SLOW_DOWN_STEP_S
          break
        case 'expired_token':
          throw new Error('인증 시간이 만료되었습니다. 다시 시도하세요')
        case 'access_denied':
          throw new Error('GitHub 에서 승인이 거부되었습니다')
        default:
          throw new Error(`GitHub 로그인에 실패했습니다${body.error_description ? `: ${body.error_description}` : body.error ? ` (${body.error})` : ''}`)
      }
    }
  }

  private async post<T>(url: string, payload: Record<string, string>, signal?: AbortSignal): Promise<T> {
    const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    try {
      const res = await this.fetchImpl(url, {
        method: 'POST',
        headers: { Accept: 'application/json', 'Content-Type': 'application/json', 'User-Agent': 'Antler' },
        body: JSON.stringify(payload),
        signal: signal ? AbortSignal.any([timeout, signal]) : timeout
      })
      // 오류(error 필드)도 200 으로 오므로 상태 코드가 아니라 본문으로 판단한다. 5xx·JSON 아닌 본문은 일시 오류다
      if (res.status >= 500) throw new TransientError(`GitHub 서버 오류(${res.status})`)
      try {
        return (await res.json()) as T
      } catch (err) {
        if (signal?.aborted || timeout.aborted) throw err
        throw new TransientError('GitHub 응답을 해석할 수 없습니다')
      }
    } catch (err) {
      if (signal?.aborted) throw new AbortError()
      if (timeout.aborted) throw new TransientError('GitHub 응답이 없습니다(시간 초과)')
      if (err instanceof TransientError) throw err
      throw new TransientError(`GitHub 에 연결할 수 없습니다${err instanceof Error ? `(${err.message})` : ''}`)
    }
  }
}
