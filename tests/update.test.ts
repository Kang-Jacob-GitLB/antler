import { createHash } from 'node:crypto'
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  UpdatePolicy,
  compareSemver,
  detectInstallKind,
  digestFile,
  isAllowedDownloadUrl,
  isOfferable,
  isReleaseAssetUrl,
  metaFor,
  modeFor,
  parseDismiss,
  parseMetadata,
  pickAsset,
  sanitizeError,
  sha256FromDigest,
  verifyDigests,
  verifyFile,
  type ReleaseAsset
} from '../src/main/update/logic'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { downloadVerified, findManualUpdate } from '../src/main/update/manual'
import { UpdateService, type AutoBackend, type UpdateDeps } from '../src/main/update/UpdateService'
import type { UpdateState } from '../src/shared/types'

const BASE = 'https://github.com/Kang-Jacob-GitLB/antler/releases/download'
const sha512b64 = (b: Buffer): string => createHash('sha512').update(b).digest('base64')
const sha256hex = (b: Buffer): string => createHash('sha256').update(b).digest('hex')

describe('semver', () => {
  it('비교', () => {
    expect(compareSemver('0.2.0', '0.1.0')).toBe(1)
    expect(compareSemver('v0.1.0', '0.1.0')).toBe(0)
    expect(compareSemver('1.0.0-rc.1', '1.0.0')).toBe(-1)
    expect(compareSemver('1.0.0-rc.2', '1.0.0-rc.10')).toBe(-1)
    expect(compareSemver('1.0.0-alpha', '1.0.0-1')).toBe(1)
    expect(compareSemver('1.10.0', '1.9.9')).toBe(1)
    expect(compareSemver('abc', '1.0.0')).toBeNull()
  })
  it('프리릴리스는 현재가 프리릴리스일 때만 알림', () => {
    expect(isOfferable('0.1.0', '0.2.0')).toBe(true)
    expect(isOfferable('0.1.0', '0.1.0')).toBe(false)
    expect(isOfferable('0.2.0', '0.1.0')).toBe(false)
    expect(isOfferable('0.1.0', '0.2.0-rc.1')).toBe(false)
    expect(isOfferable('0.2.0-rc.1', '0.2.0-rc.2')).toBe(true)
    expect(isOfferable('0.2.0-rc.1', '0.2.0')).toBe(true)
  })
})

describe('설치 형태와 자산 선택', () => {
  const names = [
    'Antler-Setup-0.2.0-x64.exe',
    'Antler-Portable-0.2.0.exe',
    'Antler-0.2.0-arm64.dmg',
    'Antler-0.2.0-x64.dmg',
    'Antler-0.2.0-x64.AppImage',
    'antler_0.2.0_amd64.deb',
    'latest.yml'
  ]
  const assets: ReleaseAsset[] = names.map((name) => ({ name, size: 10, url: `${BASE}/v0.2.0/${name}` }))

  it('설치 형태 판별', () => {
    expect(detectInstallKind('win32', {})).toBe('nsis')
    expect(detectInstallKind('win32', { PORTABLE_EXECUTABLE_DIR: 'C:\\x' })).toBe('portable')
    const ai = { APPIMAGE: '/t/a.AppImage', APPDIR: '/tmp/.mount_x' }
    expect(detectInstallKind('linux', ai, { execPath: '/tmp/.mount_x/antler' })).toBe('appimage')
    // 상속된 환경 변수만으로는 AppImage 로 보지 않는다(실행 파일이 APPDIR 밖)
    expect(detectInstallKind('linux', ai, { execPath: '/opt/Antler/antler' })).toBe('unsupported')
    expect(detectInstallKind('linux', { APPIMAGE: '/t/a.AppImage' }, { execPath: '/opt/Antler/antler' })).toBe('unsupported')
    expect(detectInstallKind('linux', {}, { packageType: 'deb\n', execPath: '/opt/Antler/antler' })).toBe('deb')
    // package-type 이 deb 이면 APPIMAGE 가 상속돼 있어도 deb
    expect(detectInstallKind('linux', { ...ai, APPDIR: '/opt/Antler' }, { packageType: 'deb', execPath: '/opt/Antler/antler' })).toBe('deb')
    expect(detectInstallKind('linux', {}, { packageType: 'rpm' })).toBe('unsupported')
    expect(detectInstallKind('linux', {})).toBe('unsupported')
    expect(detectInstallKind('darwin', {})).toBe('mac')
    expect(detectInstallKind('freebsd', {})).toBe('unsupported')
    expect(modeFor('nsis')).toBe('auto')
    expect(modeFor('appimage')).toBe('auto')
    expect(modeFor('mac')).toBe('manual-dmg')
    expect(modeFor('portable')).toBe('manual-file')
    expect(modeFor('deb')).toBe('manual-file')
  })
  it('플랫폼·아키텍처별 자산', () => {
    expect(pickAsset(assets, 'mac', 'arm64', '0.2.0')?.name).toBe('Antler-0.2.0-arm64.dmg')
    expect(pickAsset(assets, 'mac', 'x64', '0.2.0')?.name).toBe('Antler-0.2.0-x64.dmg')
    expect(pickAsset(assets, 'portable', 'x64', '0.2.0')?.name).toBe('Antler-Portable-0.2.0.exe')
    expect(pickAsset(assets, 'deb', 'x64', '0.2.0')?.name).toBe('antler_0.2.0_amd64.deb')
    expect(pickAsset(assets, 'deb', 'arm64', '0.2.0')).toBeNull()
    expect(pickAsset(assets, 'mac', 'ia32', '0.2.0')).toBeNull()
    expect(pickAsset(assets, 'nsis', 'x64', '0.2.0')).toBeNull() // auto 는 electron-updater 담당
  })
  it('다른 버전·위험한 이름은 고르지 않는다', () => {
    expect(pickAsset(assets, 'mac', 'x64', '0.3.0')).toBeNull()
    const evil = [{ name: '..\\evil.dmg', size: 1, url: `${BASE}/x/evil.dmg` }]
    expect(pickAsset(evil, 'mac', 'x64', '0.2.0')).toBeNull()
  })
})

describe('다운로드 URL 허용 목록', () => {
  it('github.com · objects.githubusercontent.com 의 https 만', () => {
    expect(isAllowedDownloadUrl('https://github.com/a/b')).toBe(true)
    expect(isAllowedDownloadUrl('https://objects.githubusercontent.com/x?y=1')).toBe(true)
    expect(isAllowedDownloadUrl('http://github.com/a')).toBe(false)
    expect(isAllowedDownloadUrl('https://evil.com/a')).toBe(false)
    expect(isAllowedDownloadUrl('https://github.com.evil.com/a')).toBe(false)
    expect(isAllowedDownloadUrl('https://user:pw@github.com/a')).toBe(false)
    expect(isAllowedDownloadUrl('https://github.com:8443/a')).toBe(false)
    expect(isAllowedDownloadUrl('not a url')).toBe(false)
  })
  it('첫 요청은 이 저장소의 릴리스 자산이어야 한다', () => {
    expect(isReleaseAssetUrl(`${BASE}/v0.2.0/a.dmg`)).toBe(true)
    expect(isReleaseAssetUrl('https://github.com/other/repo/releases/download/v1/a.dmg')).toBe(false)
  })
  it('경로 이탈(..·인코딩된 점/슬래시/백슬래시)을 거부한다', () => {
    expect(isReleaseAssetUrl(`${BASE}/../../../../evil/repo/releases/download/v9.9.9/x.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/%2e%2e/%2e%2e/x.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/..%2f..%2fx.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/a%5cb.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/a\\b.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/./a.dmg`)).toBe(false)
    expect(isReleaseAssetUrl(`${BASE}/v1/a.dmg?x=1`)).toBe(false)
  })
})

describe('메타데이터 · 해시', () => {
  const yml = `version: 0.2.0
files:
  - url: Antler-0.2.0-arm64.dmg
    sha512: AAA=
    size: 100
  - url: Antler-0.2.0-x64.dmg
    sha512: 'BBB='
    size: 200
path: Antler-0.2.0-arm64.dmg
sha512: AAA=
releaseDate: '2026-10-15T09:23:46.078Z'
`
  it('latest*.yml 파싱', () => {
    const m = parseMetadata(yml)!
    expect(m.version).toBe('0.2.0')
    expect(m.files).toEqual([
      { url: 'Antler-0.2.0-arm64.dmg', sha512: 'AAA=', size: 100 },
      { url: 'Antler-0.2.0-x64.dmg', sha512: 'BBB=', size: 200 }
    ])
    expect(metaFor(m, 'Antler-0.2.0-x64.dmg')?.sha512).toBe('BBB=')
    expect(metaFor(m, 'nope.dmg')).toBeNull()
    expect(parseMetadata('files:\n  - url: a')).toBeNull()
  })
  it('API digest 해석', () => {
    expect(sha256FromDigest(`sha256:${'a'.repeat(64)}`)).toBe('a'.repeat(64))
    expect(sha256FromDigest('sha1:abc')).toBeUndefined()
    expect(sha256FromDigest(undefined)).toBeUndefined()
  })
  it('sha512·sha256·크기 검증', () => {
    const d = { sha512: 'S5', sha256: 'S2', size: 10 }
    expect(verifyDigests(d, { sha512: 'S5', size: 10 })).toEqual({ ok: true, by: 'sha512' })
    expect(verifyDigests(d, { sha512: 'XX', size: 10 }).ok).toBe(false)
    expect(verifyDigests(d, { sha512: 'S5', size: 11 }).ok).toBe(false)
    expect(verifyDigests(d, { sha256: 'S2' })).toEqual({ ok: true, by: 'sha256' })
    expect(verifyDigests(d, { sha256: 'no' }).ok).toBe(false)
    // 해시가 있으면 sha512 가 우선이라 sha256 이 맞아도 sha512 가 틀리면 실패
    expect(verifyDigests(d, { sha512: 'XX', sha256: 'S2' }).ok).toBe(false)
    // 크기만으로는 통과시키지 않는다
    expect(verifyDigests(d, { size: 10 }).ok).toBe(false)
    expect(verifyDigests(d, { sha256: 'S2', size: 11 }).ok).toBe(false)
    // 있는 해시는 모두 맞아야 한다
    expect(verifyDigests(d, { sha512: 'S5', sha256: 'no' }).ok).toBe(false)
    expect(verifyDigests(d, { sha512: 'S5', sha256: 'S2' })).toEqual({ ok: true, by: 'sha512' })
    expect(verifyDigests(d, {}).ok).toBe(false)
  })
  it('파일 해시 계산', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'antler-upd-'))
    try {
      const body = Buffer.from('hello antler')
      writeFileSync(join(dir, 'f'), body)
      const d = await digestFile(join(dir, 'f'))
      expect(d).toEqual({ sha512: sha512b64(body), sha256: sha256hex(body), size: body.length })
      expect((await verifyFile(join(dir, 'f'), { sha512: sha512b64(body) })).ok).toBe(true)
      expect((await verifyFile(join(dir, 'f'), { sha512: sha512b64(Buffer.from('x')) })).ok).toBe(false)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

describe('나중에 / 건너뛰기', () => {
  it('자동 확인은 나중에·건너뛴 버전을 숨기고 수동 확인은 항상 보인다', () => {
    let skipped: string | undefined
    const p = new UpdatePolicy(() => skipped)
    expect(p.shouldNotify('0.2.0', false)).toBe(true)
    p.snooze('0.2.0')
    expect(p.shouldNotify('0.2.0', false)).toBe(false)
    expect(p.shouldNotify('0.2.0', true)).toBe(true)
    expect(p.shouldNotify('0.2.1', false)).toBe(true) // 더 새 버전은 다시 알림
    // 나중에는 메모리에만 있다 — 앱을 다시 시작(새 정책 인스턴스)하면 풀린다
    expect(new UpdatePolicy(() => skipped).shouldNotify('0.2.0', false)).toBe(true)
    skipped = '0.3.0'
    expect(p.shouldNotify('0.3.0', false)).toBe(false)
    expect(p.shouldNotify('0.2.5', false)).toBe(false)
    expect(p.shouldNotify('0.3.1', false)).toBe(true)
    expect(p.shouldNotify('0.3.0', true)).toBe(true)
  })
})

describe('IPC 입력 검증', () => {
  it('update:dismiss', () => {
    expect(parseDismiss('later', 'v0.2.0')).toEqual({ action: 'later', version: '0.2.0' })
    expect(parseDismiss('skip', '1.0.0-rc.1')).toEqual({ action: 'skip', version: '1.0.0-rc.1' })
    expect(() => parseDismiss('bogus', '0.2.0')).toThrow()
    expect(() => parseDismiss('skip', '../../etc')).toThrow()
    expect(() => parseDismiss('skip', 5)).toThrow()
    expect(() => parseDismiss(undefined, undefined)).toThrow()
  })
  it('오류 문구에서 토큰·경로·URL 쿼리를 지운다', () => {
    const s = sanitizeError(new Error('fail https://objects.githubusercontent.com/x?token=SECRET ghp_abcdefghijklmnopqrstuvwxyz0123 C:\\Users\\me\\a.dmg'))
    expect(s).not.toContain('SECRET')
    expect(s).not.toContain('ghp_')
    expect(s).not.toContain('Users')
    expect(s.length).toBeLessThanOrEqual(201)
  })
})

// ── fetch 모의 ───────────────────────────────────────────────────────

interface Route {
  status?: number
  body?: Buffer | string
  headers?: Record<string, string>
}
function mockFetch(routes: Record<string, Route>): typeof fetch & { calls: string[] } {
  const calls: string[] = []
  const fn = (async (input: RequestInfo | URL) => {
    const url = String(input)
    calls.push(url)
    const r = routes[url]
    if (!r) return new Response('not found', { status: 404 })
    return new Response((r.body ?? "") as BodyInit, { status: r.status ?? 200, headers: r.headers ?? {} })
  }) as typeof fetch & { calls: string[] }
  fn.calls = calls
  return fn
}

const API = 'https://api.github.com/repos/Kang-Jacob-GitLB/antler'
const DMG = Buffer.from('fake-dmg-content'.repeat(100))

function releaseJson(over: Record<string, unknown> = {}): string {
  return JSON.stringify({
    tag_name: 'v0.2.0',
    draft: false,
    prerelease: false,
    assets: [
      { name: 'Antler-0.2.0-arm64.dmg', size: DMG.length, browser_download_url: `${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`, digest: `sha256:${sha256hex(DMG)}` },
      { name: 'Antler-0.2.0-x64.dmg', size: 5, browser_download_url: `${BASE}/v0.2.0/Antler-0.2.0-x64.dmg` },
      { name: 'latest-mac.yml', size: 100, browser_download_url: `${BASE}/v0.2.0/latest-mac.yml` }
    ],
    ...over
  })
}
const metaYml = (sha: string): string => `version: 0.2.0\nfiles:\n  - url: Antler-0.2.0-arm64.dmg\n    sha512: ${sha}\n    size: ${DMG.length}\n`

describe('findManualUpdate (mock fetch)', () => {
  it('새 버전과 sha512(메타데이터) 기대값을 찾는다', async () => {
    const f = mockFetch({
      [`${API}/releases/latest`]: { body: releaseJson() },
      [`${BASE}/v0.2.0/latest-mac.yml`]: { body: metaYml(sha512b64(DMG)) }
    })
    const u = await findManualUpdate(f, { current: '0.1.0', kind: 'mac', arch: 'arm64' })
    expect(u?.version).toBe('0.2.0')
    expect(u?.asset.name).toBe('Antler-0.2.0-arm64.dmg')
    expect(u?.expected.sha512).toBe(sha512b64(DMG))
    expect(u?.expected.sha256).toBe(sha256hex(DMG))
    expect(u?.notesUrl).toBe('https://github.com/Kang-Jacob-GitLB/antler/releases/tag/v0.2.0')
  })
  it('메타데이터가 없으면 API digest 로, 같은/낮은 버전이면 null', async () => {
    const f = mockFetch({ [`${API}/releases/latest`]: { body: releaseJson() } })
    const u = await findManualUpdate(f, { current: '0.1.0', kind: 'mac', arch: 'arm64' })
    expect(u?.expected.sha512).toBeUndefined()
    expect(u?.expected.sha256).toBe(sha256hex(DMG))
    expect(await findManualUpdate(f, { current: '0.2.0', kind: 'mac', arch: 'arm64' })).toBeNull()
  })
  it('정식 사용자에게 프리릴리스는 숨긴다', async () => {
    const f = mockFetch({ [`${API}/releases/latest`]: { body: releaseJson({ tag_name: 'v0.3.0-rc.1', prerelease: true }) } })
    expect(await findManualUpdate(f, { current: '0.1.0', kind: 'mac', arch: 'arm64' })).toBeNull()
  })
  it('프리릴리스 사용자는 목록에서 찾는다', async () => {
    const f = mockFetch({ [`${API}/releases?per_page=10`]: { body: `[${releaseJson({ tag_name: 'v0.2.0-rc.2', prerelease: true, assets: [] })}]` } })
    await expect(findManualUpdate(f, { current: '0.2.0-rc.1', kind: 'mac', arch: 'arm64' })).rejects.toThrow('설치 파일')
  })
  it('다른 저장소의 자산 URL 은 거부', async () => {
    const bad = JSON.parse(releaseJson()) as { assets: { browser_download_url: string }[] }
    bad.assets[0]!.browser_download_url = 'https://github.com/evil/repo/releases/download/v0.2.0/Antler-0.2.0-arm64.dmg'
    const f = mockFetch({ [`${API}/releases/latest`]: { body: JSON.stringify(bad) } })
    await expect(findManualUpdate(f, { current: '0.1.0', kind: 'mac', arch: 'arm64' })).rejects.toThrow()
  })
  it('요청 한도(403)는 읽기 쉬운 오류로', async () => {
    const f = mockFetch({ [`${API}/releases/latest`]: { status: 403 } })
    await expect(findManualUpdate(f, { current: '0.1.0', kind: 'mac', arch: 'arm64' })).rejects.toThrow('한도')
  })
})

describe('downloadVerified (mock fetch)', () => {
  let dir: string
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-dl-'))
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  const update = (expected: Record<string, unknown>): Parameters<typeof downloadVerified>[1] => ({
    version: '0.2.0',
    notesUrl: 'https://github.com/Kang-Jacob-GitLB/antler/releases/tag/v0.2.0',
    asset: { name: 'Antler-0.2.0-arm64.dmg', size: DMG.length, url: `${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg` },
    expected
  })
  const assetUrl = `${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`

  it('리다이렉트를 따라가 검증 후 최종 이름으로 저장하고 진행률을 보고한다', async () => {
    const cdn = 'https://objects.githubusercontent.com/blob/abc?sig=1'
    const f = mockFetch({ [assetUrl]: { status: 302, headers: { location: cdn } }, [cdn]: { body: DMG } })
    const pct: number[] = []
    const out = await downloadVerified(f, update({ sha512: sha512b64(DMG), size: DMG.length }), dir, (p) => pct.push(p))
    expect(readFileSync(out).equals(DMG)).toBe(true)
    expect(readdirSync(dir)).toEqual(['Antler-0.2.0-arm64.dmg'])
    expect(pct.at(-1)).toBe(100)
    expect(f.calls).toEqual([assetUrl, cdn])
  })
  it('해시가 틀리면 파일을 남기지 않고 실패한다', async () => {
    const f = mockFetch({ [assetUrl]: { body: DMG } })
    await expect(downloadVerified(f, update({ sha512: sha512b64(Buffer.from('other')) }), dir, () => {})).rejects.toThrow('검증 실패')
    expect(readdirSync(dir)).toEqual([])
  })
  it('허용되지 않은 호스트로의 리다이렉트는 따라가지 않는다', async () => {
    const f = mockFetch({ [assetUrl]: { status: 302, headers: { location: 'https://evil.example.com/x' } }, 'https://evil.example.com/x': { body: DMG } })
    await expect(downloadVerified(f, update({ sha256: sha256hex(DMG) }), dir, () => {})).rejects.toThrow('허용되지 않은')
    expect(f.calls).toEqual([assetUrl])
    expect(readdirSync(dir)).toEqual([])
  })
  it('검증 정보가 하나도 없으면 내려받지 않는다', async () => {
    const f = mockFetch({ [assetUrl]: { body: DMG } })
    await expect(downloadVerified(f, update({}), dir, () => {})).rejects.toThrow('검증 정보가 없어 자동으로 열지 않았습니다')
    // 크기만 있어도 거부
    await expect(downloadVerified(f, update({ size: DMG.length }), dir, () => {})).rejects.toThrow('검증 정보가 없어')
    expect(f.calls).toEqual([])
  })
  it('크기만 있을 때 크기가 다르면 실패', async () => {
    const f = mockFetch({ [assetUrl]: { body: DMG.subarray(0, 50) } })
    await expect(downloadVerified(f, update({ sha256: sha256hex(DMG), size: DMG.length }), dir, () => {})).rejects.toThrow('크기')
    expect(readdirSync(dir)).toEqual([])
  })
})

// ── UpdateService ────────────────────────────────────────────────────

function makeService(over: Partial<UpdateDeps> = {}, backend?: Partial<AutoBackend>) {
  const sent: UpdateState[] = []
  let skipped: string | undefined
  const openPath = vi.fn(async () => '')
  const showItem = vi.fn()
  const dir = mkdtempSync(join(tmpdir(), 'antler-svc-'))
  const be: AutoBackend = { check: async () => '0.2.0', download: async (p) => p(50), cancel: () => {}, quitAndInstall: vi.fn(), ...backend }
  const svc = new UpdateService({
    currentVersion: '0.1.0',
    platform: 'win32',
    arch: 'x64',
    env: {},
    isPackaged: true,
    getSkipped: () => skipped,
    setSkipped: (v) => (skipped = v),
    send: (s) => sent.push(s),
    createAutoBackend: () => be,
    downloadsDir: () => dir,
    openPath,
    showItem,
    ...over
  })
  return { svc, sent, openPath, showItem, be, dir, skipped: () => skipped }
}

describe('UpdateService', () => {
  it('auto: 확인 → 내려받기 → 설치', async () => {
    const { svc, be } = makeService()
    expect((await svc.check(true)).status).toBe('available')
    expect(svc.get().version).toBe('0.2.0')
    expect(svc.mode).toBe('auto')
    const done = await svc.download()
    expect(done.status).toBe('downloaded')
    await svc.install()
    expect(be.quitAndInstall).toHaveBeenCalled()
  })
  it('개발 빌드(미패키지)는 확인하지 않는다', async () => {
    const { svc, be } = makeService({ isPackaged: false })
    const spy = vi.spyOn(be, 'check')
    expect((await svc.check(false)).status).toBe('idle')
    expect((await svc.check(true)).status).toBe('not-available')
    expect(spy).not.toHaveBeenCalled()
  })
  it('나중에: 자동 확인은 숨기고 수동 확인은 다시 보여 준다', async () => {
    const { svc } = makeService()
    await svc.check(true)
    expect(svc.dismiss('later', '0.2.0').status).toBe('idle')
    expect((await svc.check(false)).status).toBe('idle')
    expect((await svc.check(true)).status).toBe('available')
  })
  it('건너뛰기: 설정에 저장하고 더 새 버전은 알린다', async () => {
    let latest = '0.2.0'
    const h = makeService({}, { check: async () => latest })
    await h.svc.check(true)
    h.svc.dismiss('skip', '0.2.0')
    expect(h.skipped()).toBe('0.2.0')
    expect((await h.svc.check(false)).status).toBe('idle')
    latest = '0.2.1'
    expect((await h.svc.check(false)).status).toBe('available')
  })
  it('dismiss 는 지금 알리는 버전만 받는다(임의 버전으로 알림을 끌 수 없다)', async () => {
    const h = makeService()
    expect(() => h.svc.dismiss('skip', '0.2.0')).toThrow('알림 중인') // 아직 알림 없음(idle)
    await h.svc.check(true)
    expect(() => h.svc.dismiss('skip', '999999.0.0')).toThrow('알림 중인')
    expect(() => h.svc.dismiss('later', '0.9.0')).toThrow('알림 중인')
    expect(h.skipped()).toBeUndefined()
    expect(h.svc.get().status).toBe('available')
    h.svc.dismiss('skip', 'v0.2.0')
    expect(h.skipped()).toBe('0.2.0')
  })
  it('ConfigStore: renderer 의 setUi 로는 건너뛴 버전을 바꿀 수 없다', () => {
    const dir = mkdtempSync(join(tmpdir(), 'antler-cfg-'))
    try {
      const store = new ConfigStore(join(dir, 'c.json'))
      expect(() => store.setUi({ skippedUpdateVersion: '999999.0.0' })).toThrow()
      expect(store.get().ui.skippedUpdateVersion).toBeUndefined()
      store.setUi({ theme: 'dark' }) // 다른 값은 그대로 가능
      expect(() => store.setSkippedUpdateVersion('../x')).toThrow()
      store.setSkippedUpdateVersion('0.2.0')
      expect(store.get().ui.skippedUpdateVersion).toBe('0.2.0')
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
  it('mac: 해시 정보가 전혀 없으면 내려받지도 열지도 않는다', async () => {
    const noHash = JSON.parse(releaseJson()) as { assets: Record<string, unknown>[] }
    delete noHash.assets[0]!['digest']
    const f = mockFetch({ [`${API}/releases/latest`]: { body: JSON.stringify(noHash) } })
    const h = makeService({ platform: 'darwin', arch: 'arm64', fetch: f })
    try {
      await h.svc.check(true)
      const r = await h.svc.download()
      expect(r.status).toBe('error')
      expect(r.error).toContain('자동으로 열지 않았습니다')
      expect(h.openPath).not.toHaveBeenCalled()
      expect(f.calls).not.toContain(`${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`)
    } finally {
      rmSync(h.dir, { recursive: true, force: true })
    }
  })
  it('다시 열기 전에 해시를 다시 확인한다', async () => {
    const f = mockFetch({
      [`${API}/releases/latest`]: { body: releaseJson() },
      [`${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`]: { body: DMG }
    })
    const h = makeService({ platform: 'darwin', arch: 'arm64', fetch: f })
    try {
      await h.svc.check(true)
      await h.svc.download()
      expect(h.openPath).toHaveBeenCalledTimes(1)
      await h.svc.install() // 변조 없음 → 다시 열림
      expect(h.openPath).toHaveBeenCalledTimes(2)
      writeFileSync(join(h.dir, 'Antler-0.2.0-arm64.dmg'), 'tampered')
      await h.svc.install()
      expect(h.openPath).toHaveBeenCalledTimes(2)
      expect(h.svc.get().status).toBe('error')
      expect(h.svc.get().error).toContain('검증 실패')
    } finally {
      rmSync(h.dir, { recursive: true, force: true })
    }
  })
  it('잘못된 dismiss 입력은 던진다', () => {
    const { svc } = makeService()
    expect(() => svc.dismiss('xx', '0.2.0')).toThrow()
    expect(() => svc.dismiss('skip', '; rm -rf')).toThrow()
  })
  it('같은 버전이면 not-available(수동) / idle(자동)', async () => {
    const { svc } = makeService({}, { check: async () => null })
    expect((await svc.check(false)).status).toBe('idle')
    expect((await svc.check(true)).status).toBe('not-available')
  })
  it('자동 확인 실패는 조용히, 수동 확인 실패는 오류로', async () => {
    const { svc } = makeService({}, { check: async () => { throw new Error('net::ERR_INTERNET_DISCONNECTED') } })
    expect((await svc.check(false)).status).toBe('idle')
    expect((await svc.check(true)).status).toBe('error')
  })
  it('내려받기 취소는 available 로 되돌린다', async () => {
    let release: () => void = () => {}
    const { svc } = makeService({}, { download: () => new Promise<void>((_r, rej) => { release = () => rej(new Error('cancelled')) }), cancel: () => release() })
    await svc.check(true)
    const p = svc.download()
    await Promise.resolve()
    expect(svc.get().status).toBe('downloading')
    svc.cancel()
    await p
    expect(svc.get().status).toBe('available')
  })
  it('내려받기 오류 → error, 다시 시도 가능', async () => {
    let fail = true
    const { svc } = makeService({}, { download: async (p) => { if (fail) throw new Error('boom'); p(100) } })
    await svc.check(true)
    expect((await svc.download()).status).toBe('error')
    fail = false
    expect((await svc.download()).status).toBe('downloaded')
  })

  it('mac: dmg 를 검증해 내려받아 연다', async () => {
    const f = mockFetch({
      [`${API}/releases/latest`]: { body: releaseJson() },
      [`${BASE}/v0.2.0/latest-mac.yml`]: { body: metaYml(sha512b64(DMG)) },
      [`${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`]: { body: DMG }
    })
    const h = makeService({ platform: 'darwin', arch: 'arm64', fetch: f })
    try {
      expect(h.svc.mode).toBe('manual-dmg')
      expect((await h.svc.check(true)).status).toBe('available')
      const done = await h.svc.download()
      expect(done.status).toBe('downloaded')
      expect(done.fileName).toBe('Antler-0.2.0-arm64.dmg')
      expect(h.openPath).toHaveBeenCalledWith(join(h.dir, 'Antler-0.2.0-arm64.dmg'))
    } finally {
      rmSync(h.dir, { recursive: true, force: true })
    }
  })
  it('mac: 해시가 틀리면 열지 않는다', async () => {
    const f = mockFetch({
      [`${API}/releases/latest`]: { body: releaseJson() },
      [`${BASE}/v0.2.0/latest-mac.yml`]: { body: metaYml(sha512b64(Buffer.from('tampered'))) },
      [`${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`]: { body: DMG }
    })
    const h = makeService({ platform: 'darwin', arch: 'arm64', fetch: f })
    try {
      await h.svc.check(true)
      const r = await h.svc.download()
      expect(r.status).toBe('error')
      expect(h.openPath).not.toHaveBeenCalled()
      expect(readdirSync(h.dir)).toEqual([])
    } finally {
      rmSync(h.dir, { recursive: true, force: true })
    }
  })
  it('portable: 내려받은 파일 위치를 보여 준다', async () => {
    const exe = Buffer.from('MZ-portable'.repeat(50))
    const f = mockFetch({
      [`${API}/releases/latest`]: {
        body: JSON.stringify({
          tag_name: 'v0.2.0',
          assets: [{ name: 'Antler-Portable-0.2.0.exe', size: exe.length, browser_download_url: `${BASE}/v0.2.0/Antler-Portable-0.2.0.exe`, digest: `sha256:${sha256hex(exe)}` }]
        })
      },
      [`${BASE}/v0.2.0/Antler-Portable-0.2.0.exe`]: { body: exe }
    })
    const h = makeService({ env: { PORTABLE_EXECUTABLE_DIR: 'C:\\p' }, fetch: f })
    try {
      expect(h.svc.mode).toBe('manual-file')
      await h.svc.check(true)
      expect((await h.svc.download()).status).toBe('downloaded')
      expect(h.showItem).toHaveBeenCalledWith(join(h.dir, 'Antler-Portable-0.2.0.exe'))
      expect(h.openPath).not.toHaveBeenCalled()
    } finally {
      rmSync(h.dir, { recursive: true, force: true })
    }
  })
  it('ANTLER_UPDATE_FAKE: 패키지된 앱에서는 무시, DEV=1 이면 허용', () => {
    expect(makeService({ env: { ANTLER_UPDATE_FAKE: 'available' } }).svc.get().status).toBe('idle')
    expect(makeService({ env: { ANTLER_UPDATE_FAKE: 'available', ANTLER_UPDATE_DEV: '1' } }).svc.get().status).toBe('available')
    expect(makeService({ isPackaged: false, env: { ANTLER_UPDATE_FAKE: 'downloading' } }).svc.get().progress).toBe(42)
    expect(makeService({ isPackaged: false, env: { ANTLER_UPDATE_FAKE: 'bogus' } }).svc.get().status).toBe('idle')
  })
})
