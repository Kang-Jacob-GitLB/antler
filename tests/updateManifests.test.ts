// Tauri 패키징의 업데이트 매니페스트 생성(scripts/make-update-manifests.mjs)과 Electron 정리 NSIS 훅(src-tauri/nsis/hooks.nsh)의 정적 검사.
// 실제 설치·제거는 하지 않는다 — 훅의 동작은 리허설(R)에서 확인한다(docs/tauri.md "Phase 6c PR 2").
import { createHash, randomBytes } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { isReleaseAssetUrl, isSafeFileName as tsIsSafeFileName, metaFor, parseMetadata, RELEASE_DOWNLOAD_PREFIX } from '../src/main/update/logic'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import * as M from '../scripts/make-update-manifests.mjs'

const root = join(__dirname, '..')
const read = (...p: string[]): string => readFileSync(join(root, ...p), 'utf8')
const req = createRequire(import.meta.url)

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

/** electron-updater 자신의 파서·URL 조립(Provider.js). electron-updater 는 직접 의존이고 그 안의 builder-util-runtime 은 거기서 푼다 */
function electronProvider(): {
  parseUpdateInfo: (raw: string, channelFile: string, url: URL) => { version: string; files: { url: string; sha512?: string; size?: number }[]; path?: string; sha512?: string; releaseDate?: string; tag?: string }
  resolveFiles: (info: unknown, base: URL, transform: (p: string) => string) => { url: URL; info: { url: string; sha512?: string } }[]
} {
  const main = req.resolve('electron-updater')
  return req(join(dirname(main), 'providers', 'Provider.js'))
}

describe('latest.json (Tauri 업데이터)', () => {
  it('표본(tests/fixtures/update/latest.json)이 스크립트 출력과 바이트까지 같다 — Rust 시험이 같은 표본을 check_manifest 에 먹인다', () => {
    const out = M.buildLatestJson({ version: '2.0.0', fileName: M.installerName('2.0.0'), signature: `${SIG}\n`, pubDate: DATE })
    expect(out).toBe(read('tests', 'fixtures', 'update', 'latest.json'))
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

  it('파일 이름 규칙이 TS(isSafeFileName)와 같다', () => {
    for (const n of ['Antler-Setup-2.0.0-x64.exe', 'a', 'A_b.c-d', 'a b', '../x', 'x/y', 'a..b', '-a', '', 'é', 'a'.repeat(201), 'a'.repeat(202)]) {
      expect(M.isSafeFileName(n), n).toBe(tsIsSafeFileName(n))
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

  it('url·path 는 파일 이름만이다(절대 URL 이면 electron-updater 가 경로를 깨뜨린다)', () => {
    const yml = M.buildLatestYml(args)
    const info = electronProvider().parseUpdateInfo(yml, 'latest.yml', new URL('https://github.com/x/latest.yml'))
    expect(info.files).toHaveLength(1)
    expect(info.files[0]!.url).toBe('Antler-Setup-2.0.0-x64.exe')
    expect(info.path).toBe('Antler-Setup-2.0.0-x64.exe')
    expect(tsIsSafeFileName(info.files[0]!.url)).toBe(true)
  })

  it('electron-updater 가 푼 다운로드 주소가 latest.json 의 url 과 같다(GitHub 공급자의 경로 규칙)', () => {
    for (const version of ['2.0.0', '2.0.0-rc.1']) {
      const fileName = M.installerName(version)
      const yml = M.buildLatestYml({ ...args, version, fileName })
      const { parseUpdateInfo, resolveFiles } = electronProvider()
      const info = parseUpdateInfo(yml, 'latest.yml', new URL('https://github.com/x/latest.yml'))
      expect(info.version).toBe(version)
      info.tag = `v${version}`
      // GitHubProvider.resolveFiles 와 같은 변환: /<소유자>/<저장소>/releases/download/<태그>/<파일>, 공백은 '-'
      const files = resolveFiles(info, new URL('https://github.com'), (p) => `/${M.REPO}/releases/download/${info.tag}/${p.replace(/ /g, '-')}`)
      const json = JSON.parse(M.buildLatestJson({ version, fileName, signature: SIG, pubDate: DATE }))
      expect(files[0]!.url.href).toBe(json.platforms['windows-x86_64-nsis'].url)
      expect(isReleaseAssetUrl(files[0]!.url.href)).toBe(true)
      expect(files[0]!.info.sha512).toBe(sha)
    }
  })

  it('우리 쪽 파서(parseMetadata·metaFor)도 읽는다', () => {
    const meta = parseMetadata(M.buildLatestYml(args))!
    expect(meta.version).toBe('2.0.0')
    const f = metaFor(meta, M.installerName('2.0.0'))!
    expect(f.sha512).toBe(sha)
    expect(f.size).toBe(123456)
  })

  it('어떤 해시 값이든 두 파서가 같은 값으로 되읽는다(+·/ 로 시작하는 base64 포함)', () => {
    const { parseUpdateInfo } = electronProvider()
    for (let i = 0; i < 200; i++) {
      let digest = createHash('sha512').update(randomBytes(8)).digest('base64')
      if (i === 0) digest = `+${digest.slice(1)}`
      if (i === 1) digest = `/${digest.slice(1)}`
      if (i === 2) digest = `1${digest.slice(1)}`
      const yml = M.buildLatestYml({ ...args, sha512: digest })
      const info = parseUpdateInfo(yml, 'latest.yml', new URL('https://github.com/x/latest.yml'))
      expect(info.files[0]!.sha512).toBe(digest)
      expect(info.sha512).toBe(digest)
      expect(parseMetadata(yml)!.files[0]!.sha512).toBe(digest)
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
    const r = M.makeManifests({ dir, version: '2.0.0', signatureText: `${SIG}\n`, migration: undefined, now: DATE })
    expect(Object.keys(r.files).sort()).toEqual(['latest.json', 'latest.yml'])
    expect(r.warnings).toEqual([])
    expect(r.sha512).toBe(sha)
    expect(r.size).toBe(content.length)
    expect(r.files['latest.yml']).toContain(`sha512: ${sha}`)
    expect(r.files['latest.yml']).toContain(`size: ${content.length}`)
  })

  it('서명이 없으면 latest.json 만 건너뛰고 경고한다(빌드는 성공)', () => {
    const dir = installerDir('2.0.0', content)
    for (const signatureText of [null, undefined, '', '  \n']) {
      const r = M.makeManifests({ dir, version: '2.0.0', signatureText, now: DATE })
      expect(Object.keys(r.files)).toEqual(['latest.yml'])
      expect(r.warnings.join()).toContain('latest.json')
    }
  })

  it('ELECTRON_MIGRATION 이 off·false·0·no(대소문자·공백 무시)면 latest.yml 을 만들지 않는다', () => {
    const dir = installerDir('2.0.0', content)
    const cases = [['off', false], ['OFF', false], [' off ', false], ['false', false], ['False', false], ['0', false], ['no', false], ['NO', false],
      ['on', true], ['', true], [undefined, true], ['1', true], ['true', true], ['yes', true]] as const
    for (const [migration, expected] of cases) {
      const r = M.makeManifests({ dir, version: '2.0.0', signatureText: SIG, migration, now: DATE })
      expect('latest.yml' in r.files, String(migration)).toBe(expected)
      expect('latest.json' in r.files).toBe(true)
    }
    expect(M.migrationEnabled(undefined)).toBe(true)
  })

  it('설치본이 없거나 이름이 버전과 다르면 실패한다', () => {
    const dir = installerDir('2.0.0', content)
    expect(() => M.makeManifests({ dir, version: '2.0.1', signatureText: SIG })).toThrow(/설치본이 없습니다/)
    expect(() => M.makeManifests({ dir, version: '2.0.0', installer: 'Antler-Setup-9.9.9-x64.exe', signatureText: SIG })).toThrow(/버전과 맞지 않습니다/)
    expect(() => M.makeManifests({ dir, version: 'bad', signatureText: SIG })).toThrow()
  })
})

describe('Electron 정리 NSIS 훅 (정적 검사)', () => {
  const hook = read('src-tauri', 'nsis', 'hooks.nsh').replace(/^﻿/, '')
  const code = hook.split('\n').filter((l) => !l.trimStart().startsWith(';')).join('\n')

  it('GUID 가 electron-builder 가 appId(com.antler.app)로 계산하는 값과 같다', () => {
    // electron-builder 의 의존인 app-builder-lib 을 electron-builder 쪽에서 푼다(pnpm 은 직접 의존이 아니면 루트에서 풀리지 않는다)
    const abl = createRequire(req.resolve('electron-builder')).resolve('app-builder-lib')
    const nsisTarget = readFileSync(join(dirname(abl), 'targets', 'nsis', 'NsisTarget.js'), 'utf8')
    const ns = /ELECTRON_BUILDER_NS_UUID = [\w.]+\.UUID\.parse\("([0-9a-f-]{36})"\)/.exec(nsisTarget)?.[1]
    expect(ns, 'NsisTarget.js 에서 네임스페이스를 읽지 못했다').toBeTruthy()
    const { UUID } = createRequire(abl)('builder-util-runtime')
    const appId = (JSON.parse(read('src-tauri', 'tauri.conf.json')) as { identifier: string }).identifier
    expect(appId).toBe(/^appId:\s*(\S+)/m.exec(read('electron-builder.yml'))?.[1])
    const guid = UUID.v5(appId, UUID.parse(ns)).toString()
    expect(code).toContain(`!define ANTLER_ELECTRON_GUID "${guid}"`)
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
    const fn = code.slice(code.indexOf('Function AntlerForceRun'))
    expect(fn).toContain('"--force-run"')
    expect(fn).toContain('"/R"')
    expect(fn).toContain('nsis_tauri_utils::RunAsUser "$INSTDIR\\${MAINBINARYNAME}.exe"')
    expect(fn).not.toMatch(/^\s*Exec(Wait|Shell)?\s/m)
  })

  it('한국어 문구가 깨지지 않도록 UTF-8 BOM 이 있다(NSIS 는 BOM 없는 파일을 시스템 코드 페이지로 읽는다)', () => {
    const bytes = readFileSync(join(root, 'src-tauri', 'nsis', 'hooks.nsh'))
    expect([...bytes.subarray(0, 3)]).toEqual([0xef, 0xbb, 0xbf])
  })
})
