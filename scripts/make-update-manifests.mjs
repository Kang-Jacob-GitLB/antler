#!/usr/bin/env node
// Tauri 판 Windows 설치본(NSIS)의 업데이트 매니페스트 두 개를 만든다(.github/workflows/tauri-package.yml 의 Windows 갈래가 쓴다).
//   latest.json — Tauri 업데이터용(src-tauri/src/update/logic.rs 의 check_manifest 가 읽는 형식). 서명(.sig)이 있을 때만 만든다
//   latest.yml  — Electron 판(electron-updater)이 Tauri 설치본을 받아 가게 하는 이관용. electron-builder 가 만드는 모양을 따른다
// 발행하지 않는다: 파일을 --dir 에 쓸 뿐이다. 실제 첨부는 나중의 릴리스 전환 PR(feat!:)이 한다.
//
// 사용: node scripts/make-update-manifests.mjs --version <semver> --dir <설치본이 있는 폴더> [--installer <파일 이름>]
//         [--sig <.sig 파일>] [--electron-migration on|off]
//   --installer  기본 Antler-Setup-<버전>-x64.exe (Electron 의 artifactName 규칙. 서명은 내용에 걸려 이름과 무관하다)
//   --electron-migration  off·false·0·no 면 latest.yml 을 만들지 않는다(비상 정지 스위치). 기본은 환경 변수 ELECTRON_MIGRATION, 그것도 없으면 on
//
// latest.yml 의 url·path 는 **파일 이름만** 쓴다 — electron-updater 의 GitHub 공급자가 `/<소유자>/<저장소>/releases/download/<태그>/<url>` 로 붙이므로
// 절대 URL 을 넣으면 경로가 깨져 이관이 통째로 실패한다. latest.json 의 url 은 반대로 절대 URL 이다(check_manifest 가 태그 결속을 본다).
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { basename, join } from 'node:path'

export const REPO = 'Kang-Jacob-GitLB/antler'
/** src-tauri/src/update/logic.rs 의 RELEASE_DOWNLOAD_PREFIX 와 같아야 한다(tests/updateManifests.test.ts 가 대조한다) */
export const DOWNLOAD_PREFIX = `https://github.com/${REPO}/releases/download/`

/** release.yml 의 버전 검사와 같은 식(MAJOR.MINOR.PATCH[-pre]) */
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?$/
/** 서명은 base64 문자만, 16KiB 이하(logic.rs 의 MAX_SIGNATURE_LEN) */
const MAX_SIGNATURE_LEN = 16 * 1024

/** logic.rs 의 is_safe_file_name 과 같다: ^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$ 이고 `..` 없음 */
export function isSafeFileName(n) {
  return typeof n === 'string' && /^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$/.test(n) && !n.includes('..')
}

export function assertVersion(v) {
  if (typeof v !== 'string' || !SEMVER.test(v)) throw new Error(`semver 가 아닌 버전입니다: ${String(v)}`)
  return v
}

/** Windows 설치본 이름(Electron 규칙) */
export const installerName = (version) => `Antler-Setup-${assertVersion(version)}-x64.exe`

export const sha512Base64 = (bytes) => createHash('sha512').update(bytes).digest('base64')

/** 서명 파일 내용 → 한 줄 base64. 형식이 틀리면 던진다(check_manifest 가 거부하는 값을 올리지 않는다) */
export function cleanSignature(text) {
  const sig = String(text ?? '').trim()
  if (!sig) throw new Error('서명이 비어 있습니다')
  if (sig.length > MAX_SIGNATURE_LEN || !/^[A-Za-z0-9+/=]+$/.test(sig)) throw new Error('서명 형식이 올바르지 않습니다(base64 한 줄이어야 합니다)')
  return sig
}

/** Tauri 업데이터 매니페스트(latest.json). 플랫폼 키는 `windows-x86_64-nsis` 와 `windows-x86_64` 둘 다 같은 항목이다. `notes` 는 서명되지 않은 필드라 넣지 않는다 */
export function buildLatestJson({ version, fileName, signature, pubDate }) {
  assertVersion(version)
  if (!isSafeFileName(fileName)) throw new Error(`안전하지 않은 파일 이름입니다: ${fileName}`)
  const date = new Date(pubDate)
  if (Number.isNaN(date.getTime())) throw new Error(`날짜가 올바르지 않습니다: ${String(pubDate)}`)
  const entry = { signature: cleanSignature(signature), url: `${DOWNLOAD_PREFIX}v${version}/${fileName}` }
  const doc = { version, pub_date: date.toISOString(), platforms: { 'windows-x86_64-nsis': entry, 'windows-x86_64': { ...entry } } }
  return `${JSON.stringify(doc, null, 2)}\n`
}

/** Electron 호환 latest.yml. 키 순서·모양은 electron-builder(updateInfoBuilder.createUpdateInfo)와 같다: version, files[{url,sha512,size}], path, sha512, releaseDate */
export function buildLatestYml({ version, fileName, sha512, size, releaseDate }) {
  assertVersion(version)
  if (!isSafeFileName(fileName)) throw new Error(`안전하지 않은 파일 이름입니다: ${fileName}`)
  if (!/^[A-Za-z0-9+/]{86}==$/.test(sha512 ?? '')) throw new Error('sha512(base64) 형식이 올바르지 않습니다')
  if (!Number.isSafeInteger(size) || size <= 0) throw new Error('size 가 올바르지 않습니다')
  const date = new Date(releaseDate)
  if (Number.isNaN(date.getTime())) throw new Error(`날짜가 올바르지 않습니다: ${String(releaseDate)}`)
  return [
    `version: ${version}`,
    'files:',
    `  - url: ${fileName}`,
    `    sha512: ${sha512}`,
    `    size: ${size}`,
    `path: ${fileName}`,
    `sha512: ${sha512}`,
    `releaseDate: '${date.toISOString()}'`,
    ''
  ].join('\n')
}

/** ELECTRON_MIGRATION 값 해석: `off`·`false`·`0`·`no`(대소문자·앞뒤 공백 무시)가 끈다. 비어 있거나 다른 값이면 켠다 */
export const migrationEnabled = (value) => !['off', 'false', '0', 'no'].includes(String(value ?? '').trim().toLowerCase())

/**
 * 설치본(디스크의 실물)을 읽어 매니페스트를 만들고, 만든 값이 실물과 맞는지 되짚는다(가드).
 * 돌려주는 값: { files: { <이름>: <내용> }, warnings: [문자열], sha512, size }
 */
export function makeManifests({ dir, version, installer, signatureText, migration, now = new Date() }) {
  assertVersion(version)
  const fileName = installer ?? installerName(version)
  if (fileName !== installerName(version)) throw new Error(`설치본 이름이 버전과 맞지 않습니다: ${fileName} (기대: ${installerName(version)})`)
  const file = join(dir, fileName)
  if (!existsSync(file) || !statSync(file).isFile()) throw new Error(`설치본이 없습니다: ${file}`)
  const bytes = readFileSync(file)
  const sha512 = sha512Base64(bytes)
  const size = bytes.length
  const files = {}
  const warnings = []

  if (signatureText == null || String(signatureText).trim() === '') {
    warnings.push('서명(.sig)이 없어 latest.json 을 만들지 않았습니다')
  } else {
    files['latest.json'] = buildLatestJson({ version, fileName, signature: signatureText, pubDate: now })
  }

  if (migrationEnabled(migration)) {
    files['latest.yml'] = buildLatestYml({ version, fileName, sha512, size, releaseDate: now })
  } else {
    warnings.push('ELECTRON_MIGRATION 이 꺼져(off·false·0·no) 있어 latest.yml 을 만들지 않았습니다(Electron 판 이관 중단)')
  }

  // 가드: url 의 파일 이름이 올릴 자산과 같고, 해시·크기가 실물과 같다
  if (files['latest.json']) {
    const doc = JSON.parse(files['latest.json'])
    for (const [key, p] of Object.entries(doc.platforms)) {
      if (basename(p.url) !== fileName || !p.url.startsWith(`${DOWNLOAD_PREFIX}v${doc.version}/`)) throw new Error(`latest.json ${key} 의 url 이 자산과 맞지 않습니다`)
    }
    if (doc.version !== version) throw new Error('latest.json 의 버전이 입력과 다릅니다')
  }
  return { files, warnings, sha512, size, fileName }
}

function parseArgs(argv) {
  const out = {}
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (!a.startsWith('--')) throw new Error(`알 수 없는 인자: ${a}`)
    const v = argv[++i]
    if (v === undefined) throw new Error(`${a} 에 값이 없습니다`)
    out[a.slice(2)] = v
  }
  return out
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2))
    if (!args.version || !args.dir) throw new Error('--version 과 --dir 이 필요합니다')
    const signatureText = args.sig && existsSync(args.sig) ? readFileSync(args.sig, 'utf8') : null
    const r = makeManifests({
      dir: args.dir,
      version: args.version,
      installer: args.installer,
      signatureText,
      migration: args['electron-migration'] ?? process.env.ELECTRON_MIGRATION
    })
    for (const [name, content] of Object.entries(r.files)) {
      writeFileSync(join(args.dir, name), content)
      console.log(`만듦: ${name}`)
    }
    for (const w of r.warnings) console.log(`::warning::${w}`)
    console.log(`설치본: ${r.fileName} (${r.size} 바이트, sha512 ${r.sha512.slice(0, 16)}…)`)
  } catch (e) {
    console.error(`::error::${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }
}

if (/make-update-manifests\.mjs$/.test(process.argv[1] ?? '')) main()
