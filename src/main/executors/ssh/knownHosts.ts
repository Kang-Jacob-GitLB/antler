// known_hosts 기반 호스트 키 검증. 키를 자동으로 추가하지 않는다(TOFU 없음) — 모르는 호스트는 거부한다.
//
// 지원: 평문 항목(쉼표 목록·와일드카드·!부정·[host]:port), 해시 항목 |1|salt|hash(HMAC-SHA1), @revoked(거부).
// 무시: @cert-authority(인증서 검증 미지원이므로 항목 자체를 건너뛴다).
import { createHash, createHmac } from 'node:crypto'
import { readFileSync } from 'node:fs'

interface KnownHostEntry {
  marker?: 'revoked' | 'cert-authority'
  hosts: string
  keyType: string
  keyBase64: string
}

export type HostKeyCheck =
  | { status: 'ok' }
  | { status: 'unknown' }
  | { status: 'mismatch' }
  | { status: 'revoked' }

export function parseKnownHosts(text: string): KnownHostEntry[] {
  const out: KnownHostEntry[] = []
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const f = line.split(/\s+/)
    let marker: KnownHostEntry['marker']
    if (f[0]!.startsWith('@')) {
      const m = f.shift()!
      if (m === '@revoked') marker = 'revoked'
      else if (m === '@cert-authority') marker = 'cert-authority'
      else continue
    }
    const [hosts, keyType, keyBase64] = f
    if (!hosts || !keyType || !keyBase64) continue
    out.push({ marker, hosts, keyType, keyBase64 })
  }
  return out
}

/** SSH 공개키 blob 의 앞 문자열(키 종류) */
export function keyTypeOfBlob(blob: Buffer): string {
  if (blob.length < 4) return ''
  const len = blob.readUInt32BE(0)
  if (len > blob.length - 4) return ''
  return blob.subarray(4, 4 + len).toString('ascii')
}

/** known_hosts 에서 쓰는 호스트 표기: 22 번 포트는 host, 그 외는 [host]:port */
export function knownHostsName(host: string, port: number): string {
  const h = host.toLowerCase()
  return port === 22 ? h : `[${h}]:${port}`
}

function globMatch(pattern: string, s: string): boolean {
  let re = ''
  for (const ch of pattern) {
    if (ch === '*') re += '.*'
    else if (ch === '?') re += '.'
    else re += ch.replace(/[.+^${}()|[\]\\]/g, '\\$&')
  }
  return new RegExp(`^${re}$`, 'i').test(s)
}

function hostMatches(hosts: string, name: string): boolean {
  if (hosts.startsWith('|1|')) {
    const [, , saltB64, hashB64] = hosts.split('|')
    if (!saltB64 || !hashB64) return false
    const mac = createHmac('sha1', Buffer.from(saltB64, 'base64')).update(name).digest()
    return mac.equals(Buffer.from(hashB64, 'base64'))
  }
  let positive = false
  for (const p of hosts.split(',')) {
    if (p.startsWith('!')) {
      if (globMatch(p.slice(1), name)) return false
    } else if (globMatch(p, name)) {
      positive = true
    }
  }
  return positive
}

function matching(entries: KnownHostEntry[], host: string, port: number): KnownHostEntry[] {
  const name = knownHostsName(host, port)
  return entries.filter((e) => e.marker !== 'cert-authority' && hostMatches(e.hosts, name))
}

export function checkHostKey(text: string, host: string, port: number, key: Buffer): HostKeyCheck {
  const hits = matching(parseKnownHosts(text), host, port)
  const b64 = key.toString('base64')
  const type = keyTypeOfBlob(key)
  if (hits.some((e) => e.marker === 'revoked' && e.keyBase64 === b64)) return { status: 'revoked' }
  const sameType = hits.filter((e) => e.marker !== 'revoked' && e.keyType === type)
  if (sameType.some((e) => e.keyBase64 === b64)) return { status: 'ok' }
  if (sameType.length > 0) return { status: 'mismatch' }
  return { status: 'unknown' }
}

/** known_hosts 에 이 호스트로 등록된 키 종류에 대응하는 ssh2 serverHostKey 알고리즘(서버가 다른 종류를 내밀지 않게 제한) */
export function knownHostKeyAlgorithms(text: string, host: string, port: number): string[] {
  const out: string[] = []
  for (const e of matching(parseKnownHosts(text), host, port)) {
    if (e.marker === 'revoked') continue
    const algs = e.keyType === 'ssh-rsa' ? ['rsa-sha2-512', 'rsa-sha2-256', 'ssh-rsa'] : [e.keyType]
    for (const a of algs) if (!out.includes(a)) out.push(a)
  }
  return out
}

export function fingerprint(key: Buffer): string {
  return `SHA256:${createHash('sha256').update(key).digest('base64').replace(/=+$/, '')}`
}

export function readKnownHosts(path: string): string {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

export type VerifyResult = { ok: true } | { ok: false; message: string }

/**
 * 연결 시점마다 파일을 다시 읽어 검증한다(사용자가 ssh CLI 로 키를 수락한 직후 재시도하면 바로 반영되도록).
 * 메시지에는 공개 지문만 싣는다.
 */
export function verifyHostKey(knownHostsPath: string, host: string, port: number, key: Buffer): VerifyResult {
  const label = knownHostsName(host, port)
  const r = checkHostKey(readKnownHosts(knownHostsPath), host, port, key)
  switch (r.status) {
    case 'ok':
      return { ok: true }
    case 'unknown':
      return {
        ok: false,
        message:
          `알 수 없는 호스트입니다: ${label} (키 지문 ${fingerprint(key)}). ` +
          `터미널에서 ssh CLI 로 한 번 접속해 호스트 키를 수락한 뒤 다시 시도하세요. ` +
          `Antler 는 호스트 키를 자동으로 등록하지 않습니다.`
      }
    case 'mismatch':
      return {
        ok: false,
        message:
          `호스트 키가 known_hosts 의 기록과 다릅니다: ${label} (받은 키 지문 ${fingerprint(key)}). ` +
          `중간자 공격일 수 있어 연결을 거부했습니다. 서버를 재설치한 것이 확실하다면 known_hosts 의 해당 줄을 지우고 ssh CLI 로 다시 확인하세요.`
      }
    case 'revoked':
      return { ok: false, message: `폐기된(@revoked) 호스트 키입니다: ${label}. 연결을 거부했습니다.` }
  }
}
