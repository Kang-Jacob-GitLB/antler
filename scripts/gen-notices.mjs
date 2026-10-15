#!/usr/bin/env node
// renderer 번들에 들어가는 라이브러리의 라이선스 고지를 resources/licenses/THIRD_PARTY_NOTICES.txt 로 모은다.
// 압축된 번들에서는 @license 주석이 지워지므로 배포물에 별도 파일로 동봉한다(electron-builder extraResources).
// main 의 ssh2 와 그 의존 패키지는 app.asar 의 node_modules 에 LICENSE 째로 실리므로 여기서 다루지 않는다.
// 폰트(Pretendard·나눔고딕코딩, OFL 1.1)는 같은 폴더의 *-LICENSE.txt 로 따로 둔다.
// 사용: node scripts/gen-notices.mjs (의존성 버전을 올린 뒤 다시 실행)
import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..')
const BUNDLED = ['react', 'react-dom', 'scheduler', 'zustand', '@tanstack/react-virtual', '@tanstack/virtual-core', 'highlight.js']

/** 패키지 디렉터리를 찾는다(간접 의존은 그것을 쓰는 패키지 기준으로 푼다) */
function pkgDir(name) {
  const bases = [ROOT, dirname(createRequire(resolve(ROOT, 'package.json')).resolve('react-dom/package.json')),
    dirname(createRequire(resolve(ROOT, 'package.json')).resolve('@tanstack/react-virtual/package.json'))]
  for (const b of bases) {
    try {
      return dirname(createRequire(resolve(b, 'package.json')).resolve(`${name}/package.json`))
    } catch {
      // 다음 기준으로
    }
  }
  throw new Error(`패키지를 찾을 수 없음: ${name}`)
}

const parts = []
for (const name of BUNDLED) {
  const dir = pkgDir(name)
  const pkg = JSON.parse(readFileSync(resolve(dir, 'package.json'), 'utf8'))
  const file = readdirSync(dir).find((f) => /^(licen[cs]e|copying)(\.|$)/i.test(f))
  if (!file) throw new Error(`${name}: 라이선스 파일 없음`)
  const text = readFileSync(resolve(dir, file), 'utf8').trim()
  parts.push(`${'='.repeat(72)}\n${pkg.name}@${pkg.version} — ${pkg.license}\n${'='.repeat(72)}\n\n${text}\n`)
}
const header = 'Antler 에 포함된 서드파티 소프트웨어의 라이선스 고지입니다.\n' +
  '폰트 라이선스는 같은 폴더의 Pretendard-LICENSE.txt, NanumGothicCoding-LICENSE.txt 를 보세요.\n\n'
const out = resolve(ROOT, 'resources', 'licenses', 'THIRD_PARTY_NOTICES.txt')
writeFileSync(out, header + parts.join('\n'))
console.log(`${BUNDLED.length} notices → ${out}`)
