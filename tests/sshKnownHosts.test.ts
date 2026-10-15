import { createHmac, randomBytes } from 'node:crypto'
import { describe, expect, it } from 'vitest'
import { checkHostKey, knownHostKeyAlgorithms } from '../src/main/executors/ssh/knownHosts'
import { newKeyPair } from './sshHarness'

const k1 = newKeyPair()
const k2 = newKeyPair()
const b64 = (k: { blob: Buffer }) => k.blob.toString('base64')

function hashed(host: string): string {
  const salt = randomBytes(20)
  const hash = createHmac('sha1', salt).update(host).digest()
  return `|1|${salt.toString('base64')}|${hash.toString('base64')}`
}

describe('known_hosts 검증', () => {
  it('평문·쉼표 목록·와일드카드', () => {
    const t = `# c\nexample.com,10.0.0.1 ssh-ed25519 ${b64(k1)} comment\n*.corp.local ssh-ed25519 ${b64(k1)}\n`
    expect(checkHostKey(t, 'example.com', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, '10.0.0.1', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'a.corp.local', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'EXAMPLE.com', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'other.com', 22, k1.blob).status).toBe('unknown')
  })

  it('비표준 포트는 [host]:port 로만 일치한다', () => {
    const t = `[example.com]:2222 ssh-ed25519 ${b64(k1)}\n`
    expect(checkHostKey(t, 'example.com', 2222, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'example.com', 22, k1.blob).status).toBe('unknown')
    expect(checkHostKey(t, 'example.com', 2223, k1.blob).status).toBe('unknown')
  })

  it('해시 항목(HMAC-SHA1), 포트 포함 형태도', () => {
    const t = `${hashed('example.com')} ssh-ed25519 ${b64(k1)}\n${hashed('[example.com]:2200')} ssh-ed25519 ${b64(k2)}\n`
    expect(checkHostKey(t, 'example.com', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'example.com', 2200, k2.blob).status).toBe('ok')
    expect(checkHostKey(t, 'example.com', 2200, k1.blob).status).toBe('mismatch')
    expect(checkHostKey(t, 'nope.com', 22, k1.blob).status).toBe('unknown')
  })

  it('같은 종류의 다른 키는 불일치(mismatch)', () => {
    const t = `example.com ssh-ed25519 ${b64(k1)}\n`
    expect(checkHostKey(t, 'example.com', 22, k2.blob).status).toBe('mismatch')
  })

  it('@revoked 는 거부, @cert-authority 는 무시', () => {
    const t = `@revoked * ssh-ed25519 ${b64(k2)}\n@cert-authority *.x.com ssh-ed25519 ${b64(k1)}\nz.x.com ssh-ed25519 ${b64(k1)}\n`
    expect(checkHostKey(t, 'z.x.com', 22, k2.blob).status).toBe('revoked')
    expect(checkHostKey(t, 'z.x.com', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(`@cert-authority *.x.com ssh-ed25519 ${b64(k1)}\n`, 'a.x.com', 22, k1.blob).status).toBe('unknown')
  })

  it('부정 패턴과 빈 파일', () => {
    const t = `*.x.com,!bad.x.com ssh-ed25519 ${b64(k1)}\n`
    expect(checkHostKey(t, 'good.x.com', 22, k1.blob).status).toBe('ok')
    expect(checkHostKey(t, 'bad.x.com', 22, k1.blob).status).toBe('unknown')
    expect(checkHostKey('', 'a', 22, k1.blob).status).toBe('unknown')
  })

  it('등록된 키 종류로 serverHostKey 알고리즘을 제한한다', () => {
    const t = `h ssh-rsa AAAA\nh ecdsa-sha2-nistp256 AAAA\n`
    expect(knownHostKeyAlgorithms(t, 'h', 22)).toEqual(['rsa-sha2-512', 'rsa-sha2-256', 'ssh-rsa', 'ecdsa-sha2-nistp256'])
    expect(knownHostKeyAlgorithms(t, 'zz', 22)).toEqual([])
  })
})
