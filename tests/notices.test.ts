// 서드파티 라이선스 고지 생성(scripts/lib/notices.mjs)과 Tauri 번들·워크플로 연결의 시험.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { parseYaml } from './yamlLite'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import { BUNDLED, npmSection, readLicenseFiles, rustNotices, selectNormalCrates } from '../scripts/lib/notices.mjs'

const root = join(__dirname, '..')
const read = (...p: string[]): string => readFileSync(join(root, ...p), 'utf8')

type Pkg = { id: string; name: string; version: string; license: string | null; license_file: string | null; source: string | null; manifest_path: string }
type Dep = { pkg: string; dep_kinds: { kind: string | null }[] }

let tmp: string
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), 'antler-notices-'))
})
afterEach(() => rmSync(tmp, { recursive: true, force: true }))

function crate(name: string, files: Record<string, string>, extra: Partial<Pkg> = {}): Pkg {
  const dir = join(tmp, `${name}-1.0.0`)
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'Cargo.toml'), '')
  for (const [f, text] of Object.entries(files)) writeFileSync(join(dir, f), text)
  return { id: `${name} 1.0.0`, name, version: '1.0.0', license: 'MIT', license_file: null, source: 'registry+x', manifest_path: join(dir, 'Cargo.toml'), ...extra }
}

describe('readLicenseFiles', () => {
  it('LICENSE-MIT·LICENSE-APACHE·COPYING·NOTICE 를 이름순으로 모두 읽고 다른 파일·빈 파일은 건너뛴다', () => {
    const p = crate('multi', { 'LICENSE-MIT': 'mit text\r\n', 'LICENSE-APACHE': 'apache text', COPYING: 'copying', NOTICE: 'notice', 'README.md': 'x', 'LICENSE.empty': '  ', 'licensed.rs': 'no' })
    const files = readLicenseFiles(join(tmp, 'multi-1.0.0')) as { name: string; text: string }[]
    expect(files.map((f) => f.name)).toEqual(['COPYING', 'LICENSE-APACHE', 'LICENSE-MIT', 'NOTICE'])
    expect(files.find((f) => f.name === 'LICENSE-MIT')?.text).toBe('mit text')
    expect(p.name).toBe('multi')
  })
})

describe('npmSection', () => {
  it('라이선스 파일이 둘이면 파일 이름 머리글로 이어 붙이고, 없으면 실패한다', () => {
    const p = crate('npmpkg', { LICENSE_MIT: 'mit', 'LICENSE_APACHE-2.0': 'apache' })
    const section = npmSection({ name: '@x/api', version: '2.0.0', license: 'Apache-2.0 OR MIT' }, join(tmp, 'npmpkg-1.0.0')) as string
    expect(section).toContain('@x/api@2.0.0 — Apache-2.0 OR MIT')
    expect(section).toContain('--- LICENSE_APACHE-2.0 ---\napache')
    expect(section).toContain('--- LICENSE_MIT ---\nmit')
    const empty = crate('nolic', {})
    expect(() => npmSection({ name: 'n', version: '1', license: 'MIT' }, join(tmp, 'nolic-1.0.0'))).toThrow(/라이선스 파일 없음/)
    expect(p.id).not.toBe(empty.id)
  })

  it('renderer 가 실제로 import 하는 라이브러리(marked·@tauri-apps/api 포함)를 목록에 둔다', () => {
    for (const name of ['react', 'react-dom', 'zustand', '@tanstack/react-virtual', 'highlight.js', 'marked', '@tauri-apps/api']) expect(BUNDLED).toContain(name)
  })
})

describe('selectNormalCrates', () => {
  const pkg = (name: string, source: string | null = 'registry+x'): Pkg => ({ id: name, name, version: '1.0.0', license: 'MIT', license_file: null, source, manifest_path: '' })
  const dep = (p: string, ...kinds: (string | null)[]): Dep => ({ pkg: p, dep_kinds: kinds.map((kind) => ({ kind })) })

  it('정상 의존만 따라간다: build·dev 전용은 빼고, 루트와 경로 의존도 뺀다', () => {
    const meta = {
      packages: [pkg('app', null), pkg('a'), pkg('b'), pkg('buildonly'), pkg('devonly'), pkg('both'), pkg('local', null), pkg('c')],
      workspace_members: ['app'],
      resolve: {
        root: 'app',
        nodes: [
          { id: 'app', deps: [dep('a', null), dep('buildonly', 'build'), dep('devonly', 'dev'), dep('both', 'build', null), dep('local', null)] },
          { id: 'a', deps: [dep('b', null)] },
          { id: 'b', deps: [dep('a', null)] }, // 순환
          { id: 'buildonly', deps: [dep('c', null)] },
          { id: 'both', deps: [] }
        ]
      }
    }
    expect((selectNormalCrates(meta) as Pkg[]).map((p) => p.name)).toEqual(['a', 'b', 'both'])
  })
})

describe('rustNotices', () => {
  it('같은 본문은 한 번만 싣고, 라이선스 파일·license 필드가 없는 크레이트도 실패하지 않고 목록에 적는다', () => {
    const a = crate('alpha', { LICENSE: 'shared text' })
    const b = crate('beta', { LICENSE: 'shared text' })
    const c = crate('gamma', { 'LICENSE-MIT': 'mit', 'LICENSE-APACHE': 'apache' }, { license: 'MIT OR Apache-2.0' })
    const d = crate('delta', {}, { license: null })
    const e = crate('epsilon', { 'COPYING.txt': 'ignored?' }, { license: null, license_file: 'custom.txt' })
    writeFileSync(join(tmp, 'epsilon-1.0.0', 'custom.txt'), 'custom text')
    const r = rustNotices([a, b, c, d, e]) as { text: string; count: number; missing: string[]; uniqueTexts: number }
    expect(r.count).toBe(5)
    expect(r.missing).toEqual(['delta@1.0.0'])
    expect(r.text).toContain('delta@1.0.0 — (license 필드 없음) [라이선스 파일 없음]')
    expect(r.text).toContain('gamma@1.0.0 — MIT OR Apache-2.0')
    expect(r.text).toContain('epsilon@1.0.0 — (license_file: custom.txt)')
    expect(r.text.match(/shared text/g)).toHaveLength(1)
    expect(r.text).toContain('다음 크레이트에 적용(2):\nalpha@1.0.0, beta@1.0.0')
    expect(r.text).toContain('--- LICENSE-APACHE ---\napache\n\n--- LICENSE-MIT ---\nmit')
    expect(r.text).toContain('custom text')
    expect(r.uniqueTexts).toBe(3)
  })
})

describe('Tauri 번들·워크플로 연결', () => {
  const conf = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { build: { beforeBuildCommand: string }; bundle: { resources: Record<string, string> } }

  it('tauri.conf.json 의 bundle.resources 가 고지 파일과 폰트 라이선스를 licenses/ 아래에 싣는다', () => {
    expect(conf.bundle.resources).toEqual({
      'gen/licenses/THIRD_PARTY_NOTICES.txt': 'licenses/THIRD_PARTY_NOTICES.txt',
      '../resources/licenses/Pretendard-LICENSE.txt': 'licenses/Pretendard-LICENSE.txt',
      '../resources/licenses/NanumGothicCoding-LICENSE.txt': 'licenses/NanumGothicCoding-LICENSE.txt'
    })
    for (const src of Object.keys(conf.bundle.resources).filter((s) => s.startsWith('../'))) expect(() => readFileSync(join(root, 'src-tauri', src))).not.toThrow()
  })

  it('생성물은 커밋하지 않고(.gitignore), build.rs 가 없을 때 자리 표시 파일을 둔다', () => {
    expect(read('.gitignore')).toMatch(/^src-tauri\/gen\/licenses\/$/m)
    expect(read('src-tauri', 'build.rs')).toContain('gen/licenses/THIRD_PARTY_NOTICES.txt')
    expect(read('src-tauri', 'build.rs').indexOf('ensure_notices_placeholder();')).toBeLessThan(read('src-tauri', 'build.rs').indexOf('tauri_build::build();'))
  })

  it('tauri build 가 beforeBuildCommand 에서 고지를 먼저 만든다', () => {
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> }
    expect(pkg.scripts['tauri:notices']).toBe('node scripts/gen-tauri-notices.mjs')
    expect(conf.build.beforeBuildCommand.startsWith('pnpm tauri:notices && ')).toBe(true)
  })

  it('tauri-package.yml 의 모든 빌드 잡이 빌드 앞에서 고지를 만들고, 그 단계에는 시크릿이 없다', () => {
    // yamlLite: js-yaml 은 Electron 의존(electron-builder)과 함께 사라졌다
    type Step = { name?: string; run?: string }
    const jobs = (parseYaml(read('.github', 'workflows', 'tauri-package.yml')) as { jobs: Record<string, { steps: Step[] }> }).jobs
    for (const id of ['windows', 'unix']) {
      const steps = jobs[id]?.steps ?? []
      const gen = steps.findIndex((s) => s.run?.includes('pnpm tauri:notices'))
      const build = steps.findIndex((s) => s.run?.includes('tauri build'))
      expect(gen, `${id}: 고지 단계`).toBeGreaterThan(-1)
      expect(gen, `${id}: 빌드보다 앞`).toBeLessThan(build)
      expect(JSON.stringify(steps[gen])).not.toContain('secrets.')
    }
    // 의존 크레이트를 받아야 하므로 Rust 설치 뒤에 온다
    const win = jobs['windows']?.steps ?? []
    expect(win.findIndex((s) => s.name === 'Rust stable')).toBeLessThan(win.findIndex((s) => s.run?.includes('pnpm tauri:notices')))
    // macOS 는 대상 아키텍처의 의존만 모은다
    const unix = jobs['unix']?.steps.find((s) => s.run?.includes('pnpm tauri:notices'))
    expect(unix?.run).toContain('--target ${{ matrix.target }}')
  })
})
