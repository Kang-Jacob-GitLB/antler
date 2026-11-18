#!/usr/bin/env node
// 릴리스 발행 전 점검(.github/workflows/release.yml 의 preflight 잡 — 빌드·태그보다 앞). 하나라도 어긋나면 exit 1.
//   1) 버전이 semver(MAJOR.MINOR.PATCH[-pre])다 — 설치본 이름·업데이트 비교가 이 값에 기댄다
//   2) 업데이트 공개 키(src-tauri/src/update/host.rs 의 UPDATE_PUBKEY)가 들어 있다. `None` 인 채로 발행하면 그 릴리스의 앱은 업데이트를 설치하지 못한다
//      (fail-closed — 이미 설치된 사용자는 앱 안에서 영영 고칠 수 없다). 그래서 키 없는 빌드의 발행을 여기서 막는다
// 사용: node scripts/check-release-preflight.mjs --version <버전> [--host <host.rs 경로>]
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

export const HOST_RS = 'src-tauri/src/update/host.rs'

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?$/

/** host.rs 소스에서 운영 공개 키 상수를 읽는다. 상수가 없으면 undefined, `None` 이면 null, `Some("…")` 면 그 문자열 */
export function readPubkey(rustSource) {
  const m = /^\s*pub const UPDATE_PUBKEY: Option<&str> = (None|Some\("([^"]*)"\));/m.exec(rustSource)
  if (!m) return undefined
  return m[1] === 'None' ? null : m[2]
}

/** 공개 키 문자열이 `tauri signer generate` 의 `.pub` 내용(minisign 공개 키 텍스트의 base64)인지 본다. 시험용 키(TEST-ONLY)는 거부한다 */
export function checkPubkey(rustSource) {
  const key = readPubkey(rustSource)
  if (key === undefined) return { ok: false, reason: 'UPDATE_PUBKEY 상수를 host.rs 에서 찾지 못했습니다' }
  if (key === null) return { ok: false, reason: 'UPDATE_PUBKEY 가 None 입니다 — 공개 키를 넣기 전에는 발행할 수 없습니다(docs/updates.md "업데이트 공개 키")' }
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(key)) return { ok: false, reason: 'UPDATE_PUBKEY 가 base64 가 아닙니다' }
  const text = Buffer.from(key, 'base64').toString('utf8').replace(/\r\n/g, '\n')
  const [comment, body, ...rest] = text.split('\n')
  if (!comment?.startsWith('untrusted comment: minisign public key')) return { ok: false, reason: 'UPDATE_PUBKEY 가 minisign 공개 키 파일(.pub)의 내용이 아닙니다' }
  if (/TEST-ONLY/i.test(comment)) return { ok: false, reason: 'UPDATE_PUBKEY 가 시험용 키입니다' }
  if (!body || !/^RW[A-Za-z0-9+/]{54}$/.test(body) || rest.some((l) => l.trim() !== '')) return { ok: false, reason: 'UPDATE_PUBKEY 의 키 줄 형식이 올바르지 않습니다' }
  return { ok: true }
}

export function checkVersion(version) {
  return SEMVER.test(version ?? '') ? { ok: true } : { ok: false, reason: `semver 가 아닌 버전입니다: ${version}` }
}

function parseArgs(argv) {
  const out = {}
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--') || argv[i + 1] === undefined) throw new Error(`인자가 올바르지 않습니다: ${argv[i]}`)
    out[argv[i].slice(2)] = argv[i + 1]
  }
  return out
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2))
    const results = [checkVersion(args.version), checkPubkey(readFileSync(resolve(args.host ?? HOST_RS), 'utf8'))]
    const failed = results.filter((r) => !r.ok)
    for (const f of failed) console.error(`::error::${f.reason}`)
    if (failed.length) process.exit(1)
    console.log(`발행 전 점검 통과: 버전 ${args.version}, 업데이트 공개 키 있음`)
  } catch (e) {
    console.error(`::error::${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }
}

if (/check-release-preflight\.mjs$/.test(process.argv[1] ?? '')) main()
