#!/usr/bin/env node
// src-tauri/src/git/fetch/special_table.rs 를 만든다. 확인 창 표시(src/main/ipc/confirmText.ts)의 SPECIAL 정규식이 맞추는 코드 포인트 범위는
// tests/goldenFetch.test.ts 가 0..=0x10FFFF 를 훑어 골든 tests/golden/fetch/confirm.json 의 special 에 적는다 — 이 스크립트는 그 골든을 Rust 표로 옮긴다.
// 순서: UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts && node scripts/gen-fetch-special.mjs
// Rust 테스트(src-tauri/tests/fetch_guard.rs)가 표와 골든이 같은지 본다.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const golden = JSON.parse(readFileSync(join(root, 'tests/golden/fetch/confirm.json'), 'utf8'))
const lists = JSON.parse(readFileSync(join(root, 'tests/golden/fetch/lists.json'), 'utf8'))
const ranges = golden.special
if (!Array.isArray(ranges) || ranges.length === 0) throw new Error('confirm.json 에 special 범위가 없다')

const hex = (n) => `0x${n.toString(16)}`
const lines = ranges.map(([lo, hi]) => `    (${hex(lo)}, ${hex(hi)}),`)
const text = `// 생성 파일 — 직접 고치지 않는다. \`node scripts/gen-fetch-special.mjs\` 가 tests/golden/fetch/confirm.json 에서 만든다.
// TS confirmText.ts 의 SPECIAL 정규식이 맞추는 코드 포인트 범위(닫힌 구간, 오름차순). Node 의 Unicode ${lists.runtime.unicode} 판에서 뽑았다.

/// 만든 Node 의 Unicode 판
pub const UNICODE_VERSION: &str = ${JSON.stringify(lists.runtime.unicode)};

pub const SPECIAL_RANGES: &[(u32, u32)] = &[
${lines.join('\n')}
];
`
const out = join(root, 'src-tauri/src/git/fetch/special_table.rs')
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, text)
console.log(`${out}: ${ranges.length} 범위`)
