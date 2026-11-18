#!/usr/bin/env node
// 배포용으로 빌드한 실행 파일에 개발·시험 전용 feature(cdp, test-hooks)가 들어가지 않았는지 검사한다(.github/workflows/tauri-package.yml).
// 방법: 그 feature 아래에서만 존재하는 문자열이 바이너리에 있는지 찾는다. 있으면 exit 1.
//   cdp         — WebView2 원격 디버깅 포트를 여는 코드(src-tauri/src/lib.rs 의 browser_args)가 읽는 환경 변수 이름
//   test-hooks  — 같은 lib.rs 가 test-hooks 빌드에서 시작할 때 출력하는 표식(그 feature 에는 따로 고유한 문자열이 없다)
// 한계: "없음"을 확인하는 검사라 문자열이 바이너리에서 사라지면 조용히 통과한다. cdp 표식은 실제 cdp 빌드로 존재를 확인했고
//   (docs/tauri.md), test-hooks 표식이 릴리스 프로파일에서 살아남는지는 아직 확인하지 못했다.
// 사용: node scripts/check-release-features.mjs <바이너리 경로>
import { readFileSync } from 'node:fs'

/** feature 이름 → 바이너리에 있으면 안 되는 문자열. tests/releaseFeatureGuard.test.ts 가 이 값이 Rust 소스에 그대로 있는지 대조한다 */
export const FORBIDDEN = [
  { feature: 'cdp', marker: 'ANTLER_CDP_PORT' },
  { feature: 'test-hooks', marker: 'antler-build-feature:test-hooks' }
]

/** 바이트 열에서 금지 표식을 찾아 [{feature, marker}] 로 돌려준다 */
export function findForbidden(bytes) {
  return FORBIDDEN.filter(({ marker }) => bytes.includes(Buffer.from(marker, 'utf8')))
}

function main() {
  const file = process.argv[2]
  if (!file) {
    console.error('::error::사용: node scripts/check-release-features.mjs <바이너리 경로>')
    process.exit(2)
  }
  let bytes
  try {
    bytes = readFileSync(file)
  } catch (e) {
    console.error(`::error::바이너리를 읽지 못했습니다: ${file} (${e instanceof Error ? e.message : String(e)})`)
    process.exit(2)
  }
  if (bytes.length < 1_000_000) {
    // 비정상적으로 작으면 엉뚱한 파일을 검사한 것이다(없음 검사가 공허하게 통과하지 않게)
    console.error(`::error::바이너리가 너무 작습니다(${bytes.length} 바이트): ${file}`)
    process.exit(2)
  }
  const hits = findForbidden(bytes)
  if (hits.length > 0) {
    for (const h of hits) console.error(`::error::배포 빌드에 ${h.feature} feature 가 들어 있습니다(표식 ${h.marker}): ${file}`)
    process.exit(1)
  }
  console.log(`배포 빌드 feature 검사 통과(${FORBIDDEN.map((f) => f.feature).join(', ')} 없음): ${file}`)
}

if (/check-release-features\.mjs$/.test(process.argv[1] ?? '')) main()
