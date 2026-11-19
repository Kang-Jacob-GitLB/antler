#!/usr/bin/env node
// Tauri 판 설치본에 동봉할 서드파티 라이선스 고지(npm + Rust)를 src-tauri/gen/licenses/THIRD_PARTY_NOTICES.txt 로 만든다(tauri.conf.json bundle.resources).
// npm: renderer 번들에 들어가는 라이브러리(gen-notices.mjs 와 같은 목록). Rust: `cargo metadata --locked` 의 정상 의존 트리(빌드 대상 플랫폼 기준)와
// 각 크레이트의 라이선스 파일(~/.cargo/registry/src). 새 도구는 쓰지 않는다. 결과 파일은 커밋하지 않는다(.gitignore).
// 사용: node scripts/gen-tauri-notices.mjs [--target <triple>]  (생략하면 TAURI_ENV_TARGET_TRIPLE, 그것도 없으면 rustc 의 host)
import { execFileSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { BUNDLED, NOTICE_HEADER, npmNotices, rustNotices, selectNormalCrates } from './lib/notices.mjs'

const ROOT = resolve(import.meta.dirname, '..')
const argIdx = process.argv.indexOf('--target')
let target = (argIdx >= 0 ? process.argv[argIdx + 1] : undefined) || process.env['TAURI_ENV_TARGET_TRIPLE']
if (!target) {
  const host = /^host: (\S+)$/m.exec(execFileSync('rustc', ['-vV'], { encoding: 'utf8' }))
  if (!host) throw new Error('rustc -vV 에서 host 를 찾을 수 없음')
  target = host[1]
}
const raw = execFileSync('cargo', ['metadata', '--locked', '--format-version', '1', '--filter-platform', target, '--manifest-path', resolve(ROOT, 'src-tauri', 'Cargo.toml')], {
  encoding: 'utf8',
  maxBuffer: 512 * 1024 * 1024
})
const rust = rustNotices(selectNormalCrates(JSON.parse(raw)))
if (rust.count === 0) throw new Error('Rust 의존 크레이트가 하나도 없다 — cargo metadata 해석 실패로 본다')
const dir = resolve(ROOT, 'src-tauri', 'gen', 'licenses')
mkdirSync(dir, { recursive: true })
const out = resolve(dir, 'THIRD_PARTY_NOTICES.txt')
const text = `${NOTICE_HEADER}${npmNotices(ROOT).join('\n')}\n${rust.text}`
writeFileSync(out, text)
if (rust.missing.length) console.warn(`라이선스 파일이 없는 크레이트 ${rust.missing.length}개: ${rust.missing.join(', ')}`)
console.log(`npm ${BUNDLED.length} + rust ${rust.count} crates (${rust.uniqueTexts} unique texts, target ${target}) → ${out} (${(Buffer.byteLength(text) / 1024).toFixed(0)} KiB)`)
