#!/usr/bin/env node
// 릴리스에 첨부할 자산 폴더를 발행 직전에 점검한다(.github/workflows/release.yml — 태그를 만들기 **전에** 돈다). 하나라도 어긋나면 exit 1.
// tauri-package.yml 은 점검용 수동 실행에서도 쓰이므로 서명 키가 없거나 macOS·Linux 갈래가 실패해도 경고만 하고 끝난다 — 그 느슨함이 발행으로 새지 않게 여기서 막는다:
//   필수 자산(설치본 + .sig) 전부 · latest.json(다섯 플랫폼 키가 전부, 키마다 url 이 그 자산을 가리킨다) · latest.yml(저장소 변수 ELECTRON_MIGRATION 이 꺼지지 않았다면)
//   · 자산 목록 밖의 파일이 없다(예상 밖 파일이 릴리스에 딸려 가지 않게).
// 사용: node scripts/check-release-assets.mjs --version <버전> --dir <자산 폴더> [--electron-migration <값>]
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { DOWNLOAD_PREFIX, installerName, isSafeFileName, MANUAL_ASSETS, migrationEnabled, sha512Base64 } from './make-update-manifests.mjs'

/** latest.json 이 담아야 하는 플랫폼 키(Windows 는 같은 항목 둘 + 수동 경로 셋) */
export const REQUIRED_PLATFORM_KEYS = ['windows-x86_64-nsis', 'windows-x86_64', ...MANUAL_ASSETS.map((a) => a.key)]

/** 이 릴리스에 올라가야 하는 파일 이름 전부. `latest.yml` 은 이관이 꺼져(ELECTRON_MIGRATION=off 등) 있으면 빠진다 */
export function expectedAssets(version, migration) {
  const installers = [installerName(version), ...MANUAL_ASSETS.map((a) => a.name(version))]
  const names = [...installers, ...installers.map((n) => `${n}.sig`), 'latest.json']
  if (migrationEnabled(migration)) names.push('latest.yml')
  return names
}

/**
 * 폴더의 파일 목록과 내용을 점검한다. `readText(name)` 은 텍스트 파일 읽기, `sizeOf(name)` 은 바이트 수, `sha512Of(name)` 은 실물의 sha512(base64).
 * 문제 목록(문자열)을 돌려준다 — 비어 있으면 통과.
 */
export function checkAssets({ version, migration, names, readText, sizeOf, sha512Of }) {
  const problems = []
  const want = expectedAssets(version, migration)
  for (const n of want) if (!names.includes(n)) problems.push(`첨부할 자산이 없습니다: ${n}`)
  for (const n of names) if (!want.includes(n)) problems.push(`예상하지 않은 파일입니다: ${n}`)
  for (const n of want) {
    if (names.includes(n) && !(sizeOf(n) > 0)) problems.push(`크기가 0 입니다: ${n}`)
  }
  for (const n of want.filter((w) => w.endsWith('.sig') && names.includes(w))) {
    if (!/^[A-Za-z0-9+/=]+$/.test(readText(n).trim())) problems.push(`서명 형식이 올바르지 않습니다: ${n}`)
  }

  if (names.includes('latest.json')) {
    let doc
    try {
      doc = JSON.parse(readText('latest.json'))
    } catch {
      problems.push('latest.json 이 JSON 이 아닙니다')
    }
    if (doc) {
      if (doc.version !== version) problems.push(`latest.json 의 버전(${doc.version})이 ${version} 과 다릅니다`)
      for (const key of REQUIRED_PLATFORM_KEYS) {
        const p = doc.platforms?.[key]
        if (!p) {
          problems.push(`latest.json 에 ${key} 항목이 없습니다`)
          continue
        }
        const file = key.startsWith('windows') ? installerName(version) : MANUAL_ASSETS.find((a) => a.key === key).name(version)
        if (p.url !== `${DOWNLOAD_PREFIX}v${version}/${file}`) problems.push(`latest.json ${key} 의 url 이 ${file} 을 가리키지 않습니다`)
        if (names.includes(`${file}.sig`) && p.signature !== readText(`${file}.sig`).trim()) problems.push(`latest.json ${key} 의 서명이 ${file}.sig 와 다릅니다`)
      }
      for (const key of Object.keys(doc.platforms ?? {})) if (!REQUIRED_PLATFORM_KEYS.includes(key)) problems.push(`latest.json 에 예상하지 않은 플랫폼 키가 있습니다: ${key}`)
    }
  }

  if (names.includes('latest.yml')) {
    const yml = readText('latest.yml')
    const file = installerName(version)
    const sha = names.includes(file) ? sha512Of(file) : null
    if (!yml.startsWith(`version: ${version}\n`)) problems.push(`latest.yml 의 버전이 ${version} 이 아닙니다`)
    if (!yml.includes(`\n  - url: ${file}\n`) || !yml.includes(`\npath: ${file}\n`)) problems.push(`latest.yml 이 ${file} 을 (파일 이름만으로) 가리키지 않습니다`)
    if (sha && (!yml.includes(`\n    sha512: ${sha}\n`) || !yml.includes(`\nsha512: ${sha}\n`))) problems.push('latest.yml 의 sha512 가 설치본 실물과 다릅니다')
    if (names.includes(file) && !yml.includes(`\n    size: ${sizeOf(file)}\n`)) problems.push('latest.yml 의 size 가 설치본 실물과 다릅니다')
  }
  return problems
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
    if (!args.version || !args.dir) throw new Error('--version 과 --dir 이 필요합니다')
    const entries = readdirSync(args.dir, { withFileTypes: true })
    const names = entries.filter((e) => e.isFile()).map((e) => e.name)
    const problems = entries.filter((e) => !e.isFile()).map((e) => `파일이 아닌 항목입니다: ${e.name}`)
    for (const n of names) if (!isSafeFileName(n)) problems.push(`안전하지 않은 파일 이름입니다: ${n}`)
    problems.push(
      ...checkAssets({
        version: args.version,
        migration: args['electron-migration'],
        names,
        readText: (n) => readFileSync(join(args.dir, n), 'utf8'),
        sizeOf: (n) => statSync(join(args.dir, n)).size,
        sha512Of: (n) => sha512Base64(readFileSync(join(args.dir, n)))
      })
    )
    for (const p of problems) console.error(`::error::${p}`)
    if (problems.length) process.exit(1)
    console.log(`자산 점검 통과: ${names.length}개 (${args.version}${migrationEnabled(args['electron-migration']) ? '' : ', latest.yml 없음 — ELECTRON_MIGRATION 꺼짐'})`)
  } catch (e) {
    console.error(`::error::${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }
}

if (/check-release-assets\.mjs$/.test(process.argv[1] ?? '')) main()
