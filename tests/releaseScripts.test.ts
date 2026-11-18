// 발행 가드 스크립트의 판정 로직(Phase 6c PR 7): scripts/check-release-preflight.mjs · check-release-assets.mjs · release-freeze.mjs.
// 워크플로가 이 스크립트를 어떤 순서로 부르는지는 tests/releaseWorkflow.test.ts. 여기서는 "어긋나면 실패한다"를 실제 CLI(종료 코드)로 본다.
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as A from '../scripts/check-release-assets.mjs'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as F from '../scripts/release-freeze.mjs'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as M from '../scripts/make-update-manifests.mjs'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as P from '../scripts/check-release-preflight.mjs'

const root = join(__dirname, '..')
const SIG = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpSVVE9PQ=='
const NOW = new Date('2026-11-18T00:00:00.000Z')
const tmps: string[] = []
afterEach(() => {
  for (const d of tmps.splice(0)) rmSync(d, { recursive: true, force: true })
})
const tmp = (): string => {
  const d = mkdtempSync(join(tmpdir(), 'antler-release-'))
  tmps.push(d)
  return d
}
const node = (script: string, ...args: string[]): { status: number | null; stderr: string; stdout: string } => {
  const r = spawnSync(process.execPath, [join(root, 'scripts', script), ...args], { encoding: 'utf8' })
  return { status: r.status, stderr: r.stderr, stdout: r.stdout }
}

describe('check-release-preflight — 공개 키 없는 빌드의 발행을 막는다', () => {
  const host = (decl: string): string => `//! 문서\npub const OTHER: u8 = 1;\n${decl}\n`
  const GOOD = readFileSync(join(root, 'src-tauri', 'tests', 'fixtures', 'update_keys', 'TEST-ONLY-pubkey-a.pub'), 'utf8').trim()
  const b64 = (s: string): string => Buffer.from(s, 'utf8').toString('base64')

  it('지금 저장소의 host.rs 는 UPDATE_PUBKEY 가 None 이라 실패한다(공개 키를 넣기 전에는 발행 불가) — 키를 넣으면 이 시험은 통과 쪽으로 바뀐다', () => {
    const src = readFileSync(join(root, 'src-tauri', 'src', 'update', 'host.rs'), 'utf8')
    const key = P.readPubkey(src)
    const r = node('check-release-preflight.mjs', '--version', '2.0.0')
    if (key === null) {
      expect(r.status).toBe(1)
      expect(r.stderr).toContain('UPDATE_PUBKEY 가 None 입니다')
    } else {
      // 키가 들어간 뒤: 형식이 맞아야 하고 시험용 키가 아니어야 한다
      expect(P.checkPubkey(src)).toEqual({ ok: true })
      expect(r.status).toBe(0)
    }
  })

  it('None·없음·base64 아님·minisign 이 아닌 내용·시험용 키·잘린 키 줄은 거부하고, 올바른 공개 키는 통과한다', () => {
    expect(P.checkPubkey(host('pub const UPDATE_PUBKEY: Option<&str> = None;')).ok).toBe(false)
    expect(P.checkPubkey(host('// 없음')).ok).toBe(false)
    expect(P.checkPubkey(host('pub const UPDATE_PUBKEY: Option<&str> = Some("not base64!");')).ok).toBe(false)
    expect(P.checkPubkey(host(`pub const UPDATE_PUBKEY: Option<&str> = Some("${b64('hello')}");`)).ok).toBe(false)
    const text = Buffer.from(GOOD, 'base64').toString('utf8')
    expect(P.checkPubkey(host(`pub const UPDATE_PUBKEY: Option<&str> = Some("${GOOD}");`))).toEqual({ ok: true })
    expect(P.checkPubkey(host(`pub const UPDATE_PUBKEY: Option<&str> = Some("${b64(text.replace('public key:', 'public key: TEST-ONLY'))}");`)).ok).toBe(false)
    expect(P.checkPubkey(host(`pub const UPDATE_PUBKEY: Option<&str> = Some("${b64(text.replace(/RW\S{20}/, 'RW'))}");`)).ok).toBe(false)
    // 주석 처리된 선언은 읽지 않는다
    expect(P.readPubkey('// pub const UPDATE_PUBKEY: Option<&str> = Some("AAAA");\npub const UPDATE_PUBKEY: Option<&str> = None;')).toBeNull()
  })

  it('버전이 semver 가 아니면 실패한다', () => {
    for (const v of ['v2.0.0', '2.0', '', '02.0.0', '2.0.0+b']) expect(P.checkVersion(v).ok, v).toBe(false)
    for (const v of ['2.0.0', '2.0.0-rc.1', '10.20.30']) expect(P.checkVersion(v).ok, v).toBe(true)
    expect(node('check-release-preflight.mjs', '--version', 'v2.0.0').status).toBe(1)
  })
})

describe('check-release-assets — 자산이 하나라도 빠지면 태그를 만들기 전에 실패한다', () => {
  const VERSION = '2.0.0'
  const INSTALLERS = [M.installerName(VERSION), ...M.MANUAL_ASSETS.map((a: { name: (v: string) => string }) => a.name(VERSION))]

  /** 설치본·.sig·latest.json·latest.yml 을 tauri-package.yml 산출물과 같은 모양으로 만든다 */
  function fullDir(migration?: string): string {
    const dir = tmp()
    for (const n of INSTALLERS) {
      writeFileSync(join(dir, n), `bytes of ${n}`)
      writeFileSync(join(dir, `${n}.sig`), `${SIG}\n`)
    }
    const r = M.makeManifests({ dir, version: VERSION, signatureText: SIG, signed: true, migration, now: NOW })
    for (const [name, content] of Object.entries(r.files)) writeFileSync(join(dir, name), content as string)
    return dir
  }
  const run = (dir: string, migration = ''): { status: number | null; stderr: string } => node('check-release-assets.mjs', '--version', VERSION, '--dir', dir, '--electron-migration', migration)

  it('완전한 산출물(Windows·macOS 둘·Linux + 서명 + 두 매니페스트)은 통과한다', () => {
    const r = run(fullDir())
    expect(r.stderr).toBe('')
    expect(r.status).toBe(0)
  })

  it('필수 자산 하나를 뺄 때마다 실패한다 — 설치본·.sig·latest.json·latest.yml 전부', () => {
    for (const missing of A.expectedAssets(VERSION, undefined) as string[]) {
      const dir = fullDir()
      rmSync(join(dir, missing))
      const r = run(dir)
      expect(r.status, missing).toBe(1)
      expect(r.stderr, missing).toContain(missing)
    }
  })

  it('서명 키가 없던 빌드(서명·latest.json 이 없는 산출물)와 macOS·Linux 갈래가 빠진 산출물은 발행하지 못한다', () => {
    // tauri-package.yml 이 점검용으로는 통과시키는 모양을 그대로 만든다
    const dir = tmp()
    writeFileSync(join(dir, M.installerName(VERSION)), 'installer')
    const r = M.makeManifests({ dir, version: VERSION, signatureText: null, signed: false, now: NOW })
    for (const [name, content] of Object.entries(r.files)) writeFileSync(join(dir, name), content as string)
    const out = run(dir)
    expect(out.status).toBe(1)
    expect(out.stderr).toContain('.sig')
    expect(out.stderr).toContain('latest.json')
  })

  it('latest.json 의 플랫폼 키 누락·url 불일치·서명 불일치·버전 불일치를 잡는다', () => {
    const tamper = (mutate: (doc: { version: string; platforms: Record<string, { url: string; signature: string }> }) => void): { status: number | null; stderr: string } => {
      const dir = fullDir()
      const doc = JSON.parse(readFileSync(join(dir, 'latest.json'), 'utf8'))
      mutate(doc)
      writeFileSync(join(dir, 'latest.json'), JSON.stringify(doc))
      return run(dir)
    }
    expect(tamper((d) => delete d.platforms['darwin-aarch64-dmg']).stderr).toContain('darwin-aarch64-dmg 항목이 없습니다')
    expect(tamper((d) => (d.platforms['linux-x86_64-deb']!.url = d.platforms['linux-x86_64-deb']!.url.replace('.deb', '.x'))).stderr).toContain('linux-x86_64-deb')
    expect(tamper((d) => (d.platforms['windows-x86_64']!.signature = 'AAAA')).stderr).toContain('서명이')
    expect(tamper((d) => (d.version = '2.0.1')).stderr).toContain('버전')
    expect(tamper((d) => (d.platforms['android'] = { url: 'x', signature: 'y' })).stderr).toContain('예상하지 않은 플랫폼 키')
  })

  it('latest.yml 이 설치본 실물(sha512·size)과 다르거나 파일 이름이 아닌 주소를 담으면 실패한다', () => {
    const dir = fullDir()
    const yml = readFileSync(join(dir, 'latest.yml'), 'utf8')
    writeFileSync(join(dir, 'latest.yml'), yml.replace(/sha512: \S+/g, `sha512: ${'A'.repeat(86)}==`))
    expect(run(dir).stderr).toContain('sha512')
    const dir2 = fullDir()
    writeFileSync(join(dir2, 'latest.yml'), readFileSync(join(dir2, 'latest.yml'), 'utf8').replace(/size: \d+/, 'size: 1'))
    expect(run(dir2).stderr).toContain('size')
    const dir3 = fullDir()
    writeFileSync(join(dir3, 'latest.yml'), readFileSync(join(dir3, 'latest.yml'), 'utf8').replace(`url: ${M.installerName(VERSION)}`, `url: https://github.com/x/${M.installerName(VERSION)}`))
    expect(run(dir3).stderr).toContain('파일 이름만')
  })

  it('예상하지 않은 파일(릴리스에 딸려 갈 수 있는 것)이 있으면 실패한다', () => {
    const dir = fullDir()
    writeFileSync(join(dir, 'extra.txt'), 'x')
    expect(run(dir).stderr).toContain('예상하지 않은 파일입니다: extra.txt')
  })

  it('중단 장치: ELECTRON_MIGRATION 이 꺼져 있으면 latest.yml 없이 통과하고, 켜져 있는데 없으면 실패한다 — 꺼진 동안 이후 릴리스가 막히지 않는다', () => {
    const off = fullDir('off')
    expect(readdirSync(off)).not.toContain('latest.yml')
    for (const v of ['off', 'OFF', 'false', '0', 'no']) expect(run(off, v).status, v).toBe(0)
    for (const v of ['', 'on', 'true']) expect(run(off, v).status, v).toBe(1)
    // 꺼진 값이어도 latest.yml 이 *있으면* 예상 밖 파일이다(이관 중단이 실제로 반영됐는지를 본다)
    const on = fullDir()
    expect(run(on, 'off').stderr).toContain('예상하지 않은 파일입니다: latest.yml')
  })

  it('필수 자산 목록은 설치본 4 + .sig 4 + latest.json (+ latest.yml)이다', () => {
    expect(A.expectedAssets(VERSION, undefined)).toHaveLength(10)
    expect(A.expectedAssets(VERSION, 'off')).toHaveLength(9)
    expect(A.REQUIRED_PLATFORM_KEYS).toEqual(['windows-x86_64-nsis', 'windows-x86_64', 'darwin-aarch64-dmg', 'darwin-x86_64-dmg', 'linux-x86_64-deb'])
  })
})

describe('release-freeze — 직전 정식 릴리스가 초안이면 야간 릴리스를 만들지 않는다(fail-closed)', () => {
  const d = F.decideFreeze
  it('공개된 릴리스만 통과시킨다', () => {
    expect(d({ last: 'v1.12.3', rc: 0, out: '{"isDraft":false}' }).frozen).toBe(false)
  })
  it('초안·조회 실패(없음 포함)·읽지 못한 응답·isDraft 없음은 모두 막는다', () => {
    expect(d({ last: 'v2.0.0', rc: 0, out: '{"isDraft":true}' }).frozen).toBe(true)
    expect(d({ last: 'v2.0.0', rc: 1, out: 'release not found' }).frozen).toBe(true)
    expect(d({ last: 'v2.0.0', rc: 1, out: '{"isDraft":false}' }).frozen).toBe(true) // 종료 코드가 실패면 출력이 그럴듯해도 막는다
    expect(d({ last: 'v2.0.0', rc: 0, out: 'not json' }).frozen).toBe(true)
    expect(d({ last: 'v2.0.0', rc: 0, out: '{}' }).frozen).toBe(true)
    expect(d({ last: 'v2.0.0', rc: 0, out: '{"isDraft":"false"}' }).frozen).toBe(true)
    expect(d({ last: 'v2.0.0', rc: 0, out: 'null' }).frozen).toBe(true)
  })
  it('기준 태그가 없으면 막을 초안도 없다', () => {
    expect(d({ last: 'none', rc: 1, out: '' }).frozen).toBe(false)
    expect(d({ last: '', rc: 1, out: '' }).frozen).toBe(false)
  })
  it('CLI: GITHUB_OUTPUT 에 붙일 frozen·reason 줄을 낸다(잘못된 rc 는 막는 쪽)', () => {
    const run = (...a: string[]): string => node('release-freeze.mjs', ...a).stdout
    expect(run('--last', 'v2.0.0', '--rc', '0', '--out', '{"isDraft":true}')).toMatch(/^frozen=true\nreason=.*초안/)
    expect(run('--last', 'v2.0.0', '--rc', '0', '--out', '{"isDraft":false}')).toMatch(/^frozen=false\n/)
    expect(run('--last', 'v2.0.0', '--rc', 'x', '--out', '{"isDraft":false}')).toMatch(/^frozen=true\n/)
    expect(run('--last', 'v2.0.0', '--out', '{"isDraft":false}')).toMatch(/^frozen=true\n/)
  })
})
