import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { SIDECAR_UPDATE_CHANNELS } from '../src/sidecar/configOwns'
import { registerUpdateIpc } from '../src/sidecar/updateIpc'
import type { UpdateService } from '../src/main/update/UpdateService'

// Rust 가 설정을 소유하는 실행(Phase 6b PR 10)의 사이드카 처리기: update:* 7개뿐이고, 본문은 Electron 판 registerIpc(handlers.ts)와 같다.

type Fn = (e: { sender: object }, ...args: unknown[]) => unknown

function register(updates: Partial<Record<keyof UpdateService, (...a: unknown[]) => unknown>>): Map<string, Fn> {
  const handlers = new Map<string, Fn>()
  registerUpdateIpc(updates as unknown as UpdateService, (ch, fn) => void handlers.set(ch, fn))
  return handlers
}

describe('사이드카의 update 처리기(소유 실행)', () => {
  it('등록되는 채널이 정확히 update:* 7개다', () => {
    const handlers = register({})
    expect([...handlers.keys()].sort()).toEqual([...SIDECAR_UPDATE_CHANNELS].sort())
  })

  it('인자를 UpdateService 로 그대로 넘기고 결과를 봉투로 감싼다(check 는 수동 확인)', async () => {
    const calls: string[] = []
    const rec =
      (name: string, ret: unknown = { name }) =>
      (...a: unknown[]) => {
        calls.push(`${name}(${a.map(String).join(',')})`)
        return ret
      }
    const h = register({ get: rec('get'), check: rec('check'), download: rec('download'), cancel: rec('cancel', null), install: rec('install', null), dismiss: rec('dismiss'), releaseNotes: rec('releaseNotes') })
    const e = { sender: {} }
    expect(await h.get('update:get')!(e)).toEqual({ ok: true, data: { name: 'get' } })
    await h.get('update:check')!(e)
    await h.get('update:download')!(e)
    expect(await h.get('update:cancel')!(e)).toEqual({ ok: true, data: null })
    await h.get('update:install')!(e)
    await h.get('update:dismiss')!(e, 'later', '1.2.3')
    await h.get('update:releaseNotes')!(e, '1.2.3')
    expect(calls).toEqual(['get()', 'check(true)', 'download()', 'cancel()', 'install()', 'dismiss(later,1.2.3)', 'releaseNotes(1.2.3)'])
  })

  it('예외는 메시지와 코드 ERROR 의 봉투가 된다', async () => {
    const h = register({
      get: () => {
        throw new Error('깨졌다')
      },
      download: () => Promise.reject('문자열 오류')
    })
    expect(await h.get('update:get')!({ sender: {} })).toEqual({ ok: false, error: '깨졌다', code: 'ERROR' })
    expect(await h.get('update:download')!({ sender: {} })).toEqual({ ok: false, error: '문자열 오류', code: 'ERROR' })
  })

  it('처리기 본문이 Electron 판 registerIpc(handlers.ts)의 update 처리기와 같다', () => {
    const body = (src: string): string[] =>
      [...src.matchAll(/handle\('(update:\w+)',[^\n]*?=> (updates\.[^\n]*)\)\n/g)].map((m) => `${m[1]} ${m[2]}`)
    const electron = body(readFileSync(new URL('../src/main/ipc/handlers.ts', import.meta.url), 'utf8'))
    const sidecar = body(readFileSync(new URL('../src/sidecar/updateIpc.ts', import.meta.url), 'utf8'))
    expect(electron).toHaveLength(7)
    expect(sidecar).toEqual(electron)
  })
})
