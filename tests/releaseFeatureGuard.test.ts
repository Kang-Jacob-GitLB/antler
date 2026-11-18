// 배포 빌드 feature 가드(scripts/check-release-features.mjs)와 패키징 워크플로(.github/workflows/tauri-package.yml)의 정적 검사.
// 워크플로는 이 PR 머지 전에는 실행할 수 없다(workflow_dispatch 는 기본 브랜치의 파일만 돈다) — 여기서는 "발행하지 않는다"는 불변식과 가드 연결만 본다.
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { createRequire } from 'node:module'
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

  type Step = { name?: string; uses?: string; run?: string; if?: string; env?: Record<string, string>; with?: Record<string, unknown> }
  type Job = { environment?: string; env?: Record<string, string>; needs?: string | string[]; steps: Step[] }
  const yaml = createRequire(createRequire(import.meta.url).resolve('electron-builder'))('js-yaml') as { load(s: string): unknown }
  const jobs = (yaml.load(wf) as { jobs: Record<string, Job> }).jobs
  const job = (id: string): Job => {
    const j = jobs[id]
    if (!j) throw new Error(`잡이 없다: ${id}`)
    return j
  }
  const stepsWithSecrets = (job: Job): string[] => job.steps.filter((s) => JSON.stringify(s).includes('secrets.')).map((s) => s.name ?? '')

  it('서명 시크릿은 서명 잡의 두 단계(키 존재 확인·서명)에만 있고, 빌드 잡·빌드 단계에는 없다', () => {
    // 시크릿을 참조하는 줄: 존재 확인 단계(KEY) 하나, 서명 단계(키·암호) 둘 — 다른 곳에는 없다
    const uses = lines.filter((l) => !l.trimStart().startsWith('#') && l.includes('secrets.')).map((l) => l.trim())
    expect(uses).toEqual([
      'KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}',
      'TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}',
      'TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}'
    ])
    for (const [id, job] of Object.entries(jobs)) {
      if (id === 'windows-sign') continue
      expect(JSON.stringify(job), `${id} 잡에 시크릿이 있다`).not.toContain('secrets.')
      expect(job.environment, `${id} 잡이 Environment 를 선언한다`).toBeUndefined()
    }
    const build = job('windows')
    expect(JSON.stringify(build)).not.toMatch(/SIGNING|createUpdaterArtifacts|tauri\.updater\.conf/)
    const buildStep = build.steps.find((s) => s.run?.includes('tauri build'))
    expect(buildStep?.run?.trim()).toBe('pnpm tauri build --bundles nsis')
    expect(buildStep?.env).toBeUndefined()
    expect(build.env).toEqual({ VERSION: '${{ needs.version.outputs.version }}' })
  })

  it('windows-sign 잡: 키는 서명 단계에서만 쓰이고, 빌드·설치 스크립트가 도는 단계에는 없다', () => {
    const sign = job('windows-sign')
    expect(sign.environment).toBe('release')
    expect(sign.needs).toEqual(['version', 'windows'])
    expect(sign.env).toEqual({ VERSION: '${{ needs.version.outputs.version }}' })
    expect(stepsWithSecrets(sign)).toEqual(['서명 키 확인', '업데이트 서명'])
    const idx = (name: string): number => sign.steps.findIndex((s) => s.name === name)
    const stepAt = (name: string): Step => {
      const s = sign.steps[idx(name)]
      if (!s) throw new Error(`단계가 없다: ${name}`)
      return s
    }
    const install = stepAt('Tauri CLI 설치')
    // 키가 주어지는 단계보다 앞에서 설치하고, 설치 스크립트는 돌리지 않는다. 이 잡에는 빌드가 없다
    expect(install.run).toBe('pnpm install --frozen-lockfile --ignore-scripts')
    expect(idx('Tauri CLI 설치')).toBeLessThan(idx('서명 키 확인'))
    expect(idx('서명 키 확인')).toBeLessThan(idx('업데이트 서명'))
    expect(sign.steps.some((s) => /tauri build|cargo|rustup/.test(s.run ?? ''))).toBe(false)
    const step = stepAt('업데이트 서명')
    expect(step.if).toBe("steps.key.outputs.signed == 'true'")
    expect(Object.keys(step.env ?? {}).sort()).toEqual(['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'])
    // 서명에 버전을 묶는다(requireSignedVersion). 매니페스트 생성은 서명 뒤, 키 없는 단계에서
    expect(step.run).toContain('tauri signer sign --app-version "$VERSION"')
    const manifest = stepAt('업데이트 매니페스트 생성·검증')
    expect(idx('업데이트 매니페스트 생성·검증')).toBeGreaterThan(idx('업데이트 서명'))
    expect(manifest.env).toEqual({ ELECTRON_MIGRATION: '${{ vars.ELECTRON_MIGRATION }}' })
    expect(manifest.run).toContain('scripts/make-update-manifests.mjs')
  })

  it('cdp·test-hooks 를 켜지 않는다', () => {
    // 단계 이름("… feature 검사 (cdp · test-hooks 없음)")은 빼고 본다
    expect(code.replace(/^\s*- name:.*$/gm, '')).not.toMatch(/--features|cdp|test-hooks/)
    expect(code).not.toMatch(/set -x|xtrace/)
  })

  it('세 갈래가 모두 빌드 뒤 feature 가드를 부른다', () => {
    expect([...code.matchAll(/node scripts\/check-release-features\.mjs/g)].length).toBeGreaterThanOrEqual(2)
  })

  it('버전 검사는 문자열 전체를 본다([[ =~ ]], 줄 단위 grep 이 아니다)', () => {
    const check = job('version').steps.map((s) => s.run ?? '').join('\n')
    expect(check).toContain('[[ "$VERSION" =~ ^')
    expect(check).not.toContain('grep -E')
  })

  it('업데이트 서명은 빌드가 아니라 사후 서명이다: 덮어쓰기 설정이 없고 기본 설정에도 업데이터 설정이 없다(공개 키 대기)', () => {
    expect(existsSync(join(root, 'src-tauri', 'tauri.updater.conf.json'))).toBe(false)
    const base = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { plugins?: unknown; bundle: Record<string, unknown> }
    expect(base.plugins).toBeUndefined()
    expect(base.bundle.createUpdaterArtifacts).toBeUndefined()
  })
})
