import { inspect } from 'node:util'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { GhTokenMirror, MAX_GH_TOKEN_LENGTH, handleGhToken } from '../src/sidecar/ghToken'

const TOKEN = 'ghp_0123456789abcdefghijklmnopqrstuvwxyzAB'

afterEach(() => vi.restoreAllMocks())

describe('사이드카 ghToken(Rust 가 보낸 GitHub 토큰의 메모리 사본)', () => {
  it('문자열은 저장하고 null 은 비운다. 바뀔 때만 알린다', () => {
    const m = new GhTokenMirror()
    const changed = vi.fn()
    expect(m.get()).toBeNull()
    expect(handleGhToken(m, { token: TOKEN }, changed)).toBe('set')
    expect(m.get()).toBe(TOKEN)
    expect(handleGhToken(m, { token: TOKEN }, changed)).toBe('unchanged')
    expect(handleGhToken(m, { token: `${TOKEN}2` }, changed)).toBe('set')
    expect(handleGhToken(m, { token: null }, changed)).toBe('cleared')
    expect(m.get()).toBeNull()
    expect(handleGhToken(m, { token: null }, changed)).toBe('unchanged')
    expect(changed).toHaveBeenCalledTimes(3)
  })

  it('모양이 틀린 값은 거부하고 이전 토큰 사본도 비운다', () => {
    const m = new GhTokenMirror()
    const changed = vi.fn()
    const bad: unknown[] = [undefined, '', 5, {}, [], true, 'a b', 'a\nb', 'a\u0000b', '토큰', ' x', 'x'.repeat(MAX_GH_TOKEN_LENGTH + 1)]
    for (const token of bad) {
      handleGhToken(m, { token: TOKEN }, () => undefined)
      expect(m.get()).toBe(TOKEN)
      changed.mockClear()
      expect(handleGhToken(m, { token }, changed), String(token)).toBe('rejected')
      expect(m.get(), String(token)).toBeNull()
      // 가지고 있던 토큰이 사라졌으니 바뀐 것이다
      expect(changed).toHaveBeenCalledTimes(1)
    }
    // 가진 것이 없을 때의 거부는 알리지 않는다
    changed.mockClear()
    expect(handleGhToken(m, { token: 'a b' }, changed)).toBe('rejected')
    expect(changed).not.toHaveBeenCalled()
    expect(handleGhToken(m, { token: 'x'.repeat(MAX_GH_TOKEN_LENGTH) }, changed)).toBe('set')
  })

  it('토큰이 로그·오류·직렬화·검사에 나오지 않는다', () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const stdout = vi.spyOn(process.stdout, 'write').mockImplementation(() => true)
    const m = new GhTokenMirror()
    handleGhToken(m, { token: TOKEN }, () => undefined)
    // 거부한 값(토큰 모양이 아닌 값에 토큰을 섞어 넣는다)도 적지 않는다
    handleGhToken(m, { token: `${TOKEN} bad` }, () => undefined)
    const shown = [JSON.stringify(m), inspect(m, { depth: 5 }), String(m), JSON.stringify({ m }), inspect({ nested: { m } }, { depth: 5 })]
    for (const text of shown) expect(text).not.toContain(TOKEN)
    for (const call of [...stderr.mock.calls, ...stdout.mock.calls]) expect(JSON.stringify(call)).not.toContain(TOKEN)
    expect(stderr).toHaveBeenCalledTimes(1)
  })

  it('사이드카 본문은 토큰을 env·로그로 내보내지 않는다(소스 검사)', () => {
    const root = join(__dirname, '..', 'src', 'sidecar')
    const mirror = readFileSync(join(root, 'ghToken.ts'), 'utf8')
    expect(mirror).not.toMatch(/process\.env/)
    expect(mirror).not.toMatch(/console\.(log|info|debug|dir)/)
    // 이 파일의 console 호출은 고정 문구 하나뿐이다
    expect(mirror.match(/console\.error\(/g)).toHaveLength(1)
    const index = readFileSync(join(root, 'index.ts'), 'utf8')
    // ghToken 메시지는 Rust 가 GitHub 를 처리할 때만 받는다
    expect(index).toMatch(/if \(rustApi\) handleGhToken\(ghMirror, msg,/)
    // 토큰의 출처가 사본이다(rustApi 일 때 이 사이드카의 서비스는 로그인하지 않는다)
    expect(index).toContain('githubToken: () => (rustApi ? ghMirror.get() : gh.gitToken())')
    expect(index).toContain('...(rustApi ? {} : { avatarCacheFile:')
  })
})
