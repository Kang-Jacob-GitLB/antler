// 배포 빌드 feature 가드(scripts/check-release-features.mjs)와 패키징 워크플로(.github/workflows/tauri-package.yml)의 정적 검사.
// 워크플로는 이 PR 머지 전에는 실행할 수 없다(workflow_dispatch 는 기본 브랜치의 파일만 돈다) — 여기서는 "발행하지 않는다"는 불변식과 가드 연결만 본다.
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import { FORBIDDEN, findForbidden } from '../scripts/check-release-features.mjs'

const root = join(__dirname, '..')
const read = (...p: string[]): string => readFileSync(join(root, ...p), 'utf8')

function rustSources(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) rustSources(p, out)
    else if (e.name.endsWith('.rs')) out.push(p)
  }
  return out
}

describe('check-release-features', () => {
  it('금지 표식이 있으면 찾고, 없으면 통과한다', () => {
    const clean = Buffer.concat([Buffer.alloc(100), Buffer.from('ANTLER_USER_DATA remote'), Buffer.alloc(100)])
    expect(findForbidden(clean)).toEqual([])
    const dirty = Buffer.concat([clean, Buffer.from('xx ANTLER_CDP_PORT yy')])
    expect(findForbidden(dirty).map((h: { feature: string }) => h.feature)).toEqual(['cdp'])
    const both = Buffer.concat([dirty, Buffer.from('antler-build-feature:test-hooks')])
    expect(findForbidden(both).map((h: { feature: string }) => h.feature)).toEqual(['cdp', 'test-hooks'])
  })

  it('표식이 Rust 소스에 그대로 있다(이름을 바꿔도 가드가 조용히 공허해지지 않게)', () => {
    const src = rustSources(join(root, 'src-tauri', 'src')).map((f) => readFileSync(f, 'utf8')).join('\n')
    for (const { marker } of FORBIDDEN as { feature: string; marker: string }[]) expect(src, marker).toContain(`"${marker}"`)
  })

  it('cdp 표식은 cdp 코드 안에서만, test-hooks 표식은 test-hooks 아래에서만 나온다', () => {
    const lib = read('src-tauri', 'src', 'lib.rs')
    expect(lib).toMatch(/#\[cfg\(any\(debug_assertions, feature = "cdp"\)\)\]\s*\n\s*if let Ok\(port\) = std::env::var\("ANTLER_CDP_PORT"\)/)
    expect(lib).toMatch(/#\[cfg\(feature = "test-hooks"\)\]\s*\nconst FEATURE_MARKER_TEST_HOOKS: &str = "antler-build-feature:test-hooks";/)
    expect(lib).toMatch(/#\[cfg\(feature = "test-hooks"\)\]\s*\n\s*eprintln!\("\{FEATURE_MARKER_TEST_HOOKS\}"\);/)
  })

  it('Cargo 의 기본 feature 에 cdp·test-hooks 가 없다', () => {
    const cargo = read('src-tauri', 'Cargo.toml')
    expect(cargo).not.toMatch(/^default\s*=/m)
  })
})

describe('tauri-package.yml', () => {
  const wf = read('.github', 'workflows', 'tauri-package.yml')
  const lines = wf.split('\n')
  const code = lines.filter((l) => !l.trimStart().startsWith('#')).join('\n')

  it('발행하지 않는다: 쓰기 권한·릴리스 액션·태그·push 가 없다', () => {
    expect(code).toMatch(/^permissions:\n {2}contents: read$/m)
    expect(code).not.toMatch(/contents:\s*write|action-gh-release|gh release|git tag|git push|actions\/create-release/)
  })

  it('수동 실행(workflow_dispatch)만 쓰고 release.yml 과 이어지지 않는다', () => {
    expect(code).toMatch(/^on:\n {2}workflow_dispatch:/m)
    expect(code).not.toMatch(/^ {2}(push|pull_request|workflow_call|schedule):/m)
    for (const f of readdirSync(join(root, '.github', 'workflows'))) {
      if (f === 'tauri-package.yml') continue
      expect(read('.github', 'workflows', f), f).not.toContain('tauri-package')
    }
  })

  it('서명 시크릿은 빌드 단계의 env 에만 들어가고 cdp·test-hooks 를 켜지 않는다', () => {
    // 시크릿 이름이 나오는 줄: 존재 확인 단계(KEY) 하나, 빌드 단계(키·암호) 둘
    const uses = lines.filter((l) => !l.trimStart().startsWith('#') && l.includes('secrets.')).map((l) => l.trim())
    expect(uses).toEqual([
      'KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}',
      'TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}',
      'TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}'
    ])
    // 단계 이름("… feature 검사 (cdp · test-hooks 없음)")은 빼고 본다
    expect(code.replace(/^\s*- name:.*$/gm, '')).not.toMatch(/--features|cdp|test-hooks/)
    expect(code).not.toMatch(/set -x|xtrace/)
  })

  it('세 갈래가 모두 빌드 뒤 feature 가드를 부르고, 서명 설정은 시크릿이 있을 때만 쓴다', () => {
    expect([...code.matchAll(/node scripts\/check-release-features\.mjs/g)].length).toBeGreaterThanOrEqual(2)
    expect(code).toContain('if: steps.key.outputs.signed == \'true\'')
    const signedStep = code.slice(code.indexOf('빌드 (NSIS, 업데이트 서명)'), code.indexOf('빌드 (NSIS, 서명 없음)'))
    expect(signedStep).toContain('--config src-tauri/tauri.updater.conf.json')
    expect(code.slice(code.indexOf('빌드 (NSIS, 서명 없음)'), code.indexOf('배포 빌드 feature 검사'))).not.toContain('tauri.updater.conf.json')
  })

  it('저장소 변수 ELECTRON_MIGRATION 을 매니페스트 스크립트에 넘긴다', () => {
    expect(code).toContain('ELECTRON_MIGRATION: ${{ vars.ELECTRON_MIGRATION }}')
  })

  it('서명 덮어쓰기 설정은 createUpdaterArtifacts 만 켜고, 기본 설정에는 업데이터 설정이 없다(공개 키 대기)', () => {
    expect(JSON.parse(read('src-tauri', 'tauri.updater.conf.json')).bundle).toEqual({ createUpdaterArtifacts: true })
    const base = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { plugins?: unknown; bundle: Record<string, unknown> }
    expect(base.plugins).toBeUndefined()
    expect(base.bundle.createUpdaterArtifacts).toBeUndefined()
  })
})
