import { existsSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { RUST_CONFIG_CHANNELS, SIDECAR_UPDATE_CHANNELS, rustOwnsConfig, sidecarAccepts } from '../src/sidecar/configOwns'
import { IPC_CHANNELS } from '../src/shared/ipc'

// Rust 가 설정 파일을 소유하는 실행(Phase 6b PR 9, env ANTLER_RUST_CONFIG=1): 한 실행에서 설정 파일을 쓰는 쪽은 하나다.
// 이때(PR 10) 사이드카는 `update:*` 7채널만 받는다. `ConfigStore` 의 readOnly 옵션은 Electron 판 코드(src/main)에 남아 있어 계속 시험한다(6c 에서 정리). Electron 판은 이 옵션을 쓰지 않아 동작이 그대로다.

const dir = (): string => mkdtempSync(join(tmpdir(), 'antler-cfgowns-'))

describe('사이드카의 설정 소유 판정', () => {
  it('env 값이 정확히 "1" 일 때만 Rust 가 소유한다(알 수 없는 값은 사이드카가 전부 맡는다)', () => {
    expect(rustOwnsConfig('1')).toBe(true)
    for (const v of [undefined, '', '0', 'true', 'yes', '1 ', ' 1']) expect(rustOwnsConfig(v), String(v)).toBe(false)
  })

  it('설정 소유 채널 11개는 IPC 허용 목록의 부분집합이고 소유할 때는 사이드카가 받지 않는다', () => {
    expect(RUST_CONFIG_CHANNELS).toHaveLength(11)
    for (const ch of RUST_CONFIG_CHANNELS) {
      expect((IPC_CHANNELS as readonly string[]).includes(ch), ch).toBe(true)
      expect(sidecarAccepts(ch, true), ch).toBe(false)
      expect(sidecarAccepts(ch, false), ch).toBe(true)
    }
  })

  it('소유할 때 사이드카가 받는 채널은 update:* 7개뿐이다(IPC_CHANNELS 의 update: 접두 채널과 정확히 같다)', () => {
    const updates = (IPC_CHANNELS as readonly string[]).filter((c) => c.startsWith('update:'))
    expect([...SIDECAR_UPDATE_CHANNELS].sort()).toEqual([...updates].sort())
    expect(SIDECAR_UPDATE_CHANNELS).toHaveLength(7)
    // 허용 목록의 모든 채널을 돌려 본다 — 소유할 때 받는 것이 정확히 그 7개다
    const accepted = (IPC_CHANNELS as readonly string[]).filter((c) => sidecarAccepts(c, true))
    expect(accepted.sort()).toEqual([...updates].sort())
    // 소유하지 않으면(ANTLER_RUST_GIT=0) 전부 받는다
    expect((IPC_CHANNELS as readonly string[]).every((c) => sidecarAccepts(c, false))).toBe(true)
    // 허용 목록 밖의 채널은 소유할 때 받지 않는다
    for (const ch of ['', 'git:nope', 'update:', 'update:get ', 'UPDATE:GET', 'update:getX']) expect(sidecarAccepts(ch, true), ch).toBe(false)
  })

  it('Rust 의 UPDATE_CHANNELS(router.rs)와 같은 목록이다', () => {
    const rs = readFileSync(new URL('../src-tauri/src/router.rs', import.meta.url), 'utf8')
    const block = /pub const UPDATE_CHANNELS: &\[&str\] = &\[([\s\S]*?)\];/.exec(rs)![1]!
    const rust = [...block.matchAll(/"([a-zA-Z:]+)"/g)].map((m) => m[1])
    expect(rust).toEqual([...SIDECAR_UPDATE_CHANNELS])
  })

  it('Rust 의 CONFIG_CHANNELS(router.rs)와 같은 목록이다', () => {
    const rs = readFileSync(new URL('../src-tauri/src/router.rs', import.meta.url), 'utf8')
    const block = /pub const CONFIG_CHANNELS: &\[&str\] = &\[([\s\S]*?)\];/.exec(rs)![1]!
    const rust = [...block.matchAll(/"([a-zA-Z:]+)"/g)].map((m) => m[1])
    expect(rust).toEqual([...RUST_CONFIG_CHANNELS])
  })
})

describe('읽기 전용 설정(readOnly)', () => {
  it('파일이 있어도 열지 않고 기본값으로 시작하며 파일을 건드리지 않는다', () => {
    const d = dir()
    const file = join(d, 'antler.config.json')
    const text = JSON.stringify({ version: 1, sources: [{ id: 'local', kind: 'local', label: 'Local' }], repos: [{ id: 'r', sourceId: 'local', path: 'C:\\a', name: 'a' }], ui: {} })
    writeFileSync(file, text, 'utf8')
    const c = new ConfigStore(file, { readOnly: true })
    expect(c.get().repos).toEqual([])
    // 빈 목록은 사실이 아니다 — 캐시 정리의 근거로 삼지 못하게 loadFailed 도 켠다
    expect(c.loadFailed).toBe(true)
    expect(readFileSync(file, 'utf8')).toBe(text)
    expect(readdirSync(d)).toEqual(['antler.config.json'])
  })

  it('파일이 없어도 만들지 않고 모든 변경자가 던진다(쓰는 쪽은 Rust 하나다)', () => {
    const d = dir()
    const file = join(d, 'antler.config.json')
    const c = new ConfigStore(file, { readOnly: true })
    const ops: (() => unknown)[] = [
      () => c.setUi({ theme: 'dark' }),
      () => c.addRepo({ sourceId: 'local', path: 'C:\\a', name: 'a' }),
      () => c.updateRepo('x', { name: 'b' }),
      () => c.removeRepo('x'),
      () => c.setEditorExePath('C:\\e.exe'),
      () => c.setSkippedUpdateVersion('1.2.3'),
      () => c.setTrustedConfig('x', 'a'.repeat(64)),
      () => c.setFetchApproval('x', 'a'.repeat(64)),
      // 원격 전용 저장소의 기본 브랜치 기록은 레코드가 있을 때만 쓴다 — 빈 설정에는 레코드가 없어 쓸 길이 없다
      () => c.setWindowState({ x: 0, y: 0, width: 800, height: 600 })
    ]
    for (const op of ops) expect(op).toThrow()
    expect(existsSync(file)).toBe(false)
    expect(readdirSync(d)).toEqual([])
    // 알림도 가지 않는다(미러·fetch 설정을 보내지 않는다)
    let notified = 0
    c.onChange = () => void notified++
    expect(() => c.setUi({ theme: 'dark' })).toThrow()
    expect(notified).toBe(0)
  })

  it('옵션이 없으면(Electron 판·되돌리기) 지금처럼 읽고 쓴다', () => {
    const d = dir()
    const file = join(d, 'antler.config.json')
    const c = new ConfigStore(file)
    expect(c.loadFailed).toBe(false)
    c.setUi({ theme: 'dark' })
    expect(existsSync(file)).toBe(true)
    expect(new ConfigStore(file).get().ui.theme).toBe('dark')
  })
})
