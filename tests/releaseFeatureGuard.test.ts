// 배포 빌드 feature 가드(scripts/check-release-features.mjs)와 패키징 워크플로(.github/workflows/tauri-package.yml)의 정적 검사.
// 워크플로는 머지 전에는 실행할 수 없다(workflow_dispatch 는 기본 브랜치의 파일만 돈다) — 여기서는 "이 워크플로는 발행하지 않는다"는 불변식과 가드 연결만 본다.
// 발행 경로(release.yml·nightly-release.yml)의 정적 검사는 tests/releaseWorkflow.test.ts.
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseYaml } from './yamlLite'
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
    // test-hooks 표식은 build_marker.rs 에 있고(lib.rs 는 시험 훅 이름을 두지 않는다 — src-tauri/tests/api_route.rs), lib.rs 는 시작할 때 부르기만 한다
    const marker = read('src-tauri', 'src', 'build_marker.rs')
    expect(marker).toMatch(/#\[cfg\(feature = "test-hooks"\)\]\s*\nconst FEATURE_MARKER_TEST_HOOKS: &str = "antler-build-feature:test-hooks";/)
    expect(marker).toMatch(/#\[cfg\(feature = "test-hooks"\)\]\s*\n\s*eprintln!\("\{FEATURE_MARKER_TEST_HOOKS\}"\);/)
    expect(lib).toMatch(/pub fn run\(\) \{\s*\n\s*build_marker::announce\(\);/)
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

  it('수동 실행(workflow_dispatch)과 재사용 호출(workflow_call)만 쓰고, 부르는 곳은 release.yml 하나다', () => {
    expect(code).toMatch(/^on:\n {2}workflow_dispatch:/m)
    expect(code).toMatch(/^ {2}workflow_call:/m)
    expect(code).not.toMatch(/^ {2}(push|pull_request|schedule):/m)
    for (const f of readdirSync(join(root, '.github', 'workflows'))) {
      if (f === 'tauri-package.yml') continue
      // 부르는 곳(`uses:`)만 센다 — tauri.yml 의 변경 감지 목록처럼 이름을 적기만 하는 것은 호출이 아니다
      const calls = /^\s*uses:\s*\.\/\.github\/workflows\/tauri-package\.yml\s*$/m.test(read('.github', 'workflows', f))
      expect(calls, f).toBe(f === 'release.yml')
    }
  })

  it('재사용 호출은 계산한 커밋(inputs.sha)을 빌드한다 — 세 잡의 모든 checkout 이 ref 를 준다', () => {
    const checkouts = Object.values(jobs).flatMap((j) => j.steps.filter((s) => s.uses?.startsWith('actions/checkout@')))
    expect(checkouts).toHaveLength(3)
    for (const s of checkouts) expect(s.with?.['ref']).toBe('${{ inputs.sha || github.sha }}')
    const on = (parseYaml(wf) as { on: { workflow_call: { inputs: Record<string, { required?: boolean }> } } }).on
    expect(Object.keys(on.workflow_call.inputs).sort()).toEqual(['sha', 'version'])
    expect(on.workflow_call.inputs['sha']?.required).toBe(true)
    expect(on.workflow_call.inputs['version']?.required).toBe(true)
  })

  type Step = { name?: string; uses?: string; run?: string; if?: string; env?: Record<string, string>; with?: Record<string, unknown> }
  type Job = { environment?: string; env?: Record<string, string>; needs?: string | string[]; steps: Step[] }
  const jobs = (parseYaml(wf) as { jobs: Record<string, Job> }).jobs
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
      if (id === 'sign') continue
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

  it('sign 잡(Windows·macOS·Linux 서명): 키는 서명 단계에서만 쓰이고, 빌드·설치 스크립트가 도는 단계에는 없다', () => {
    const sign = job('sign')
    expect(sign.environment).toBe('release')
    expect(sign.needs).toEqual(['version', 'windows', 'unix'])
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
    expect(install.run).toBe('pnpm install --frozen-lockfile --ignore-scripts --store-dir "$RUNNER_TEMP/pnpm-store"')
    // 서명 잡은 빌드 잡이 저장한 캐시를 복원하지 않는다(오염된 Tauri CLI 바이너리가 서명 키를 읽는 길) — setup-node 에 cache 가 없다
    const node = sign.steps.find((s) => s.uses?.startsWith('actions/setup-node@'))
    expect(node?.with).toEqual({ 'node-version': '22' })
    expect(JSON.stringify(sign)).not.toMatch(/actions\/cache|cache:|cache-dependency/)
    expect(idx('Tauri CLI 설치')).toBeLessThan(idx('서명 키 확인'))
    expect(idx('서명 키 확인')).toBeLessThan(idx('업데이트 서명'))
    expect(sign.steps.some((s) => /tauri build|cargo|rustup/.test(s.run ?? ''))).toBe(false)
    const step = stepAt('업데이트 서명')
    expect(step.if).toBe("steps.key.outputs.signed == 'true'")
    expect(Object.keys(step.env ?? {}).sort()).toEqual(['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'])
    // 서명에 버전을 묶는다(requireSignedVersion). 매니페스트 생성은 서명 뒤, 키 없는 단계에서
    expect(step.run).toContain('tauri signer sign --app-version "$VERSION"')
    // macOS dmg·Linux deb(수동 경로 자산)도 같은 단계에서 서명한다. unix 갈래가 실패해도 Windows 서명은 막지 않는다
    for (const name of ['Antler-${VERSION}-arm64.dmg', 'Antler-${VERSION}-x64.dmg', 'antler_${VERSION}_amd64.deb', 'Antler-${VERSION}-arm64.app.tar.gz', 'Antler-${VERSION}-x64.app.tar.gz'])
      expect(step.run, name).toContain(name)
    expect((sign as { if?: string }).if).toBe("${{ !cancelled() && needs.windows.result == 'success' }}")
    const manifest = stepAt('업데이트 매니페스트 생성·검증')
    expect(idx('업데이트 매니페스트 생성·검증')).toBeGreaterThan(idx('업데이트 서명'))
    expect(manifest.env).toEqual({ ELECTRON_MIGRATION: '${{ vars.ELECTRON_MIGRATION }}' })
    expect(manifest.run).toContain('scripts/make-update-manifests.mjs')
    expect(manifest.run).toContain('--signed "${{ steps.key.outputs.signed }}"')

    // 서명 입력의 출처 고정: 아티팩트마다 다른 입력 폴더(in/…)에 받고(merge-multiple·pattern 없음), out/ 에 쓰는 것은 명시 복사 단계뿐이다
    const downloads = sign.steps.filter((s) => s.uses?.startsWith('actions/download-artifact'))
    expect(downloads.length).toBe(4)
    const paths = downloads.map((s) => String(s.with?.['path']))
    expect(new Set(paths).size).toBe(paths.length)
    for (const s of downloads) {
      expect(String(s.with?.['path']), '입력 폴더').toMatch(/^in\/[a-z0-9-]+$/)
      expect(s.with).not.toHaveProperty('merge-multiple')
      expect(s.with).not.toHaveProperty('pattern')
    }
    const copy = stepAt('서명 입력 복사')
    expect(idx('서명 입력 복사')).toBeLessThan(idx('서명 키 확인'))
    expect(copy.run).toContain('cp --no-dereference')
    expect(copy.run).toContain('[ -L "$dir" ] || [ -L "$f" ]')
    expect(copy.run).toContain('-gt 300000000')
    expect([...(copy.run ?? '').matchAll(/^\s*copy_one (\S+) "([^"]+)" (yes|no)$/gm)].map((m) => `${m[1]} ${m[2]} ${m[3]}`)).toEqual([
      'in/win Antler-Setup-${VERSION}-x64.exe yes',
      'in/mac-arm64 Antler-${VERSION}-arm64.dmg no',
      'in/mac-x64 Antler-${VERSION}-x64.dmg no',
      'in/linux antler_${VERSION}_amd64.deb no',
      'in/mac-arm64 Antler-${VERSION}-arm64.app.tar.gz no',
      'in/mac-x64 Antler-${VERSION}-x64.app.tar.gz no'
    ])
    // out/ 를 만들거나 복사로 채우는 곳은 복사 단계뿐이다(서명·매니페스트 단계는 고정 이름·스크립트만)
    for (const s of sign.steps) {
      if (s === copy) continue
      // 명령어만 본다(파일 이름 `….app.tar.gz` 의 `tar` 는 명령이 아니다 — 앞이 `.` 인 단어는 뺀다)
      expect(s.run ?? '', s.name).not.toMatch(/(?<![.\w-])(mkdir|cp|mv|ln|unzip|tar)\b/)
      if (s.uses?.startsWith('actions/download-artifact')) expect(String(s.with?.['path']), '다운로드가 out/ 에 쓴다').not.toMatch(/^out(\/|$)/)
    }
    expect(step.run ?? '').not.toContain('*')
    const upload = sign.steps.find((s) => s.uses?.startsWith('actions/upload-artifact'))
    expect(String(upload?.with?.['path'])).not.toContain('out/*')
    for (const line of String(upload?.with?.['path']).split('\n').filter(Boolean)) expect(line).toMatch(/^out\/(Antler-Setup-\$\{\{ env\.VERSION \}\}-x64\.exe|Antler-\$\{\{ env\.VERSION \}\}-(arm64|x64)\.(dmg|app\.tar\.gz)|antler_\$\{\{ env\.VERSION \}\}_amd64\.deb)(\.sig)?$|^out\/latest\.(json|yml)$/)
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

  it('업데이트 서명은 빌드가 아니라 사후 서명이다: 덮어쓰기 설정이 없고 conf 에도 업데이터 설정이 없다(플러그인 설정·공개 키는 lib.rs 가 UPDATE_PUBKEY 한 곳으로 만든다)', () => {
    expect(existsSync(join(root, 'src-tauri', 'tauri.updater.conf.json'))).toBe(false)
    const base = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { plugins?: unknown; bundle: Record<string, unknown> }
    // 6c PR 4: 앱이 플러그인을 단다. 그러나 설정(공개 키 포함)은 conf 가 아니라 Rust 가 넣는다 — conf 에 공개 키가 생기면 두 곳이 되고, CLI 가 개인 키를 요구하는지도 알 수 없다
    expect(base.plugins).toBeUndefined()
    expect(base.bundle.createUpdaterArtifacts).toBeUndefined()
    expect(read('src-tauri', 'src', 'lib.rs')).toContain('updater_plugin_config(update::host::UPDATE_PUBKEY)')
  })

  it('공개 키가 비어 있는 동안 배포 빌드는 업데이트를 내려받지도 설치하지도 않는다(fail-closed)', () => {
    const host = read('src-tauri', 'src', 'update', 'host.rs')
    // 키는 상수 한 곳이다. 비었거나(None) minisign 공개 키 한 줄(base64)이다
    const m = /pub const UPDATE_PUBKEY: Option<&str> = (None|Some\("([A-Za-z0-9+/=]+)"\));/.exec(host)
    expect(m, 'UPDATE_PUBKEY 상수의 모양').not.toBeNull()
    // 내려받기는 네트워크를 열기 **전에** 키를 확인하고, 설치도 키가 없으면 설치 포트(Engine::install)에 닿지 않는다
    const download = host.slice(host.indexOf('async fn download('), host.indexOf('fn install(&self)'))
    expect(download.indexOf('key_ready()')).toBeGreaterThan(-1)
    expect(download.indexOf('key_ready()')).toBeLessThan(download.indexOf('download_installer('))
    const install = host.slice(host.indexOf('fn install(&self)'), host.indexOf('async fn release_notes('))
    expect(install.indexOf('key_ready()')).toBeGreaterThan(-1)
    expect(install.indexOf('key_ready()')).toBeLessThan(install.indexOf('engine.install('))
    expect(host).toContain('업데이트 서명 키가 설정되지 않았습니다')
    // 키가 있을 때 형식: 두 줄(설명·`RW…` 56자)의 minisign 공개 키를 base64 로 감싼 값
    if (m![2]) {
      const text = Buffer.from(m![2], 'base64').toString('utf8')
      expect(text.split('\n')[0]).toMatch(/^untrusted comment: minisign public key/)
      expect(text.split('\n')[1]).toMatch(/^RW[A-Za-z0-9+/]{54}$/)
    }
    // 서명 시험 표본은 이름으로 시험 전용임이 드러나고, 비밀 키 파일은 저장소에 없다
    const keys = readdirSync(join(root, 'src-tauri', 'tests', 'fixtures', 'update_keys'))
    expect(keys.length).toBeGreaterThan(0)
    for (const f of keys) expect(f).toMatch(/^TEST-ONLY-.*\.(pub|sig)$/)
  })
})

describe('tauri.yml', () => {
  const wf = read('.github', 'workflows', 'tauri.yml')
  const code = wf.split('\n').filter((l) => !l.trimStart().startsWith('#')).join('\n')
  type Step = { name?: string; uses?: string; run?: string }
  type Job = { name?: string; 'runs-on'?: string; steps: Step[]; strategy?: { matrix: { os: string[] } } }
  const jobs = (parseYaml(wf) as { jobs: Record<string, Job> }).jobs

  it('필수 체크 잡(rust)의 이름과 러너는 그대로다 — 브랜치 룰셋이 이름으로 건다', () => {
    expect(jobs['rust']?.name).toBe('Rust · 골든 · clippy')
    expect(jobs['rust']?.['runs-on']).toBe('windows-latest')
  })

  it('권한은 contents: read 뿐이고 시크릿·쓰기 권한이 없다', () => {
    expect(code).toMatch(/^permissions:\n {2}contents: read$/m)
    expect(code).not.toMatch(/contents:\s*write|secrets\.|id-token|environment:/)
  })

  it('cross 잡: ubuntu-22.04·macos-latest 에서 check·clippy 를 돌리고, 모든 액션은 40자 SHA 로 고정한다', () => {
    const cross = jobs['cross']
    expect(cross?.name).not.toBe('Rust · 골든 · clippy')
    expect(cross?.strategy?.matrix.os).toEqual(['ubuntu-22.04', 'macos-latest'])
    const run = cross?.steps.map((s) => s.run ?? '').join('\n') ?? ''
    expect(run).toContain('cargo check --locked --all-targets --manifest-path src-tauri/Cargo.toml')
    expect(run).toContain('--features test-hooks')
    expect(run).toContain('-- -D warnings')
    expect(run).toContain('libwebkit2gtk-4.1-dev')
    for (const [id, job] of Object.entries(jobs)) {
      for (const s of job.steps) if (s.uses) expect(s.uses, `${id} 잡의 액션이 SHA 고정이 아니다`).toMatch(/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}$/)
    }
  })
})
