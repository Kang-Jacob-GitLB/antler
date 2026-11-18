// 업데이트 서비스 상태 순서 골든의 TS 쪽 실행기(Phase 6c PR 4, 수동 경로는 PR 4b). 입력은 tests/fixtures/update.scenarios.json 이고,
// 실제 UpdateService 를 돌려 상태가 나간 순서(sent)·단계 결과(results)·호출 횟수(calls)를 모은다. 네트워크를 쓰지 않는다.
//  - auto(기본): 가짜 AutoBackend
//  - manual(`dmg`·`deb`): 실제 manual.ts(findManualUpdate·downloadVerified)를 가짜 fetch·임시 폴더로 돌린다. 내려받기 단계의 `chunks`(바이트 수 목록)가 TS 진행률을 정하고,
//    같은 단계의 `progress`(Rust 가짜 백엔드가 내는 값)는 TS 가 보지 않는다 — 둘이 같은 상태 순서를 낳는지가 골든의 요점이다
// 같은 입력을 Rust 서비스(src-tauri/tests/update.rs)가 돌려 이 출력(tests/golden/update/scenarios.json)과 같은지 본다.
import { createHash } from 'node:crypto'
import { appendFileSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { UpdateState } from '../../src/shared/types'
import { UpdateService, type AutoBackend } from '../../src/main/update/UpdateService'

type CheckStep = { version: string } | { none: true } | { error: string }
/** progress·error·hang 은 auto 와 Rust 가 쓴다. chunks·openFails 는 TS 수동 경로의 가짜 fetch·열기가 쓴다 */
type DownloadStep = { progress?: number[]; error?: string; hang?: true; chunks?: number[]; openFails?: true }
type InstallStep = { ok: true } | { error: string }
type Step =
  | { op: 'check'; manual: boolean }
  | { op: 'download' | 'downloadStart' | 'downloadEnd' | 'cancel' | 'install' | 'get' | 'tamper' }
  | { op: 'dismiss'; action: unknown; version: unknown }

export interface Scenario {
  name: string
  current?: string
  enabled?: boolean
  skipped?: string | null
  fake?: string | null
  /** 수동 경로: dmg(macOS) · deb(Linux). 없으면 auto(Windows NSIS) */
  manual?: 'dmg' | 'deb'
  backend: { checks?: CheckStep[]; downloads?: DownloadStep[]; installs?: InstallStep[] }
  steps: Step[]
}

/** 호출 순서대로 하나씩 쓰고 마지막 것을 반복한다. 비었으면 `fallback` */
const pick = <T>(list: T[] | undefined, i: number, fallback: T): T => (list && list.length > 0 ? list[Math.min(i, list.length - 1)]! : fallback)

/** JSON 을 거쳐 undefined 필드를 버린다(Rust 의 skip_serializing_if 와 같은 모양) */
const plain = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T

export const DEFAULT_CURRENT = '0.1.0'

const DOWNLOAD_BASE = 'https://github.com/Kang-Jacob-GitLB/antler/releases/download'
const LATEST_API = 'https://api.github.com/repos/Kang-Jacob-GitLB/antler/releases/latest'

const assetName = (manual: 'dmg' | 'deb', version: string): string => (manual === 'dmg' ? `Antler-${version}-arm64.dmg` : `antler_${version}_amd64.deb`)

export async function runScenario(s: Scenario): Promise<unknown> {
  const sent: UpdateState[] = []
  let skipped: string | undefined = s.skipped ?? undefined
  const calls = { check: 0, download: 0, install: 0 }
  let release: () => void = () => {}
  const fake = s.fake ?? undefined
  const dir = s.manual ? mkdtempSync(join(tmpdir(), 'antler-update-golden-')) : ''
  let openFails = false
  let downloaded: string | null = null

  const backend: AutoBackend = {
    check: async () => {
      const step = pick<CheckStep>(s.backend.checks, calls.check++, { none: true })
      if ('error' in step) throw new Error(step.error)
      return 'version' in step ? step.version : null
    },
    download: async (progress) => {
      const step = pick<DownloadStep>(s.backend.downloads, calls.download++, {})
      for (const p of step.progress ?? []) progress(p)
      if (step.hang) {
        await new Promise<void>((_resolve, reject) => {
          release = () => reject(new Error('cancelled'))
        })
      }
      if (step.error !== undefined) throw new Error(step.error)
    },
    cancel: () => release(),
    quitAndInstall: () => {
      const step = pick<InstallStep>(s.backend.installs, calls.install++, { ok: true })
      if ('error' in step) throw new Error(step.error)
    }
  }

  // 수동 경로의 가짜 fetch: 릴리스 API → (check 단계) · 자산 → (download 단계). 자산 내용은 'a' 를 chunks 합만큼 — 해시는 API digest 로 준다
  let lastVersion = ''
  const manualFetch = (async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const url = String(input)
    if (url === LATEST_API) {
      const step = pick<CheckStep>(s.backend.checks, calls.check++, { none: true })
      if ('error' in step) throw new Error(step.error)
      if (!('version' in step)) return new Response('{}', { status: 404 })
      lastVersion = step.version
      const total = (pick<DownloadStep>(s.backend.downloads, 0, {}).chunks ?? [1000]).reduce((a, b) => a + b, 0)
      const digest = `sha256:${createHash('sha256').update(Buffer.alloc(total, 'a')).digest('hex')}`
      const name = assetName(s.manual!, step.version)
      const body = { tag_name: `v${step.version}`, draft: false, prerelease: false, assets: [{ name, size: total, digest, browser_download_url: `${DOWNLOAD_BASE}/v${step.version}/${name}` }] }
      return new Response(JSON.stringify(body), { status: 200 })
    }
    if (url.startsWith(`${DOWNLOAD_BASE}/v${lastVersion}/`)) {
      const step = pick<DownloadStep>(s.backend.downloads, calls.download++, {})
      openFails = step.openFails === true
      if (step.error !== undefined) throw new Error(step.error)
      const stream = new ReadableStream<Uint8Array>({
        start(controller) {
          if (step.hang) {
            init?.signal?.addEventListener('abort', () => controller.error(new Error('cancelled')))
            return
          }
          for (const n of step.chunks ?? [1000]) controller.enqueue(new Uint8Array(Buffer.alloc(n, 'a')))
          controller.close()
        }
      })
      return new Response(stream, { status: 200 })
    }
    return new Response('', { status: 404 })
  }) as typeof fetch

  const kindEnv =
    s.manual === 'dmg'
      ? { platform: 'darwin', arch: 'arm64', probe: undefined }
      : s.manual === 'deb'
        ? { platform: 'linux', arch: 'x64', probe: { packageType: 'deb' } }
        : { platform: 'win32', arch: 'x64', probe: undefined }
  const env: Record<string, string | undefined> = fake ? { ANTLER_UPDATE_FAKE: fake } : {}
  if (fake && s.manual) env['ANTLER_UPDATE_FAKE_MODE'] = s.manual === 'dmg' ? 'manual-dmg' : 'manual-file'
  const svc = new UpdateService({
    currentVersion: s.current ?? DEFAULT_CURRENT,
    platform: kindEnv.platform,
    arch: kindEnv.arch,
    env,
    // 사용할 수 없는 빌드 = 미패키지. 개발 장치(가짜 상태)도 미패키지에서만 켜진다
    isPackaged: s.enabled ?? !fake,
    ...(kindEnv.probe ? { probe: kindEnv.probe } : {}),
    getSkipped: () => skipped,
    setSkipped: (v) => (skipped = v),
    send: (state) => sent.push(plain(state)),
    createAutoBackend: () => backend,
    ...(s.manual ? { fetch: manualFetch } : {}),
    downloadsDir: () => dir,
    openPath: async (p) => {
      downloaded = p
      return openFails ? 'denied' : ''
    },
    showItem: () => {}
  })
  const results: unknown[] = []
  let started: Promise<UpdateState> | null = null
  try {
    for (const st of s.steps) {
      switch (st.op) {
        case 'check':
          results.push({ op: 'check', state: plain(await svc.check(st.manual)) })
          break
        case 'download':
          results.push({ op: 'download', state: plain(await svc.download()) })
          break
        case 'downloadStart':
          started = svc.download()
          // 상태가 downloading 이 되고 백엔드가 멈출 때까지 기다린다
          for (let i = 0; i < 20; i++) await Promise.resolve()
          results.push({ op: 'downloadStart' })
          break
        case 'downloadEnd':
          results.push({ op: 'downloadEnd', state: started ? plain(await started) : null })
          started = null
          break
        case 'cancel':
          svc.cancel()
          results.push({ op: 'cancel', state: plain(svc.get()) })
          break
        case 'install':
          // 수동 경로의 설치 = 파일 다시 열기(백엔드 호출 대신 여기서 센다 — Rust 의 `Backend::install` 호출 수와 맞춘다)
          if (s.manual && svc.get().status === 'downloaded' && downloaded) calls.install++
          await svc.install()
          results.push({ op: 'install', state: plain(svc.get()) })
          break
        case 'tamper':
          // 받은 파일이 디스크에서 바뀐다(수동 경로의 다시 열기 전 검증 시험)
          if (downloaded) appendFileSync(downloaded, 'x')
          results.push({ op: 'tamper' })
          break
        case 'get':
          results.push({ op: 'get', state: plain(svc.get()) })
          break
        case 'dismiss':
          try {
            results.push({ op: 'dismiss', state: plain(svc.dismiss(st.action, st.version)) })
          } catch (e) {
            results.push({ op: 'dismiss', error: (e as Error).message })
          }
          break
      }
    }
  } finally {
    if (dir) rmSync(dir, { recursive: true, force: true })
  }
  return { name: s.name, sent, results, skipped: skipped ?? null, calls }
}

export async function runScenarios(list: Scenario[]): Promise<unknown[]> {
  const out: unknown[] = []
  for (const s of list) out.push(await runScenario(s))
  return out
}
