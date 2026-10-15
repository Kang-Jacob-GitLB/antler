import { describe, expect, it } from 'vitest'
import { AbortError } from '../src/main/executors/GitExecutor'
import { DeviceFlow, MISSING_CLIENT_ID_MESSAGE, abortableSleep, type DeviceSession } from '../src/main/github/deviceFlow'
import { fakeFetch, json } from './ghFake'

const session = (): DeviceSession => ({ deviceCode: 'dc', interval: 5, expiresAt: Date.now() + 900_000 })

/** 기다리지 않는 sleep — 요청 사이 간격만 기록한다 */
function sleeper(): { sleep: (ms: number) => Promise<void>; waits: number[] } {
  const waits: number[] = []
  return { sleep: async (ms) => void waits.push(ms), waits }
}

describe('DeviceFlow.start', () => {
  it('device/code 로 client_id·scope 를 보낸다', async () => {
    const { fetch, calls } = fakeFetch(() => json({ device_code: 'dc', user_code: 'ABCD-1234', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 }))
    const { code, session: s } = await new DeviceFlow(fetch).start('cid')
    expect(calls[0]!.url).toBe('https://github.com/login/device/code')
    expect(calls[0]!.method).toBe('POST')
    expect(calls[0]!.headers['accept']).toBe('application/json')
    expect(JSON.parse(calls[0]!.body!)).toEqual({ client_id: 'cid', scope: 'repo read:org' })
    expect(code).toEqual({ userCode: 'ABCD-1234', verificationUri: 'https://github.com/login/device', expiresIn: 900, interval: 5 })
    // device_code 는 renderer 용 결과에 없다
    expect(JSON.stringify(code)).not.toContain('dc"')
    expect(s.deviceCode).toBe('dc')
  })

  it('client id 가 없으면 안내 메시지', async () => {
    const { fetch, calls } = fakeFetch(() => json({}))
    await expect(new DeviceFlow(fetch).start('')).rejects.toThrow(MISSING_CLIENT_ID_MESSAGE)
    expect(MISSING_CLIENT_ID_MESSAGE).toContain('Device Flow')
    expect(MISSING_CLIENT_ID_MESSAGE).toContain('PAT')
    expect(calls).toHaveLength(0)
  })

  it('Device Flow 가 꺼진 앱은 안내한다', async () => {
    const { fetch } = fakeFetch(() => json({ error: 'device_flow_disabled' }))
    await expect(new DeviceFlow(fetch).start('cid')).rejects.toThrow('Enable Device Flow')
  })
})

describe('DeviceFlow.poll', () => {
  it('pending → slow_down(+5초) → 성공', async () => {
    const responses = [{ error: 'authorization_pending' }, { error: 'slow_down' }, { error: 'authorization_pending' }, { access_token: 'gho_abc' }]
    const { fetch, calls } = fakeFetch((_r, n) => json(responses[n - 1]))
    const { sleep, waits } = sleeper()
    const token = await new DeviceFlow(fetch, sleep).poll('cid', session())
    expect(token).toBe('gho_abc')
    expect(waits).toEqual([5000, 5000, 10_000, 10_000])
    expect(calls[0]!.url).toBe('https://github.com/login/oauth/access_token')
    expect(JSON.parse(calls[0]!.body!)).toEqual({ client_id: 'cid', device_code: 'dc', grant_type: 'urn:ietf:params:oauth:grant-type:device_code' })
  })

  it('expired_token / access_denied / 기타 오류', async () => {
    const run = (body: unknown) => new DeviceFlow(fakeFetch(() => json(body)).fetch, sleeper().sleep).poll('cid', session())
    await expect(run({ error: 'expired_token' })).rejects.toThrow('만료')
    await expect(run({ error: 'access_denied' })).rejects.toThrow('거부')
    await expect(run({ error: 'incorrect_client_credentials', error_description: 'bad client' })).rejects.toThrow('bad client')
  })

  it('세션 만료 시각이 지나면 요청 없이 만료로 끝낸다', async () => {
    const { fetch, calls } = fakeFetch(() => json({ error: 'authorization_pending' }))
    let now = 0
    const flow = new DeviceFlow(fetch, async (ms) => void (now += ms), () => now)
    await expect(flow.poll('cid', { deviceCode: 'dc', interval: 5, expiresAt: 12_000 })).rejects.toThrow('만료')
    expect(calls).toHaveLength(2)
  })

  it('취소하면 AbortError', async () => {
    const ac = new AbortController()
    const { fetch, calls } = fakeFetch(() => json({ error: 'authorization_pending' }))
    const flow = new DeviceFlow(fetch, async (ms, signal) => {
      ac.abort()
      return abortableSleep(ms, signal)
    })
    await expect(flow.poll('cid', session(), ac.signal)).rejects.toBeInstanceOf(AbortError)
    expect(calls).toHaveLength(0)
  })

  it('요청 중 취소해도 AbortError', async () => {
    const ac = new AbortController()
    const { fetch } = fakeFetch(() => {
      ac.abort()
      return undefined
    })
    const throwing = (async (...args: Parameters<typeof fetch>) => {
      const res = await fetch(...args)
      if (ac.signal.aborted) throw new DOMException('aborted', 'AbortError')
      return res
    }) as typeof fetch
    await expect(new DeviceFlow(throwing, sleeper().sleep).poll('cid', session(), ac.signal)).rejects.toBeInstanceOf(AbortError)
  })
})
