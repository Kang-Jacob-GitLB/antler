#!/usr/bin/env node
// 골든(tests/golden/fetch)에서 Rust 표 둘을 만든다. 둘 다 Node 의 Unicode 판에 묶이고 TS 가 기준이다.
//  - src-tauri/src/git/fetch/special_table.rs: 확인 창 표시(src/main/ipc/confirmText.ts)의 SPECIAL 정규식이 맞추는 코드 포인트 범위
//    (tests/goldenFetch.test.ts 가 0..=0x10FFFF 를 훑어 confirm.json 의 special 에 적는다)
//  - src-tauri/src/git/fetch/lowercase_table.rs: JS toLowerCase 의 코드 포인트별 변환표와 최종 시그마 판정에 쓰는 Cased·Case_Ignorable 범위
//    (config.json 의 lowercaseMap·cased·caseIgnorable). Rust std 의 소문자화는 Unicode 판이 달라 쓰지 않는다
// 순서: UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts && node scripts/gen-fetch-special.mjs
// Rust 테스트(src-tauri/tests/fetch_guard.rs)가 표와 골든이 같은지 본다.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const read = (name) => JSON.parse(readFileSync(join(root, 'tests/golden/fetch', name), 'utf8'))
const confirm = read('confirm.json')
const config = read('config.json')
const unicode = read('lists.json').runtime.unicode
const outDir = join(root, 'src-tauri/src/git/fetch')

const hex = (n) => `0x${n.toString(16)}`
const rangeLines = (ranges) => ranges.map(([lo, hi]) => `    (${hex(lo)}, ${hex(hi)}),`).join('\n')
const nonEmpty = (v, what) => {
  if (!Array.isArray(v) || v.length === 0) throw new Error(`${what} 가 비어 있다`)
  return v
}
const write = (file, text, what) => {
  mkdirSync(outDir, { recursive: true })
  writeFileSync(join(outDir, file), text)
  console.log(`${join(outDir, file)}: ${what}`)
}

const special = nonEmpty(confirm.special, 'confirm.json 의 special')
write(
  'special_table.rs',
  `// 생성 파일 — 직접 고치지 않는다. \`node scripts/gen-fetch-special.mjs\` 가 tests/golden/fetch/confirm.json 에서 만든다.
// TS confirmText.ts 의 SPECIAL 정규식이 맞추는 코드 포인트 범위(닫힌 구간, 오름차순). Node 의 Unicode ${unicode} 판에서 뽑았다.

/// 만든 Node 의 Unicode 판
pub const UNICODE_VERSION: &str = ${JSON.stringify(unicode)};

pub const SPECIAL_RANGES: &[(u32, u32)] = &[
${rangeLines(special)}
];
`,
  `${special.length} 범위`
)

const map = nonEmpty(config.lowercaseMap, 'config.json 의 lowercaseMap')
const cased = nonEmpty(config.cased, 'config.json 의 cased')
const ignorable = nonEmpty(config.caseIgnorable, 'config.json 의 caseIgnorable')
// Rust 문자열 리터럴: 모든 글자를 \u{…} 로 적는다(제어 문자·결합 문자가 소스에서 보이지 않게)
const lit = (s) => `"${Array.from(s, (c) => `\\u{${c.codePointAt(0).toString(16)}}`).join('')}"`
write(
  'lowercase_table.rs',
  `// 생성 파일 — 직접 고치지 않는다. \`node scripts/gen-fetch-special.mjs\` 가 tests/golden/fetch/config.json 에서 만든다.
// JS String.prototype.toLowerCase 의 코드 포인트별 변환표(Node 의 Unicode ${unicode} 판)와, 문맥 규칙(최종 시그마)에 쓰는 Cased·Case_Ignorable 범위.
// Rust std 의 to_lowercase 는 Unicode 판이 달라(예: U+A7CE) Node 가 모르는 쌍을 바꾸므로 쓰지 않는다.

/// (코드 포인트, 소문자화 결과) — 오름차순, 바뀌는 코드 포인트만. Σ(U+03A3)는 문맥이 없을 때의 값(σ)이다
pub const LOWERCASE_MAP: &[(u32, &str)] = &[
${map.map(([cp, l]) => `    (${hex(cp)}, ${lit(l)}),`).join('\n')}
];

/// \\p{Cased} (닫힌 구간, 오름차순)
pub const CASED_RANGES: &[(u32, u32)] = &[
${rangeLines(cased)}
];

/// \\p{Case_Ignorable} (닫힌 구간, 오름차순)
pub const CASE_IGNORABLE_RANGES: &[(u32, u32)] = &[
${rangeLines(ignorable)}
];
`,
  `${map.length} 변환, ${cased.length}·${ignorable.length} 범위`
)
