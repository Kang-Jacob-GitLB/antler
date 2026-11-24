// Tauri 패키징의 업데이트 매니페스트 생성(scripts/make-update-manifests.mjs)과 Electron 정리 NSIS 훅(src-tauri/nsis/hooks.nsh)의 정적 검사.
// 실제 설치·제거는 하지 않는다 — 훅의 동작은 리허설(R)에서 확인한다(docs/tauri.md "Phase 6c PR 2").
import { createHash } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as M from '../scripts/make-update-manifests.mjs'

// Phase 6c PR 7(Electron 제거) 이전에는 이 파일이 electron-updater 의 파서(Provider.js)·app-builder-lib·src/main/update/logic.ts 를 직접 불러 대조했다.
// 그 의존이 사라졌으므로 같은 대조를 **지우기 전에 한 번 실행해 결과를 동결**했다: latest.yml 표본 바이트(tests/fixtures/update/latest.yml)와 아래 기대값이다.
// 동결 시점의 확인(electron-updater 6.8.9): 이 표본을 parseUpdateInfo 가 읽으면 files[0].url·path 가 파일 이름 그대로이고, resolveFiles 가 푼 주소가
// RELEASE_DOWNLOAD_PREFIX + v2.0.0/Antler-Setup-2.0.0-x64.exe 였다. 무작위 해시 200개(+·/·숫자로 시작하는 것 포함)도 두 파서가 같은 값으로 되읽었다.
// GUID 는 electron-builder 의 UUID v5(appId, ELECTRON_BUILDER_NS_UUID)로 계산한 값이다(아래).

const root = join(__dirname, '..')
const read = (...p: string[]): string => readFileSync(join(root, ...p), 'utf8')
const RELEASE_DOWNLOAD_PREFIX: string = M.DOWNLOAD_PREFIX
const tsIsSafeFileName = (n: string): boolean => /^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$/.test(n) && !n.includes('..')
/** 앱(logic.rs is_release_asset_url)의 모양: 이 저장소 releases/download/v<버전>/<안전한 파일 이름> */
const isReleaseAssetUrl = (u: string): boolean => new RegExp(`^${RELEASE_DOWNLOAD_PREFIX.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')}v[0-9A-Za-z.-]+/[A-Za-z0-9][A-Za-z0-9._-]{0,200}$`).test(u)

const SIG = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpSVVE9PQ=='
const DATE = new Date('2026-11-18T00:00:00.000Z')

const tmps: string[] = []
afterEach(() => {
  for (const d of tmps.splice(0)) rmSync(d, { recursive: true, force: true })
})
function installerDir(version: string, content: Buffer): string {
  const dir = mkdtempSync(join(tmpdir(), 'antler-manifest-'))
  tmps.push(dir)
  writeFileSync(join(dir, M.installerName(version)), content)
  return dir
}

describe('latest.json (Tauri 업데이터)', () => {
  it('표본(tests/fixtures/update/latest.json)이 스크립트 출력과 바이트까지 같다 — Rust 시험이 같은 표본을 check_manifest 에 먹인다', () => {
    const out = M.buildLatestJson({ version: '2.0.0', fileName: M.installerName('2.0.0'), signature: `${SIG}\n`, pubDate: DATE })
    expect(out).toBe(read('tests', 'fixtures', 'update', 'latest.json'))
  })

  it('수동 경로(dmg·deb)·macOS 앱 번들 항목까지 담은 표본(latest-all.json)도 스크립트 출력과 바이트까지 같다 — Rust 시험이 대상마다 check_manifest_for 에 먹인다', () => {
    const extras = M.SIGNED_ASSETS.map((a: { key: string; name: (v: string) => string }) => ({ key: a.key, fileName: a.name('2.0.0'), signature: SIG }))
    const out = M.buildLatestJson({ version: '2.0.0', fileName: M.installerName('2.0.0'), signature: SIG, pubDate: DATE, extras })
    expect(out).toBe(read('tests', 'fixtures', 'update', 'latest-all.json'))
  })

  it('수동 경로의 키·파일 이름이 앱(logic.rs)의 상수·규칙과 같다', () => {
    const rs = read('src-tauri', 'src', 'update', 'logic.rs')
    const keys = [...rs.matchAll(/pub const PLATFORM_KEY_(?:DMG_ARM64|DMG_X64|DEB_X64): &str = "([^"]+)";/g)].map((m) => m[1])
    expect(keys).toEqual(M.MANUAL_ASSETS.map((a: { key: string }) => a.key))
    // 앱의 기대 이름(Target::asset_name): Antler-<v>-<arm64|x64>.dmg · antler_<v>_amd64.deb
    expect(rs).toContain('format!("Antler-{v}-{}.dmg", self.asset_arch)')
    expect(rs).toContain('format!("antler_{v}_{}.deb", self.asset_arch)')
    // macOS 앱 번들(앱 안 설치): 키·이름(Target::asset_name — Antler-<v>-<arm64|x64>.app.tar.gz)
    const appKeys = [...rs.matchAll(/pub const PLATFORM_KEY_(?:APP_ARM64|APP_X64): &str = "([^"]+)";/g)].map((m) => m[1])
    expect(appKeys).toEqual(M.APP_ASSETS.map((a: { key: string }) => a.key))
    expect(rs).toContain('format!("Antler-{v}-{}.app.tar.gz", self.asset_arch)')
    expect(M.APP_ASSETS.map((a: { name: (v: string) => string }) => a.name('2.0.0'))).toEqual(['Antler-2.0.0-arm64.app.tar.gz', 'Antler-2.0.0-x64.app.tar.gz'])
    expect(M.MANUAL_ASSETS.map((a: { name: (v: string) => string }) => a.name('2.0.0'))).toEqual(['Antler-2.0.0-arm64.dmg', 'Antler-2.0.0-x64.dmg', 'antler_2.0.0_amd64.deb'])
    // 워크플로의 "설치본 이름 정리"가 같은 이름으로 만든다
    const wf = read('.github', 'workflows', 'tauri-package.yml')
    expect(wf).toContain('out/Antler-${VERSION}-${{ matrix.arch }}.dmg')
    expect(wf).toContain('out/antler_${VERSION}_${{ matrix.arch }}.deb')
    expect(wf).toContain('out/Antler-${VERSION}-${{ matrix.arch }}.app.tar.gz')
    // 이 이름들은 TS 수동 경로(pickAsset)의 이름 규칙이기도 하다
    for (const a of M.MANUAL_ASSETS as { name: (v: string) => string }[]) expect(tsIsSafeFileName(a.name('2.0.0'))).toBe(true)
  })

  it('url 은 이 저장소의 릴리스 자산 주소이고 TS 판정(isReleaseAssetUrl)도 통과한다', () => {
    for (const version of ['2.0.0', '2.0.0-rc.1', '10.20.30']) {
      const doc = JSON.parse(M.buildLatestJson({ version, fileName: M.installerName(version), signature: SIG, pubDate: DATE }))
      expect(doc.version).toBe(version)
      for (const key of ['windows-x86_64-nsis', 'windows-x86_64']) {
        const url: string = doc.platforms[key].url
        expect(url).toBe(`${RELEASE_DOWNLOAD_PREFIX}v${version}/Antler-Setup-${version}-x64.exe`)
        expect(isReleaseAssetUrl(url)).toBe(true)
        expect(doc.platforms[key].signature).toBe(SIG)
      }
    }
  })

  it('저장소 접두사가 TS·Rust 의 상수와 같다', () => {
    expect(M.DOWNLOAD_PREFIX).toBe(RELEASE_DOWNLOAD_PREFIX)
    const rs = read('src-tauri', 'src', 'update', 'logic.rs')
    expect(new RegExp(`RELEASE_DOWNLOAD_PREFIX: &str = "${M.DOWNLOAD_PREFIX.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`).test(rs)).toBe(true)
  })

  it('notes 같은 서명되지 않은 필드를 넣지 않는다', () => {
    const doc = JSON.parse(M.buildLatestJson({ version: '2.0.0', fileName: M.installerName('2.0.0'), signature: SIG, pubDate: DATE }))
    expect(Object.keys(doc).sort()).toEqual(['platforms', 'pub_date', 'version'])
  })

  it('서명이 비었거나 형식이 틀리면 만들지 않는다', () => {
    const base = { version: '2.0.0', fileName: M.installerName('2.0.0'), pubDate: DATE }
    for (const signature of ['', '   \n', null, 'abc def', 'ab\ncd', 'a'.repeat(16 * 1024 + 1), '<script>']) {
      expect(() => M.buildLatestJson({ ...base, signature }), String(signature)).toThrow()
    }
  })

  it('semver 가 아닌 버전·안전하지 않은 파일 이름·잘못된 날짜를 거부한다', () => {
    const ok = { version: '2.0.0', fileName: M.installerName('2.0.0'), signature: SIG, pubDate: DATE }
    for (const version of ['v2.0.0', '2.0', '2.0.0+build', '02.0.0', '', ' 2.0.0']) expect(() => M.buildLatestJson({ ...ok, version }), version).toThrow()
    for (const fileName of ['../a.exe', 'a/b.exe', 'a b.exe', '', 'a..exe', '.hidden.exe']) expect(() => M.buildLatestJson({ ...ok, fileName }), fileName).toThrow()
    expect(() => M.buildLatestJson({ ...ok, pubDate: 'not a date' })).toThrow()
  })

  it('파일 이름 규칙이 앱(logic.rs is_safe_file_name)과 같다 — 동결한 표', () => {
    const table: [string, boolean][] = [['Antler-Setup-2.0.0-x64.exe', true], ['a', true], ['A_b.c-d', true], ['a b', false], ['../x', false], ['x/y', false], ['a..b', false], ['-a', false], ['', false], ['é', false], ['a'.repeat(201), true], ['a'.repeat(202), false]]
    for (const [n, ok] of table) {
      expect(M.isSafeFileName(n), n).toBe(ok)
      expect(tsIsSafeFileName(n), n).toBe(ok)
    }
  })
})

describe('latest.yml (Electron 이관용)', () => {
  const sha = createHash('sha512').update('installer').digest('base64')
  const args = { version: '2.0.0', fileName: M.installerName('2.0.0'), sha512: sha, size: 123456, releaseDate: DATE }

  it('electron-builder 가 만드는 모양(키 순서)과 같다', () => {
    expect(M.buildLatestYml(args)).toBe(
      ['version: 2.0.0', 'files:', '  - url: Antler-Setup-2.0.0-x64.exe', `    sha512: ${sha}`, '    size: 123456', 'path: Antler-Setup-2.0.0-x64.exe', `sha512: ${sha}`, "releaseDate: '2026-11-18T00:00:00.000Z'", ''].join('\n')
    )
  })

  it('표본(tests/fixtures/update/latest.yml)이 스크립트 출력과 바이트까지 같다 — 동결 시점에 electron-updater 가 이 바이트를 읽는 것을 확인했다', () => {
    expect(M.buildLatestYml(args)).toBe(read('tests', 'fixtures', 'update', 'latest.yml'))
    // 동결 시점의 electron-updater 파서 결과: url·path 는 파일 이름 그대로(절대 URL 이면 GitHub 공급자가 경로를 깨뜨린다), 푼 주소는 latest.json 의 url 과 같다
    const lines = read('tests', 'fixtures', 'update', 'latest.yml').split('\n')
    expect(lines).toContain('  - url: Antler-Setup-2.0.0-x64.exe')
    expect(lines).toContain('path: Antler-Setup-2.0.0-x64.exe')
    expect(tsIsSafeFileName('Antler-Setup-2.0.0-x64.exe')).toBe(true)
  })

  it('url·path 는 파일 이름만이다 — 주소가 아니라 `<파일 이름>` (GitHub 공급자가 /<소유자>/<저장소>/releases/download/<태그>/ 를 붙인다)', () => {
    for (const version of ['2.0.0', '2.0.0-rc.1']) {
      const fileName = M.installerName(version)
      const yml: string = M.buildLatestYml({ ...args, version, fileName })
      expect(yml).toContain(`\n  - url: ${fileName}\n`)
      expect(yml).toContain(`\npath: ${fileName}\n`)
      expect(yml).not.toMatch(/https?:/)
      // electron-updater 가 푼 주소 = 접두사 + v<버전>/<파일 이름> = latest.json 의 url (동결 시점에 resolveFiles 로 확인한 규칙)
      const json = JSON.parse(M.buildLatestJson({ version, fileName, signature: SIG, pubDate: DATE }))
      expect(json.platforms['windows-x86_64-nsis'].url).toBe(`${RELEASE_DOWNLOAD_PREFIX}v${version}/${fileName}`)
      expect(isReleaseAssetUrl(json.platforms['windows-x86_64-nsis'].url)).toBe(true)
    }
  })

  it('어떤 해시 값이든 같은 줄 모양으로 쓴다(+·/·숫자로 시작하는 base64 포함 — 동결 시점에 두 파서가 되읽었다)', () => {
    for (const first of ['+', '/', '1', 'A']) {
      const digest = `${first}${sha.slice(1)}`
      const yml: string = M.buildLatestYml({ ...args, sha512: digest })
      expect(yml.split('\n')).toContain(`    sha512: ${digest}`)
      expect(yml.split('\n')).toContain(`sha512: ${digest}`)
    }
  })

  it('sha512·size·날짜 형식이 틀리면 만들지 않는다', () => {
    for (const sha512 of ['', 'abc', `${sha.slice(0, -2)}xx`, null]) expect(() => M.buildLatestYml({ ...args, sha512 }), String(sha512)).toThrow()
    for (const size of [0, -1, 1.5, NaN, '5']) expect(() => M.buildLatestYml({ ...args, size }), String(size)).toThrow()
    expect(() => M.buildLatestYml({ ...args, releaseDate: 'x' })).toThrow()
  })
})

describe('makeManifests (실물 대조 가드)', () => {
  const content = Buffer.from('fake installer bytes')
  const sha = createHash('sha512').update(content).digest('base64')

  it('서명이 있으면 둘 다, 해시·크기는 실물과 같다', () => {
    const dir = installerDir('2.0.0', content)
    const r = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: `${SIG}\n`, migration: undefined, now: DATE })
    expect(Object.keys(r.files).sort()).toEqual(['latest.json', 'latest.yml'])
    // 이 폴더에는 macOS·Linux 설치본이 없다 — 항목(dmg 2·deb·앱 번들 2)을 건너뛰었다고 경고한다(Windows 항목은 정상)
    expect(r.warnings).toHaveLength(5)
    expect(r.warnings.every((w: string) => w.includes('없어 latest.json 의'))).toBe(true)
    expect(Object.keys(JSON.parse(r.files['latest.json']).platforms)).toEqual(['windows-x86_64-nsis', 'windows-x86_64'])
    expect(r.sha512).toBe(sha)
    expect(r.size).toBe(content.length)
    expect(r.files['latest.yml']).toContain(`sha512: ${sha}`)
    expect(r.files['latest.yml']).toContain(`size: ${content.length}`)
  })

  it('서명이 없으면 latest.json 만 건너뛰고 경고한다(빌드는 성공)', () => {
    const dir = installerDir('2.0.0', content)
    for (const signatureText of [null, undefined, '', '  \n']) {
      const r = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText, now: DATE })
      expect(Object.keys(r.files)).toEqual(['latest.yml'])
      expect(r.warnings.join()).toContain('latest.json')
    }
  })

  it('설치본과 .sig 가 함께 있는 macOS·Linux 자산만 latest.json 에 넣고, 없거나 서명이 없으면 경고한다', () => {
    const dir = installerDir('2.0.0', content)
    const put = (name: string, text: string): void => writeFileSync(join(dir, name), text)
    // dmg arm64: 설치본 + 서명 → 들어간다. dmg x64: 설치본만 → 경고. deb: 둘 다 없음 → 경고
    put('Antler-2.0.0-arm64.dmg', 'dmg bytes')
    put('Antler-2.0.0-arm64.dmg.sig', `${SIG}\n`)
    put('Antler-2.0.0-x64.dmg', 'dmg bytes')
    const r = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: SIG, now: DATE })
    const doc = JSON.parse(r.files['latest.json'])
    expect(Object.keys(doc.platforms)).toEqual(['windows-x86_64-nsis', 'windows-x86_64', 'darwin-aarch64-dmg'])
    expect(doc.platforms['darwin-aarch64-dmg']).toEqual({ signature: SIG, url: `${M.DOWNLOAD_PREFIX}v2.0.0/Antler-2.0.0-arm64.dmg` })
    expect(r.warnings.join('\n')).toContain('Antler-2.0.0-x64.dmg 의 서명(.sig)이 없어')
    expect(r.warnings.join('\n')).toContain('antler_2.0.0_amd64.deb 이(가) 없어')
    // latest.yml(Electron 이관용)은 Windows 설치본만 가리킨다
    expect(r.files['latest.yml']).not.toContain('dmg')
    // Windows 서명이 없어도 수동 경로 항목이 있으면 그것만으로 latest.json 을 만든다
    const only = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: null, now: DATE })
    expect(Object.keys(JSON.parse(only.files['latest.json']).platforms)).toEqual(['darwin-aarch64-dmg'])
    // 서명 형식이 틀린 .sig 는 거부한다
    put('Antler-2.0.0-x64.dmg.sig', 'not base64 !!')
    expect(() => M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: SIG, now: DATE })).toThrow(/서명 형식/)
  })

  it('서명 단계가 돌았다는 표식(signed)이 없으면 .sig 가 있어도 어떤 서명 항목도 만들지 않는다', () => {
    const dir = installerDir('2.0.0', content)
    writeFileSync(join(dir, 'Antler-2.0.0-arm64.dmg'), 'dmg bytes')
    writeFileSync(join(dir, 'Antler-2.0.0-arm64.dmg.sig'), `${SIG}\n`)
    for (const signed of [undefined, false]) {
      const r = M.makeManifests({ dir, version: '2.0.0', signatureText: SIG, signed, now: DATE })
      expect('latest.json' in r.files, String(signed)).toBe(false)
      expect(r.warnings.join('\n')).toContain('--signed true')
      // latest.yml(서명과 무관)은 그대로다
      expect('latest.yml' in r.files).toBe(true)
    }
    // 워크플로는 키 확인 단계의 결과를 표식으로 넘기고, 스크립트는 'true' 문자열만 받는다
    expect(read('.github', 'workflows', 'tauri-package.yml')).toContain('--signed "${{ steps.key.outputs.signed }}"')
    expect(read('scripts', 'make-update-manifests.mjs')).toContain("args.signed === 'true'")
  })

  it('수동 경로 서명이 하나도 없고 Windows 서명도 없으면 latest.json 을 만들지 않는다', () => {
    const dir = installerDir('2.0.0', content)
    const r = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: null, now: DATE })
    expect('latest.json' in r.files).toBe(false)
    expect(r.warnings.join('\n')).toContain('latest.json 을 만들지 않았습니다')
  })

  it('ELECTRON_MIGRATION 이 off·false·0·no(대소문자·공백 무시)면 latest.yml 을 만들지 않는다', () => {
    const dir = installerDir('2.0.0', content)
    const cases = [['off', false], ['OFF', false], [' off ', false], ['false', false], ['False', false], ['0', false], ['no', false], ['NO', false],
      ['on', true], ['', true], [undefined, true], ['1', true], ['true', true], ['yes', true]] as const
    for (const [migration, expected] of cases) {
      const r = M.makeManifests({ signed: true, dir, version: '2.0.0', signatureText: SIG, migration, now: DATE })
      expect('latest.yml' in r.files, String(migration)).toBe(expected)
      expect('latest.json' in r.files).toBe(true)
    }
    expect(M.migrationEnabled(undefined)).toBe(true)
  })

  it('설치본이 없거나 이름이 버전과 다르면 실패한다', () => {
    const dir = installerDir('2.0.0', content)
    expect(() => M.makeManifests({ signed: true, dir, version: '2.0.1', signatureText: SIG })).toThrow(/설치본이 없습니다/)
    expect(() => M.makeManifests({ signed: true, dir, version: '2.0.0', installer: 'Antler-Setup-9.9.9-x64.exe', signatureText: SIG })).toThrow(/버전과 맞지 않습니다/)
    expect(() => M.makeManifests({ signed: true, dir, version: 'bad', signatureText: SIG })).toThrow()
  })
})

describe('Electron 정리 NSIS 훅 (정적 검사)', () => {
  const hook = read('src-tauri', 'nsis', 'hooks.nsh').replace(/^﻿/, '')
  const code = hook.split('\n').filter((l) => !l.trimStart().startsWith(';')).join('\n')

  it('GUID 는 동결한 값이다 — electron-builder 가 appId(com.antler.app)로 계산하는 UUID v5 (Electron 제거 전에 builder-util-runtime 의 UUID.v5 로 대조했다)', () => {
    // 이 GUID 는 현장에 설치된 Electron 판의 제거 키(HKCU …\Uninstall\<GUID>)다 — 바꾸면 이관 훅이 Electron 판을 찾지 못한다.
    // appId 가 바뀌면(첫 공개 뒤에는 바꿀 수 없다 — docs/tauri.md) 이 값도 다시 계산해야 하지만, 이미 설치된 Electron 판의 키는 그대로다
    const FROZEN_GUID = 'a0282e38-7d2f-5de0-a2ab-1ae175fbe58f'
    expect((JSON.parse(read('src-tauri', 'tauri.conf.json')) as { identifier: string }).identifier).toBe('com.antler.app')
    expect(code).toContain(`!define ANTLER_ELECTRON_GUID "${FROZEN_GUID}"`)
  })

  it('tauri.conf.json 이 이 훅을 NSIS 사용자별 설치로 건다', () => {
    const conf = JSON.parse(read('src-tauri', 'tauri.conf.json')) as { bundle: { windows: { nsis: { installMode: string; installerHooks: string } } } }
    expect(conf.bundle.windows.nsis.installMode).toBe('currentUser')
    expect(conf.bundle.windows.nsis.installerHooks).toBe('nsis/hooks.nsh')
    expect(code).toContain('!macro NSIS_HOOK_POSTINSTALL')
  })

  it('제거기 인자는 electron-builder 가 옛 판을 지울 때 쓰는 것이다 — 앱 데이터를 보존하고 바로가기를 남긴다', () => {
    expect(code).toContain('/S /KEEP_APP_DATA --updated --keep-shortcuts _?=$1')
    expect(code).not.toContain('--delete-app-data')
  })

  it('제거기는 임시 폴더로 복사해 돌린다 — 설치 폴더 안에서 제자리로 돌리면 자기 자신이 잠겨 --updated 의 폴더 옮기기가 실패한다(종료 코드 2, v2.0.0-rc.3)', () => {
    const exec = code.slice(code.indexOf('ExecWait'))
    expect(exec).toMatch(/^ExecWait '"\$PLUGINSDIR\\old-uninstaller\.exe" /)
    const before = code.slice(0, code.indexOf('ExecWait'))
    expect(before).toContain('CopyFiles /SILENT /FILESONLY "$2" "$PLUGINSDIR\\old-uninstaller.exe"')
    expect(before.indexOf('InitPluginsDir')).toBeLessThan(before.indexOf('CopyFiles'))
  })

  it('%APPDATA% 를 건드리지 않고 재귀 삭제·HKLM 쓰기를 하지 않는다', () => {
    expect(code).not.toMatch(/\$APPDATA|\$LOCALAPPDATA|%APPDATA%/)
    expect(code).not.toMatch(/RMDir\s+\/r/i)
    expect(code).not.toMatch(/WriteReg|DeleteReg/i)
    // 지우는 것은 검증한 제거기 파일과 비어 있는 InstallLocation 뿐
    expect([...code.matchAll(/^\s*(Delete|RMDir)\s+(.*)$/gm)].map((m) => `${m[1]} ${m[2]}`)).toEqual(['Delete $2', 'RMDir $1'])
  })

  it('실행하기 전에 경로를 검증한다(드라이브 경로·InstallLocation 바로 아래·.exe·존재)', () => {
    const exec = code.indexOf('ExecWait')
    expect(exec).toBeGreaterThan(0)
    const before = code.slice(0, exec)
    for (const needle of ["!= ':\\'", "!= '$1\\'", "!= '.exe'", '${IfNot} ${FileExists} $2', '$1 == $INSTDIR']) expect(before, needle).toContain(needle)
    // 새 설치 폴더가 InstallLocation 의 하위여도 제거하지 않는다(`$1\` 로 시작하는지)
    expect(before).toMatch(/StrCpy \$4 \$INSTDIR \$3\n\s*\$\{If\} \$4 == '\$1\\'/)
    // 같은 GUID 의 다른 앱 방어: DisplayName 이 `Antler` 또는 `Antler <버전>`
    expect(before).toContain('ReadRegStr $3 HKCU "${ANTLER_ELECTRON_UNINSTALL_KEY}" "DisplayName"')
    expect(before).toContain('${If} $3 != "Antler"')
    expect(before).toContain('${AndIf} $4 != "Antler "')
    expect([...code.matchAll(/ExecWait/g)]).toHaveLength(1)
    // 모든 사용자용(HKLM)은 읽기만 하고 안내한다 — 32·64비트 뷰를 모두 본다
    expect(code).toContain('ReadRegStr $3 HKLM')
    expect(code).toContain('SetRegView 32')
  })

  it('설치기가 관리자 토큰이면 HKCU 기반 제거기를 실행하지 않고 안내만 한다(ExecWait 앞에서 분기)', () => {
    const exec = code.indexOf('ExecWait')
    const admin = code.indexOf('UserInfo::GetAccountType')
    expect(admin).toBeGreaterThan(0)
    expect(admin).toBeLessThan(exec)
    const branch = code.slice(admin, exec)
    expect(branch).toMatch(/\$\{If\} \$6 == "Admin"[\s\S]*MessageBox[\s\S]*Goto done\n\s*\$\{EndIf\}/)
  })

  it('재실행(--force-run)은 제거기 호출 뒤에, /R 이 없을 때만, 권한을 낮춰(RunAsUser) 한다', () => {
    const macro = code.slice(code.indexOf('!macro NSIS_HOOK_POSTINSTALL'))
    expect(macro.indexOf('AntlerRemoveElectron')).toBeGreaterThan(0)
    expect(macro.indexOf('AntlerForceRun')).toBeGreaterThan(macro.indexOf('AntlerRemoveElectron'))
    expect(macro).toContain('!insertmacro AntlerForceRun')
    const fn = code.slice(code.indexOf('!macro AntlerForceRun'))
    expect(fn).toContain('"--force-run"')
    expect(fn).toContain('"/R"')
    expect(fn).toContain('nsis_tauri_utils::RunAsUser "$INSTDIR\\${MAINBINARYNAME}.exe"')
    expect(fn).not.toMatch(/^\s*Exec(Wait|Shell)?\s/m)
  })

  it('템플릿 전용 플러그인(nsis_tauri_utils)은 Function 본문이 아니라 매크로 안에서만 부른다 — hooks.nsh 는 플러그인 경로가 더해지기 전에 include 돼 Function 은 "Plugin not found" 로 빌드가 멈춘다(v2.0.0-rc.1)', () => {
    const functions = [...code.matchAll(/^Function\b[\s\S]*?^FunctionEnd\b/gm)].map((m) => m[0])
    expect(functions.length).toBeGreaterThan(0)
    for (const f of functions) expect(f).not.toContain('nsis_tauri_utils::')
  })

  it('한국어 문구가 깨지지 않도록 UTF-8 BOM 이 있다(NSIS 는 BOM 없는 파일을 시스템 코드 페이지로 읽는다)', () => {
    const bytes = readFileSync(join(root, 'src-tauri', 'nsis', 'hooks.nsh'))
    expect([...bytes.subarray(0, 3)]).toEqual([0xef, 0xbb, 0xbf])
  })
})
