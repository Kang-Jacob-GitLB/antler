// 업데이트의 순수 로직(electron 비의존 — 단위 테스트 대상): 버전 비교, 설치 형태 판별, 자산 선택,
// 다운로드 URL 허용 목록, 메타데이터(latest*.yml) 파싱, 해시 검증, 나중에/건너뛰기 정책, IPC 입력 검증.
import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import type { UpdateDismissAction, UpdateMode } from '@shared/types'

export const REPO_OWNER = 'Kang-Jacob-GitLB'
export const REPO_NAME = 'antler'
export const RELEASE_DOWNLOAD_PREFIX = `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/`

// ── semver ───────────────────────────────────────────────────────────

export interface Semver {
  major: number
  minor: number
  patch: number
  pre: string[]
}

const SEMVER_RE = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z.-]+)?$/

export function parseSemver(v: unknown): Semver | null {
  if (typeof v !== 'string' || v.length > 64) return null
  const m = SEMVER_RE.exec(v.trim())
  if (!m) return null
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ? m[4].split('.') : [] }
}

/** a<b → -1, a==b → 0, a>b → 1 (semver 2.0 우선순위 규칙). 해석 불가면 null */
export function compareSemver(a: string, b: string): -1 | 0 | 1 | null {
  const x = parseSemver(a)
  const y = parseSemver(b)
  if (!x || !y) return null
  for (const k of ['major', 'minor', 'patch'] as const) {
    if (x[k] !== y[k]) return x[k] < y[k] ? -1 : 1
  }
  if (!x.pre.length || !y.pre.length) return x.pre.length === y.pre.length ? 0 : x.pre.length ? -1 : 1
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i]
    const q = y.pre[i]
    if (p === undefined) return -1
    if (q === undefined) return 1
    if (p === q) continue
    const pn = /^\d+$/.test(p)
    const qn = /^\d+$/.test(q)
    if (pn && qn) return Number(p) < Number(q) ? -1 : 1
    if (pn !== qn) return pn ? -1 : 1
    return p < q ? -1 : 1
  }
  return 0
}

export const isPrerelease = (v: string): boolean => (parseSemver(v)?.pre.length ?? 0) > 0

/** candidate 가 알림 대상인가: current 보다 새 버전이고, 프리릴리스는 current 도 프리릴리스일 때만 */
export function isOfferable(current: string, candidate: string): boolean {
  if (compareSemver(candidate, current) !== 1) return false
  return !isPrerelease(candidate) || isPrerelease(current)
}

// ── 설치 형태 ────────────────────────────────────────────────────────

export type InstallKind = 'nsis' | 'portable' | 'appimage' | 'deb' | 'mac' | 'unsupported'

export interface InstallProbe {
  /** process.execPath */
  execPath?: string
  /** resources/package-type 파일 내용(electron-builder 가 deb·rpm·pacman 패키지에 넣는다). 없으면 undefined */
  packageType?: string
}

/**
 * 설치 형태. Linux 는 환경 변수만 믿지 않는다(부모 프로세스의 APPIMAGE 가 상속될 수 있다):
 *  - package-type 이 deb 면 deb
 *  - APPIMAGE·APPDIR 가 있고 실행 파일이 APPDIR 안에 있을 때만 AppImage
 *  - 그 밖(rpm·pacman·압축 해제본 등)은 지원하지 않는다
 */
export function detectInstallKind(platform: string, env: Record<string, string | undefined>, probe: InstallProbe = {}): InstallKind {
  if (platform === 'win32') return env['PORTABLE_EXECUTABLE_DIR'] ? 'portable' : 'nsis'
  if (platform === 'darwin') return 'mac'
  if (platform === 'linux') {
    if (probe.packageType?.trim() === 'deb') return 'deb'
    const appDir = env['APPDIR']
    const exe = probe.execPath
    if (env['APPIMAGE'] && appDir && exe && exe.startsWith(appDir.endsWith('/') ? appDir : `${appDir}/`)) return 'appimage'
    return 'unsupported'
  }
  return 'unsupported'
}

export function modeFor(kind: InstallKind): UpdateMode {
  if (kind === 'nsis' || kind === 'appimage') return 'auto'
  return kind === 'mac' ? 'manual-dmg' : 'manual-file'
}

// ── 릴리스 자산 선택 ─────────────────────────────────────────────────

export interface ReleaseAsset {
  name: string
  size: number
  url: string
  /** GitHub API 의 "sha256:<hex>" (있는 경우) */
  digest?: string
}

/** 자산 파일 이름으로 쓸 수 있는 안전한 이름인가(경로 구분자·제어 문자 없음) */
export const isSafeFileName = (n: string): boolean => /^[A-Za-z0-9][A-Za-z0-9._-]{0,200}$/.test(n) && !n.includes('..')

/** process.arch → 자산 이름의 아키텍처 */
const debArch = (arch: string): string => (arch === 'x64' ? 'amd64' : arch)

/** 설치 형태·아키텍처에 맞는 자산 하나. 이름은 electron-builder.yml 의 artifactName 규칙을 따른다 */
export function pickAsset(assets: ReleaseAsset[], kind: InstallKind, arch: string, version: string): ReleaseAsset | null {
  const v = version.replace(/^v/, '')
  const find = (re: RegExp): ReleaseAsset | null => assets.find((a) => re.test(a.name) && isSafeFileName(a.name)) ?? null
  const esc = (s: string): string => s.replace(/[.*+?^${}()|[\]\\-]/g, '\\$&')
  switch (kind) {
    case 'mac':
      // Antler-0.2.0-arm64.dmg / Antler-0.2.0-x64.dmg
      return find(new RegExp(`^Antler-${esc(v)}-${esc(arch)}\\.dmg$`))
    case 'portable':
      return find(new RegExp(`^Antler-Portable-${esc(v)}\\.exe$`))
    case 'deb':
      // electron-builder 기본 deb 이름: antler_0.2.0_amd64.deb
      return find(new RegExp(`^antler_${esc(v)}_${esc(debArch(arch))}\\.deb$`))
    default:
      return null
  }
}

/** 메타데이터 파일 이름(electron-builder 가 올리는 latest*.yml) */
export function metadataName(kind: InstallKind): string | null {
  if (kind === 'mac') return 'latest-mac.yml'
  if (kind === 'portable') return 'latest.yml'
  if (kind === 'deb') return 'latest-linux.yml'
  return null
}

// ── URL 허용 목록 ────────────────────────────────────────────────────

const DOWNLOAD_HOSTS = new Set(['github.com', 'objects.githubusercontent.com', 'release-assets.githubusercontent.com'])

/** 내려받기 요청(리다이렉트 포함)에 허용하는 URL: https + 허용 호스트 + 기본 포트 + 자격 증명 없음 */
export function isAllowedDownloadUrl(raw: string): boolean {
  let u: URL
  try {
    u = new URL(raw)
  } catch {
    return false
  }
  return u.protocol === 'https:' && DOWNLOAD_HOSTS.has(u.hostname) && !u.port && !u.username && !u.password
}

/**
 * 첫 요청은 이 저장소의 릴리스 자산 URL 이어야 한다(리다이렉트 후 호스트는 isAllowedDownloadUrl 로 확인).
 * URL 을 정규화한 뒤 접두사를 비교하고(`..` 로 다른 저장소 경로로 빠지는 것 방지), 인코딩된 점·슬래시·백슬래시와 `..` 세그먼트는 거부한다.
 */
export function isReleaseAssetUrl(raw: string): boolean {
  if (!isAllowedDownloadUrl(raw)) return false
  if (/%(2e|2f|5c)/i.test(raw) || raw.includes('\\')) return false
  const u = new URL(raw)
  if (u.search || u.hash || u.pathname.split('/').some((s) => s === '..' || s === '.')) return false
  return u.href.startsWith(RELEASE_DOWNLOAD_PREFIX) && u.href === raw
}

/** 릴리스 노트 페이지 URL. 태그는 v + 버전 */
export const releaseNotesUrl = (version: string): string =>
  `https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/tag/v${version.replace(/^v/, '')}`

// ── latest*.yml 파싱 ─────────────────────────────────────────────────

export interface MetaFile {
  url: string
  sha512?: string
  size?: number
}
export interface Metadata {
  version: string
  files: MetaFile[]
}

const unquote = (s: string): string => s.trim().replace(/^(['"])(.*)\1$/, '$2')

/** electron-builder 가 만든 단순한 YAML(version, files[{url,sha512,size}])만 읽는다 */
export function parseMetadata(text: string): Metadata | null {
  if (text.length > 1_000_000) return null
  let version = ''
  const files: MetaFile[] = []
  let inFiles = false
  let cur: MetaFile | null = null
  for (const raw of text.split(/\r?\n/)) {
    if (!raw.trim() || raw.trimStart().startsWith('#')) continue
    if (!/^\s/.test(raw)) {
      inFiles = /^files:\s*$/.test(raw)
      cur = null
      const m = /^version:\s*(.+)$/.exec(raw)
      if (m) version = unquote(m[1]!)
      continue
    }
    if (!inFiles) continue
    const item = /^\s*-\s+(\w+):\s*(.*)$/.exec(raw)
    const prop = item ?? /^\s+(\w+):\s*(.*)$/.exec(raw)
    if (!prop) continue
    if (item) {
      cur = { url: '' }
      files.push(cur)
    }
    if (!cur) continue
    const val = unquote(prop[2]!)
    if (prop[1] === 'url') cur.url = val
    else if (prop[1] === 'sha512') cur.sha512 = val
    else if (prop[1] === 'size' && /^\d+$/.test(val)) cur.size = Number(val)
  }
  if (!parseSemver(version)) return null
  return { version, files: files.filter((f) => f.url) }
}

/** 메타데이터에서 자산 이름과 일치하는 항목(GitHub 업로드 시 공백이 '-' 로 바뀌는 것을 감안) */
export function metaFor(meta: Metadata, assetName: string): MetaFile | null {
  const norm = (s: string): string => s.replace(/ /g, '-')
  return meta.files.find((f) => norm(decodeURIComponent(f.url.split('/').pop() ?? '')) === assetName) ?? null
}

// ── 해시 검증 ────────────────────────────────────────────────────────

export interface Expected {
  /** base64 (latest*.yml 형식) */
  sha512?: string
  /** 소문자 hex (GitHub API digest "sha256:…") */
  sha256?: string
  size?: number
}

/** API digest("sha256:<hex>") → hex. 형식이 다르면 undefined */
export function sha256FromDigest(d: unknown): string | undefined {
  if (typeof d !== 'string') return undefined
  const m = /^sha256:([0-9a-fA-F]{64})$/.exec(d)
  return m ? m[1]!.toLowerCase() : undefined
}

export interface Digests {
  sha512: string
  sha256: string
  size: number
}

export function digestFile(file: string): Promise<Digests> {
  return new Promise((resolve, reject) => {
    const h512 = createHash('sha512')
    const h256 = createHash('sha256')
    let size = 0
    const s = createReadStream(file)
    s.on('data', (c) => {
      const b = c as Buffer
      h512.update(b)
      h256.update(b)
      size += b.length
    })
    s.on('error', reject)
    s.on('end', () => resolve({ sha512: h512.digest('base64'), sha256: h256.digest('hex'), size }))
  })
}

export type VerifyResult = { ok: true; by: 'sha512' | 'sha256' } | { ok: false; reason: string }

/**
 * 계산된 값이 기대값과 맞는지. sha512 또는 sha256 기대값이 반드시 있어야 하며(크기만으로는 통과시키지 않는다),
 * 있는 해시는 모두 일치해야 한다. 크기 기대값이 있으면 그것도 맞아야 한다.
 */
export function verifyDigests(actual: Digests, exp: Expected): VerifyResult {
  if (!exp.sha512 && !exp.sha256) return { ok: false, reason: '검증 정보가 없어 자동으로 열지 않았습니다' }
  if (exp.size !== undefined && exp.size !== actual.size) return { ok: false, reason: '파일 크기가 맞지 않습니다' }
  if (exp.sha512 && exp.sha512 !== actual.sha512) return { ok: false, reason: 'sha512 검증에 실패했습니다' }
  if (exp.sha256 && exp.sha256 !== actual.sha256) return { ok: false, reason: 'sha256 검증에 실패했습니다' }
  return { ok: true, by: exp.sha512 ? 'sha512' : 'sha256' }
}

export async function verifyFile(file: string, exp: Expected): Promise<VerifyResult> {
  return verifyDigests(await digestFile(file), exp)
}

// ── 나중에 / 건너뛰기 ────────────────────────────────────────────────

/** 자동 알림 억제 정책. snooze 는 메모리에만(앱을 다시 시작하면 풀림), skip 은 설정에 저장된 값을 받아 쓴다 */
export class UpdatePolicy {
  private readonly snoozed = new Set<string>()

  constructor(private skipped: () => string | undefined) {}

  snooze(version: string): void {
    this.snoozed.add(version)
  }

  /** manual = 사용자가 직접 확인(항상 보여 줌). 자동 확인은 나중에·건너뛴 버전을 숨긴다 */
  shouldNotify(version: string, manual: boolean): boolean {
    if (manual) return true
    if (this.snoozed.has(version)) return false
    const skip = this.skipped()
    // 건너뛴 버전과 같거나 더 낮은 버전은 숨기고, 더 새 버전이 나오면 다시 알린다
    return !(skip && compareSemver(version, skip) !== 1)
  }
}

// ── IPC 입력 검증 ────────────────────────────────────────────────────

export function parseDismiss(action: unknown, version: unknown): { action: UpdateDismissAction; version: string } {
  if (action !== 'later' && action !== 'skip') throw new Error('잘못된 입력입니다')
  if (typeof version !== 'string' || !parseSemver(version)) throw new Error('잘못된 버전입니다')
  return { action, version: version.replace(/^v/, '') }
}

/** 사용자에게 보여 줄 오류 문구: 토큰·URL 쿼리·로컬 경로를 제거하고 길이를 제한한다 */
export function sanitizeError(err: unknown): string {
  let s = err instanceof Error ? err.message : String(err)
  s = s
    .replace(/https?:\/\/\S+/g, (u) => {
      try {
        return new URL(u).origin
      } catch {
        return '(url)'
      }
    })
    .replace(/\b(gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g, '***')
    .replace(/(token|authorization|bearer)\s*[:=]?\s*\S+/gi, '$1 ***')
    .replace(/[A-Za-z]:\\[^\s'"]+|\/(?:home|Users|tmp|var)\/[^\s'"]+/g, '(경로)')
  s = s.split('\n')[0] ?? ''
  return s.length > 200 ? `${s.slice(0, 200)}…` : s
}
