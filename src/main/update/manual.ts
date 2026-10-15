// 수동 설치 경로(macOS dmg · Windows portable · Linux deb): GitHub Releases 에서 새 버전과 자산을 찾아
// 검증하며 내려받는다. electron 에 의존하지 않아(fetch·파일 시스템만) 단위 테스트에서 fetch 를 모의할 수 있다.
import { createHash } from 'node:crypto'
import { createWriteStream } from 'node:fs'
import { mkdir, rename, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { Readable, Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import {
  REPO_NAME,
  REPO_OWNER,
  isAllowedDownloadUrl,
  isOfferable,
  isPrerelease,
  isReleaseAssetUrl,
  isSafeFileName,
  metaFor,
  metadataName,
  parseMetadata,
  parseSemver,
  pickAsset,
  releaseNotesUrl,
  sha256FromDigest,
  verifyDigests,
  type Digests,
  type Expected,
  type InstallKind,
  type ReleaseAsset
} from './logic'

type FetchFn = typeof fetch

const API_BASE = `https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}`
const MAX_REDIRECTS = 5
const MAX_API_BYTES = 2_000_000
const MAX_META_BYTES = 1_000_000
const MAX_ASSET_BYTES = 1_500_000_000
const HEADERS = { 'User-Agent': 'Antler-updater', Accept: 'application/vnd.github+json' }

export interface ManualUpdate {
  version: string
  notesUrl: string
  asset: ReleaseAsset
  expected: Expected
}

async function readLimited(res: Response, limit: number): Promise<string> {
  const len = Number(res.headers.get('content-length') ?? 0)
  if (len > limit) throw new Error('응답이 너무 큽니다')
  const text = await res.text()
  if (text.length > limit) throw new Error('응답이 너무 큽니다')
  return text
}

/** 리다이렉트를 직접 따라가며 매 단계 URL 을 허용 목록으로 확인한다(토큰·쿠키는 보내지 않는다) */
async function guardedGet(fetchFn: FetchFn, url: string, firstOk: (u: string) => boolean, signal?: AbortSignal): Promise<Response> {
  let cur = url
  for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
    if (!(hop === 0 ? firstOk(cur) : isAllowedDownloadUrl(cur))) throw new Error('허용되지 않은 다운로드 주소입니다')
    const res = await fetchFn(cur, { redirect: 'manual', headers: { 'User-Agent': HEADERS['User-Agent'] }, ...(signal ? { signal } : {}) })
    if (res.status >= 300 && res.status < 400) {
      const loc = res.headers.get('location')
      if (!loc) throw new Error('잘못된 리다이렉트입니다')
      cur = new URL(loc, cur).href
      continue
    }
    if (!res.ok) throw new Error(`내려받기 실패 (HTTP ${res.status})`)
    return res
  }
  throw new Error('리다이렉트가 너무 많습니다')
}

interface ApiRelease {
  tag_name?: unknown
  draft?: unknown
  prerelease?: unknown
  assets?: unknown
}

function toAssets(raw: unknown): ReleaseAsset[] {
  if (!Array.isArray(raw)) return []
  const out: ReleaseAsset[] = []
  for (const a of raw as Record<string, unknown>[]) {
    if (typeof a?.['name'] !== 'string' || typeof a['browser_download_url'] !== 'string' || typeof a['size'] !== 'number') continue
    const asset: ReleaseAsset = { name: a['name'], url: a['browser_download_url'], size: a['size'] }
    if (typeof a['digest'] === 'string') asset.digest = a['digest']
    out.push(asset)
  }
  return out
}

/** 최신 릴리스. 현재가 프리릴리스면 최근 목록에서(프리릴리스 포함), 아니면 /releases/latest(정식만) */
async function latestRelease(fetchFn: FetchFn, allowPre: boolean, signal?: AbortSignal): Promise<ApiRelease | null> {
  const url = allowPre ? `${API_BASE}/releases?per_page=10` : `${API_BASE}/releases/latest`
  const res = await fetchFn(url, { headers: HEADERS, redirect: 'error', ...(signal ? { signal } : {}) })
  if (res.status === 404) return null
  if (res.status === 403 || res.status === 429) throw new Error('GitHub 요청 한도에 걸렸습니다. 잠시 후 다시 시도하세요')
  if (!res.ok) throw new Error(`릴리스 정보를 가져오지 못했습니다 (HTTP ${res.status})`)
  const json = JSON.parse(await readLimited(res, MAX_API_BYTES)) as unknown
  if (Array.isArray(json)) return ((json as ApiRelease[]).find((r) => r.draft !== true) ?? null)
  return json as ApiRelease
}

/**
 * 새 버전이 있으면 설치 형태에 맞는 자산과 기대 해시를 돌려준다. 없으면 null.
 * 해시 우선순위: latest*.yml 의 sha512 → API digest(sha256) → 크기만(그마저 없으면 내려받기 거부)
 */
export async function findManualUpdate(
  fetchFn: FetchFn,
  opts: { current: string; kind: InstallKind; arch: string; signal?: AbortSignal }
): Promise<ManualUpdate | null> {
  const rel = await latestRelease(fetchFn, isPrerelease(opts.current), opts.signal)
  if (!rel || typeof rel.tag_name !== 'string' || rel.draft === true) return null
  const version = rel.tag_name.replace(/^v/, '')
  if (!parseSemver(version) || !isOfferable(opts.current, version)) return null
  if (rel.prerelease === true && !isPrerelease(opts.current)) return null

  const assets = toAssets(rel.assets)
  const asset = pickAsset(assets, opts.kind, opts.arch, version)
  if (!asset) throw new Error('이 환경에 맞는 설치 파일이 릴리스에 없습니다')
  if (!isReleaseAssetUrl(asset.url) || asset.size <= 0 || asset.size > MAX_ASSET_BYTES) throw new Error('릴리스 자산 정보가 올바르지 않습니다')

  const expected: Expected = { size: asset.size }
  const sha256 = sha256FromDigest(asset.digest)
  if (sha256) expected.sha256 = sha256
  const metaAsset = assets.find((a) => a.name === metadataName(opts.kind))
  if (metaAsset && isReleaseAssetUrl(metaAsset.url)) {
    try {
      const res = await guardedGet(fetchFn, metaAsset.url, isReleaseAssetUrl, opts.signal)
      const meta = parseMetadata(await readLimited(res, MAX_META_BYTES))
      const entry = meta && meta.version === version ? metaFor(meta, asset.name) : null
      if (entry?.sha512) expected.sha512 = entry.sha512
    } catch (e) {
      if (opts.signal?.aborted) throw e
      // 메타데이터를 못 읽으면 API digest·크기로 계속한다
    }
  }
  return { version, notesUrl: releaseNotesUrl(version), asset, expected }
}

/**
 * 자산을 내려받아 해시를 검증한 뒤 dir 에 최종 이름으로 저장하고 경로를 돌려준다.
 * 검증 전에는 `<이름>.part` 로만 존재하고, 실패하면 지운다 — 검증에 실패한 파일은 절대 열지 않는다.
 */
export async function downloadVerified(
  fetchFn: FetchFn,
  update: ManualUpdate,
  dir: string,
  onProgress: (percent: number) => void,
  signal?: AbortSignal
): Promise<string> {
  const { asset, expected } = update
  if (!isSafeFileName(asset.name)) throw new Error('잘못된 파일 이름입니다')
  // 해시(sha512 또는 sha256)가 없으면 검증할 수 없으므로 받지도, 열지도 않는다(크기만으로는 통과시키지 않는다)
  if (!expected.sha512 && !expected.sha256) throw new Error('검증 정보가 없어 자동으로 열지 않았습니다')
  await mkdir(dir, { recursive: true, mode: 0o700 })
  const part = join(dir, `${asset.name}.part`)
  const final = join(dir, asset.name)
  try {
    // 이전 실행이 남긴 .part 는 이 전용 폴더 안에서만 지운다. 새 파일은 'wx' 로 열어 이미 있으면(심볼릭 링크 포함) 실패한다
    await rm(part, { force: true })
    const res = await guardedGet(fetchFn, asset.url, isReleaseAssetUrl, signal)
    if (!res.body) throw new Error('응답 본문이 없습니다')
    const h512 = createHash('sha512')
    const h256 = createHash('sha256')
    let size = 0
    let lastPct = -1
    const total = asset.size
    const meter = new Transform({
      transform(chunk: Buffer, _enc, cb) {
        size += chunk.length
        if (size > MAX_ASSET_BYTES || size > total) return cb(new Error('예상보다 큰 파일입니다'))
        h512.update(chunk)
        h256.update(chunk)
        const pct = Math.min(99, Math.floor((size / total) * 100))
        if (pct !== lastPct) {
          lastPct = pct
          onProgress(pct)
        }
        cb(null, chunk)
      }
    })
    await pipeline(Readable.fromWeb(res.body as Parameters<typeof Readable.fromWeb>[0]), meter, createWriteStream(part, { mode: 0o600, flags: 'wx' }), {
      ...(signal ? { signal } : {})
    })
    const actual: Digests = { sha512: h512.digest('base64'), sha256: h256.digest('hex'), size }
    const v = verifyDigests(actual, expected)
    if (!v.ok) throw new Error(`다운로드 검증 실패: ${v.reason}`)
    await rm(final, { force: true })
    await rename(part, final)
    onProgress(100)
    return final
  } catch (e) {
    await rm(part, { force: true })
    throw e
  }
}
