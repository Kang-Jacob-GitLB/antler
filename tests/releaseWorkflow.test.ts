// 발행 경로 워크플로(release.yml · nightly-release.yml)와 PR 게이트(build.yml · tauri.yml)의 정적 검사(Phase 6c PR 7).
// 워크플로는 머지 전에 실행할 수 없고(리허설 R 은 사람이 한다), 발행은 되돌릴 수 없다 — 그래서 "가드가 있다·순서가 맞다·시크릿이 한 곳에만 있다·필수 체크 이름이 그대로다"를 여기서 못 박는다.
// 가드의 판정 로직은 tests/releaseScripts.test.ts.
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseYaml } from './yamlLite'

const root = join(__dirname, '..')
const read = (...p: string[]): string => readFileSync(join(root, ...p), 'utf8')
const WORKFLOWS = join(root, '.github', 'workflows')

type Step = { name?: string; id?: string; uses?: string; run?: string; if?: string; env?: Record<string, string>; with?: Record<string, unknown> }
type Job = { name?: string; needs?: string | string[]; if?: string; uses?: string; with?: Record<string, unknown>; secrets?: unknown; permissions?: Record<string, string>; environment?: string; outputs?: Record<string, string>; steps: Step[] }
type Workflow = { name: string; on: Record<string, unknown>; permissions?: Record<string, string>; jobs: Record<string, Job> }

const text = (f: string): string => read('.github', 'workflows', f)
/** 주석을 뺀 본문 */
const code = (f: string): string => text(f).split('\n').filter((l) => !l.trimStart().startsWith('#')).join('\n')
const wf = (f: string): Workflow => parseYaml(text(f)) as Workflow
const job = (f: string, id: string): Job => {
  const j = wf(f).jobs[id]
  if (!j) throw new Error(`${f} 에 잡이 없다: ${id}`)
  return j
}
const stepIndex = (j: Job, name: string): number => {
  const i = j.steps.findIndex((s) => s.name === name)
  if (i < 0) throw new Error(`단계가 없다: ${name}`)
  return i
}

describe('release.yml — 태그에서 Tauri 설치본을 초안으로 발행한다', () => {
  const f = 'release.yml'

  it('잡은 preflight → package(tauri-package 재사용 호출) → release 세 개이고 Electron 빌드(matrix·electron-builder)가 없다', () => {
    expect(Object.keys(wf(f).jobs)).toEqual(['preflight', 'package', 'release'])
    expect(job(f, 'package').uses).toBe('./.github/workflows/tauri-package.yml')
    expect(job(f, 'package').needs).toBe('preflight')
    expect(job(f, 'release').needs).toEqual(['preflight', 'package'])
    expect(code(f)).not.toMatch(/matrix|electron-builder|electron-vite|ELECTRON_SKIP|pnpm build|pnpm dist|installers-\*|merge-multiple/)
    // 버전·커밋은 preflight 가 한 번 정해 뒤 잡이 같은 값을 쓴다(계산한 커밋 ≠ 빌드한 커밋이 되지 않게)
    expect(job(f, 'package').with).toEqual({ version: '${{ needs.preflight.outputs.version }}', sha: '${{ needs.preflight.outputs.sha }}' })
    expect(job(f, 'release').steps[0]?.with?.['ref']).toBe('${{ needs.preflight.outputs.sha }}')
  })

  it('트리거: semver 태그 push · 수동 실행 · 재사용 호출(version·sha·notes·draft)', () => {
    const on = wf(f).on as { push: { tags: string[] }; workflow_call: { inputs: Record<string, { required?: boolean }> } }
    expect(on.push.tags).toEqual(['v[0-9]+.[0-9]+.[0-9]+', 'v[0-9]+.[0-9]+.[0-9]+-*'])
    expect('workflow_dispatch' in on).toBe(true)
    expect(Object.keys(on.workflow_call.inputs)).toEqual(['version', 'sha', 'notes', 'draft'])
    for (const k of ['version', 'sha', 'notes']) expect(on.workflow_call.inputs[k]?.required, k).toBe(true)
  })

  it('시크릿은 이 파일에 없다 — secrets 를 넘기지도(inherit 포함) 참조하지도 않는다. 서명 키는 tauri-package 의 sign 잡이 Environment 로 받는다', () => {
    expect(code(f)).not.toMatch(/secrets\./)
    expect(job(f, 'package').secrets).toBeUndefined()
    for (const file of readdirSync(WORKFLOWS)) expect(code(file), file).not.toMatch(/secrets:\s*inherit/)
    // 시크릿을 참조하는 곳은 tauri-package.yml 의 서명 잡 두 단계뿐이다(릴리스 발행 잡의 GITHUB_TOKEN 은 secrets 가 아니라 자동 토큰이다)
    const referencing = readdirSync(WORKFLOWS).filter((file) => /secrets\./.test(code(file)))
    expect(referencing).toEqual(['tauri-package.yml'])
  })

  it('쓰기 권한은 release 잡 하나에만 있다(워크플로 기본은 contents: read)', () => {
    expect(wf(f).permissions).toEqual({ contents: 'read' })
    expect(job(f, 'release').permissions).toEqual({ contents: 'write' })
    expect(job(f, 'preflight').permissions).toBeUndefined()
    expect(job(f, 'package').permissions).toBeUndefined()
  })

  it('preflight 가 빌드·태그보다 앞이다: 발행하는 실행은 버전·업데이트 공개 키를 점검하고, 공개 키 없는 빌드는 발행하지 않는다', () => {
    const pre = job(f, 'preflight')
    const check = pre.steps.find((s) => s.run?.includes('scripts/check-release-preflight.mjs'))
    expect(check, '공개 키 점검 단계').toBeTruthy()
    expect(check?.if).toBe("steps.v.outputs.publish == 'true'")
    expect(check?.run).toContain('--version "$VERSION"')
    // 점검이 package 보다 먼저 끝난다(package 는 preflight 를 needs 로 건다 — 위 테스트). 발행 아닌 수동 실행(점검 빌드)은 키 없이도 돈다
    const version = pre.steps.find((s) => s.id === 'v')
    expect(version?.run).toContain('publish=false')
    expect(version?.run).toContain('VERSION="${REF_NAME#v}"') // 태그에서 버전을 뗀다(표현식으로는 접두사 v 를 못 뗀다)
  })

  it('초안: 태그 push 는 항상 초안이고, 재사용 호출은 입력(draft)을 따른다. 릴리스 액션이 그 값을 쓴다', () => {
    const version = job(f, 'preflight').steps.find((s) => s.id === 'v')
    const tagBranch = /elif \[ "\$REF_TYPE" = "tag" \]; then[\s\S]*?\nelse/.exec(version?.run ?? '')?.[0] ?? ''
    expect(tagBranch).toContain('draft=true')
    expect(/if \[ -n "\$INPUT_VERSION" \]; then[\s\S]*?elif/.exec(version?.run ?? '')?.[0]).toContain('draft="$INPUT_DRAFT"')
    const rel = job(f, 'release').steps.find((s) => s.uses?.startsWith('softprops/action-gh-release@'))
    expect(rel?.with?.['draft']).toBe("${{ needs.preflight.outputs.draft == 'true' }}")
    expect(rel?.with?.['fail_on_unmatched_files']).toBe(true)
    expect(rel?.with?.['prerelease']).toBe("${{ contains(env.TAG, '-') }}")
    expect(rel?.with?.['files']).toBe('installers/*\n')
  })

  it('발행 가드: 자산 점검(누락 시 실패)이 태그 만들기·push 와 릴리스 생성보다 앞이다', () => {
    const rel = job(f, 'release')
    const check = rel.steps[stepIndex(rel, '자산 점검 (누락 시 실패)')]
    expect(check?.run).toContain('scripts/check-release-assets.mjs --version "$VERSION" --dir installers --pubkey-from src-tauri/src/update/host.rs --electron-migration "$ELECTRON_MIGRATION"')
    // 중단 장치: 저장소 변수 ELECTRON_MIGRATION 은 점검에도 전달된다(꺼진 동안 latest.yml 이 없어도 통과해야 이후 릴리스가 막히지 않는다)
    expect(check?.env).toEqual({ ELECTRON_MIGRATION: '${{ vars.ELECTRON_MIGRATION }}' })
    expect(stepIndex(rel, '자산 점검 (누락 시 실패)')).toBeLessThan(stepIndex(rel, '태그 만들기·push (재사용 호출)'))
    expect(stepIndex(rel, '태그 만들기·push (재사용 호출)')).toBeLessThan(stepIndex(rel, '릴리스 생성 + 설치본 첨부 (Windows / macOS / Linux)'))
    // 아티팩트는 이름으로 하나만 받는다(점검이 예상 밖 파일을 거부한다)
    const dl = rel.steps.find((s) => s.uses?.startsWith('actions/download-artifact@'))
    expect(dl?.with).toEqual({ name: 'tauri-installers', path: 'installers' })
    // 태그를 push 하는 단계는 하나뿐이고 그 앞에 점검이 있다
    expect([...code(f).matchAll(/git push/g)]).toHaveLength(1)
    expect(rel.if).toBe("needs.preflight.outputs.publish == 'true'")
  })

  it('메이저 노트(docs/release-notes/v<버전>.md)를 태그 메시지 앞에 붙인다', () => {
    const rel = job(f, 'release')
    const tag = rel.steps[stepIndex(rel, '태그 만들기·push (재사용 호출)')]
    expect(tag?.run).toContain('docs/release-notes/$TAG.md')
    expect(tag?.if).toBe('inputs.version')
  })
})

describe('공급망·캐시', () => {
  it('모든 워크플로의 외부 액션이 40자 SHA 로 고정돼 있다(로컬 재사용 호출 ./ 는 제외)', () => {
    for (const file of readdirSync(WORKFLOWS)) {
      const uses = [...code(file).matchAll(/^\s*(?:- )?uses:\s*(\S+)/gm)].map((m) => m[1]!)
      expect(uses.length, file).toBeGreaterThan(0)
      for (const u of uses) if (!u.startsWith('./')) expect(u, `${file}: ${u}`).toMatch(/^[\w.-]+\/[\w./-]+@[0-9a-f]{40}$/)
    }
  })

  it('시크릿·쓰기 권한 잡은 캐시를 복원하지 않는다: release.yml 전체·nightly 에 cache 가 없고, tauri-package 에서는 서명 잡(environment)에 없다', () => {
    for (const f of ['release.yml', 'nightly-release.yml']) expect(code(f), f).not.toMatch(/actions\/cache|\bcache:/)
    const pkg = wf('tauri-package.yml')
    for (const [id, j] of Object.entries(pkg.jobs)) {
      if (j.environment || j.permissions) expect(JSON.stringify(j), id).not.toMatch(/actions\/cache|"cache"/)
    }
    expect(pkg.jobs['sign']?.environment).toBe('release')
  })

  it('초안 강제 이중화: preflight 가 정책 스크립트를 부르고 draft 출력이 호출자 값 또는 강제값이다(태그 전체를 받는다)', () => {
    const pre = job('release.yml', 'preflight')
    expect(pre.steps[0]?.with?.['fetch-depth']).toBe(0)
    const step = pre.steps.find((s) => s.id === 'policy')
    expect(step?.run).toContain('scripts/release-draft-policy.mjs --version "$VERSION" --tags "$TAGS" --notes "$NOTES"')
    expect(pre.outputs?.['draft']).toBe("${{ steps.v.outputs.draft == 'true' || steps.policy.outputs.force == 'true' }}")
  })

  it('야간 수동 입력(inputs.bump)은 셸에 끼워 넣지 않고 env 로 받는다', () => {
    const next = job('nightly-release.yml', 'plan').steps.find((s) => s.id === 'next')
    expect(next?.env).toEqual({ BUMP: "${{ inputs.bump || 'auto' }}" })
    expect(next?.run).toContain('--bump "$BUMP"')
    expect(code('nightly-release.yml')).not.toMatch(/run:[^\n]*\$\{\{\s*inputs\./)
  })
})

describe('nightly-release.yml — 메이저는 초안으로, 초안이 공개 대기인 동안은 발행하지 않는다', () => {
  const f = 'nightly-release.yml'

  it('plan 잡에 공개 대기 가드가 있다: 출력 release 가 frozen 일 때 false 다', () => {
    const plan = job(f, 'plan')
    const step = plan.steps[stepIndex(plan, '공개 대기 확인 (직전 정식 릴리스가 초안인가)')]
    expect(step?.id).toBe('freeze')
    expect(step?.run).toContain('gh release view "$LAST" --json isDraft')
    expect(step?.run).toContain('node scripts/release-freeze.mjs --last "$LAST" --rc "$RC" --out "$OUT"')
    expect(step?.env?.['GH_TOKEN']).toBe('${{ github.token }}')
    expect(plan.outputs?.['release']).toBe("${{ steps.next.outputs.release == 'true' && steps.freeze.outputs.frozen == 'false' }}")
    // 계산(next) → 가드(freeze) → 요약 순서
    expect(stepIndex(plan, '다음 버전 계산')).toBeLessThan(stepIndex(plan, '공개 대기 확인 (직전 정식 릴리스가 초안인가)'))
    expect(stepIndex(plan, '공개 대기 확인 (직전 정식 릴리스가 초안인가)')).toBeLessThan(stepIndex(plan, '요약'))
    expect(wf(f).permissions).toEqual({ contents: 'read' })
  })

  it('메이저 올림이면 초안으로 발행한다: bump 를 출력으로 내보내 release.yml 의 draft 입력으로 넘긴다', () => {
    expect(job(f, 'plan').outputs?.['draft']).toBe("${{ steps.next.outputs.bump == 'major' }}")
    expect(code(f)).toContain('bump=${r.bump ?? ""}')
    const rel = job(f, 'release')
    expect(rel.uses).toBe('./.github/workflows/release.yml')
    expect(rel.with?.['draft']).toBe("${{ needs.plan.outputs.draft == 'true' }}")
    expect(rel.if).toBe("needs.plan.outputs.release == 'true' && inputs.dry_run != true")
    expect(rel.permissions).toEqual({ contents: 'write' })
    expect(rel.secrets).toBeUndefined()
  })
})

describe('PR 게이트 — 필수 체크 이름은 그대로다(브랜치 룰셋이 이름으로 걸려 있어 바꾸면 PR 머지가 막힌다)', () => {
  it("build.yml 의 잡 이름은 'Typecheck · Test · Build' 이고 내용은 Tauri 프런트 기준이다", () => {
    const j = job('build.yml', 'check')
    expect(j.name).toBe('Typecheck · Test · Build')
    expect(wf('build.yml').on).toHaveProperty('pull_request')
    const runs = j.steps.map((s) => s.run).filter((r): r is string => !!r)
    expect(runs).toEqual(['pnpm install --frozen-lockfile', 'pnpm typecheck', expect.stringContaining('pnpm test'), 'pnpm build', 'node scripts/check-contrast.mjs'])
    expect(code('build.yml')).not.toMatch(/electron|ELECTRON/i)
  })

  it("tauri.yml 의 잡 이름은 'Rust · 골든 · clippy' 이고 모든 PR 에서 돈다(paths 필터 없음)", () => {
    expect(job('tauri.yml', 'rust').name).toBe('Rust · 골든 · clippy')
    expect(wf('tauri.yml').on).toHaveProperty('pull_request')
    expect(code('tauri.yml')).not.toMatch(/^ {2}pull_request:\n {4}paths/m)
    const test = job('tauri.yml', 'rust').steps.find((s) => s.run?.includes('cargo test'))
    expect(test?.run).toContain('--features test-hooks')
    const clippy = job('tauri.yml', 'rust').steps.find((s) => s.name === 'clippy')
    expect([...(clippy?.run ?? '').matchAll(/-D warnings/g)]).toHaveLength(2)
    // 관련 변경 목록에 Electron 소스(src/main·src/sidecar)가 없다
    expect(code('tauri.yml')).not.toMatch(/\^src\/main|\^src\/sidecar/)
  })
})

describe('Electron 제거(Phase 6c PR 7)', () => {
  it('Electron 코드·빌드 설정·의존이 저장소에 없다', () => {
    for (const gone of ['src/main', 'src/preload', 'electron.vite.config.mts', 'electron-builder.yml', 'scripts/lib/electronHost.mjs', 'scripts/audit-ui.mjs']) {
      expect(existsSync(join(root, gone)), gone).toBe(false)
    }
    const pkg = JSON.parse(read('package.json')) as { version: string; main?: string; scripts: Record<string, string>; dependencies?: Record<string, string>; devDependencies: Record<string, string> }
    expect(pkg.version).toBe('0.0.0')
    expect(pkg.main).toBeUndefined()
    const deps = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies })
    for (const d of ['electron', 'electron-builder', 'electron-vite', 'electron-updater', 'ssh2', '@types/ssh2']) expect(deps, d).not.toContain(d)
    expect(Object.keys(pkg.scripts).filter((s) => /^dist|electron/.test(s))).toEqual([])
    expect(pkg.scripts['dev']).toBe('tauri dev')
    expect(pkg.scripts['build']).toMatch(/^vite build/)
  })

  it('tauri.conf.json 의 프런트 빌드와 package.json 의 빌드 스크립트가 같은 설정을 쓴다', () => {
    const conf = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { build: { beforeBuildCommand: string; beforeDevCommand: string } }
    const pkg = JSON.parse(read('package.json')) as { scripts: Record<string, string> }
    expect(conf.build.beforeBuildCommand).toBe(pkg.scripts['build'])
    expect(conf.build.beforeDevCommand).toBe('pnpm tauri:web')
    expect(existsSync(join(root, 'vite.config.mts'))).toBe(true)
    expect(read('vite.config.mts')).not.toMatch(/electron/i)
  })
})
