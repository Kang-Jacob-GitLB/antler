// minisign 서명 검증(node:crypto 만 쓴다 — 새 패키지 없음). Tauri CLI 의 `.sig` 와 `UPDATE_PUBKEY` 형식을 읽는다.
//   공개 키(UPDATE_PUBKEY) = base64( "untrusted comment: …\n<base64: 'Ed' + 키 ID 8바이트 + Ed25519 공개 키 32바이트>\n" )
//   서명(.sig)            = base64( "untrusted comment: …\n<base64: 알고리즘 2바이트 + 키 ID 8 + 서명 64>\ntrusted comment: <문구>\n<base64: 전역 서명 64>\n" )
//   알고리즘 'Ed' 는 파일 내용에 직접, 'ED' 는 BLAKE2b-512(내용)에 서명한다. 전역 서명은 (서명 64바이트 ‖ trusted comment 문구)에 대한 같은 키의 서명이다.
// 릴리스 가드(scripts/check-release-assets.mjs)가 "서명이 UPDATE_PUBKEY 의 키로 만든 것인가"를 확인하는 데 쓴다(키 ID 일치 + 실제 서명 검증).
import { createHash, createPublicKey, verify } from 'node:crypto'

const SPKI_ED25519_PREFIX = Buffer.from('302a300506032b6570032100', 'hex')

const decodeText = (b64) => Buffer.from(String(b64).trim(), 'base64').toString('utf8').replace(/\r\n/g, '\n')

/** UPDATE_PUBKEY → { keyId: Buffer(8), publicKey: Buffer(32) } 또는 null */
export function parsePublicKey(pubkeyB64) {
  const lines = decodeText(pubkeyB64).split('\n')
  if (!lines[0]?.startsWith('untrusted comment:') || !lines[1]) return null
  const raw = Buffer.from(lines[1].trim(), 'base64')
  if (raw.length !== 42 || raw.subarray(0, 2).toString('latin1') !== 'Ed') return null
  return { keyId: raw.subarray(2, 10), publicKey: raw.subarray(10, 42) }
}

/** `.sig` 파일 내용 → { algo, keyId, signature, trustedComment, globalSignature } 또는 null */
export function parseSignature(sigText) {
  const lines = decodeText(sigText).split('\n')
  if (!lines[0]?.startsWith('untrusted comment:') || !lines[1] || !lines[2]?.startsWith('trusted comment: ') || !lines[3]) return null
  const raw = Buffer.from(lines[1].trim(), 'base64')
  const global = Buffer.from(lines[3].trim(), 'base64')
  if (raw.length !== 74 || global.length !== 64) return null
  const algo = raw.subarray(0, 2).toString('latin1')
  if (algo !== 'Ed' && algo !== 'ED') return null
  return { algo, keyId: raw.subarray(2, 10), signature: raw.subarray(10, 74), trustedComment: lines[2].slice('trusted comment: '.length), globalSignature: global }
}

const keyObject = (pk) => createPublicKey({ key: Buffer.concat([SPKI_ED25519_PREFIX, pk]), format: 'der', type: 'spki' })

/**
 * 서명이 공개 키의 것이고 내용·trusted comment 와 맞는지 본다. 돌려주는 값: null(통과) 또는 이유 문자열.
 * `version` 을 주면 trusted comment 에 `version:<버전>` 이 있어야 한다(앱의 requireSignedVersion 전제).
 */
export function verifyMinisign({ pubkeyB64, sigText, data, version }) {
  const pub = parsePublicKey(pubkeyB64)
  if (!pub) return '공개 키 형식이 올바르지 않습니다'
  const sig = parseSignature(sigText)
  if (!sig) return '서명(.sig) 형식이 올바르지 않습니다'
  if (!sig.keyId.equals(pub.keyId)) return `서명의 키 ID(${sig.keyId.toString('hex')})가 UPDATE_PUBKEY 의 키 ID(${pub.keyId.toString('hex')})와 다릅니다`
  const key = keyObject(pub.publicKey)
  const message = sig.algo === 'ED' ? createHash('blake2b512').update(data).digest() : data
  if (!verify(null, message, key, sig.signature)) return '서명이 내용과 맞지 않습니다'
  if (!verify(null, Buffer.concat([sig.signature, Buffer.from(sig.trustedComment, 'utf8')]), key, sig.globalSignature)) return 'trusted comment 의 전역 서명이 맞지 않습니다'
  if (version !== undefined && !sig.trustedComment.split('\t').includes(`version:${version}`)) return `서명에 기록된 버전이 ${version} 이 아닙니다`
  return null
}
