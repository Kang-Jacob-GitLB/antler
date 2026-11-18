// 시험용 minisign 키·서명 생성기(node:crypto). 릴리스 가드(scripts/check-release-assets.mjs·check-release-preflight.mjs)의 서명 검증을 시험하는 데 쓴다.
// 운영 키가 아니다 — 매번 새 키 쌍을 만든다.
import { createHash, generateKeyPairSync, randomBytes, sign, type KeyObject } from 'node:crypto'

export interface Sample {
  privateKey: KeyObject
  keyId: Buffer
  publicRaw: Buffer
  /** UPDATE_PUBKEY 형식(.pub 내용의 base64) */
  pubkeyB64: string
}

export function newKey(): Sample {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519')
  const publicRaw = (publicKey.export({ format: 'der', type: 'spki' }) as Buffer).subarray(-32)
  const keyId = randomBytes(8)
  const keyLine = Buffer.concat([Buffer.from('Ed'), keyId, publicRaw]).toString('base64')
  const text = `untrusted comment: minisign public key: ${keyId.toString('hex').toUpperCase()}\n${keyLine}\n`
  return { privateKey, keyId, publicRaw, pubkeyB64: Buffer.from(text).toString('base64') }
}

/** Tauri CLI 형식의 `.sig`(base64 한 줄). prehash 면 알고리즘 'ED'(BLAKE2b-512), 아니면 'Ed' */
export function signSample(k: Sample, data: Buffer, version: string, opts: { prehash?: boolean; comment?: string } = {}): string {
  const algo = opts.prehash ? 'ED' : 'Ed'
  const message = opts.prehash ? createHash('blake2b512').update(data).digest() : data
  const sig = sign(null, message, k.privateKey)
  const trusted = opts.comment ?? `timestamp:1795020023\tfile:x.bin\tversion:${version}`
  const global = sign(null, Buffer.concat([sig, Buffer.from(trusted)]), k.privateKey)
  const text = `untrusted comment: signature from tauri secret key\n${Buffer.concat([Buffer.from(algo), k.keyId, sig]).toString('base64')}\ntrusted comment: ${trusted}\n${global.toString('base64')}\n`
  return Buffer.from(text).toString('base64')
}
