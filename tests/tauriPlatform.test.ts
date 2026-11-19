// Tauri 판 bridge 가 Rust 가 주입한 OS 이름을 쓰는지(win32 고정이 아닌지) 확인한다.
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { resolvePlatform } from '../src/renderer/tauri/platform'

describe('resolvePlatform', () => {
  it('주입된 값을 그대로 쓴다', () => {
    expect(resolvePlatform('darwin', 'Windows')).toBe('darwin')
    expect(resolvePlatform('linux', 'Windows')).toBe('linux')
    expect(resolvePlatform('win32', 'Macintosh')).toBe('win32')
  })

  it('주입이 없거나 모르는 값이면 userAgent 로 추정한다', () => {
    expect(resolvePlatform(undefined, 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)')).toBe('darwin')
    expect(resolvePlatform('sunos', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)')).toBe('win32')
    expect(resolvePlatform(42, 'Mozilla/5.0 (X11; Linux x86_64)')).toBe('linux')
  })
})

describe('bridge.ts 소스', () => {
  const src = readFileSync('src/renderer/tauri/bridge.ts', 'utf8')

  it("'win32' 를 platform 값으로 고정하지 않는다", () => {
    expect(src).not.toMatch(/platform:\s*'win32'/)
    expect(src).toMatch(/platform,\s*$/m)
    expect(src).toContain('__ANTLER_PLATFORM__')
  })

  it('창 단추는 Windows 에서만 그린다', () => {
    expect(src).toMatch(/if \(customTitlebar\) mountWindowControls\(\)/)
  })
})
