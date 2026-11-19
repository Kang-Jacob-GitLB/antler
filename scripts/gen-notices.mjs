#!/usr/bin/env node
// renderer 번들에 들어가는 라이브러리의 라이선스 고지를 resources/licenses/THIRD_PARTY_NOTICES.txt 로 모은다.
// 압축된 번들에서는 @license 주석이 지워지므로 배포물에 별도 파일로 동봉한다(electron-builder extraResources).
// main 의 ssh2 와 그 의존 패키지는 app.asar 의 node_modules 에 LICENSE 째로 실리므로 여기서 다루지 않는다.
// 폰트(Pretendard·나눔고딕코딩, OFL 1.1)는 같은 폴더의 *-LICENSE.txt 로 따로 둔다.
// Tauri 판은 이 목록에 Rust 크레이트를 더해 gen-tauri-notices.mjs 가 만든다.
// 사용: node scripts/gen-notices.mjs (의존성 버전을 올린 뒤 다시 실행)
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { BUNDLED, NOTICE_HEADER, npmNotices } from './lib/notices.mjs'

const ROOT = resolve(import.meta.dirname, '..')
const out = resolve(ROOT, 'resources', 'licenses', 'THIRD_PARTY_NOTICES.txt')
writeFileSync(out, NOTICE_HEADER + npmNotices(ROOT).join('\n'))
console.log(`${BUNDLED.length} notices → ${out}`)
