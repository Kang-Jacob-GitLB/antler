// 업데이트 서비스 상태 순서 골든의 TS 쪽 실행기(Phase 6c PR 4). 입력은 tests/fixtures/update.scenarios.json 이고,
// 가짜 AutoBackend 를 단 실제 UpdateService 를 돌려 상태가 나간 순서(sent)·단계 결과(results)·호출 횟수(calls)를 모은다. 네트워크·파일을 쓰지 않는다.
// 같은 입력을 Rust 서비스(src-tauri/tests/update.rs)가 돌려 이 출력(tests/golden/update/scenarios.json)과 같은지 본다.
import type { UpdateState } from '../../src/shared/types'
import { UpdateService, type AutoBackend } from '../../src/main/update/UpdateService'

type CheckStep = { version: string } | { none: true } | { error: string }
type DownloadStep = { progress?: number[]; error?: string; hang?: true }
type InstallStep = { ok: true } | { error: string }
type Step =
  | { op: 'check'; manual: boolean }
  | { op: 'download' | 'downloadStart' | 'downloadEnd' | 'cancel' | 'install' | 'get' }
  | { op: 'dismiss'; action: unknown; version: unknown }

export interface Scenario {
  name: string
  current?: string
  enabled?: boolean
  skipped?: string | null
  fake?: string | null
  backend: { checks?: CheckStep[]; downloads?: DownloadStep[]; installs?: InstallStep[] }
  steps: Step[]
}

/** 호출 순서대로 하나씩 쓰고 마지막 것을 반복한다. 비었으면 `fallback` */
const pick = <T>(list: T[] | undefined, i: number, fallback: T): T => (list && list.length > 0 ? list[Math.min(i, list.length - 1)]! : fallback)

/** JSON 을 거쳐 undefined 필드를 버린다(Rust 의 skip_serializing_if 와 같은 모양) */
const plain = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T

export const DEFAULT_CURRENT = '0.1.0'

export async function runScenario(s: Scenario): Promise<unknown> {
  const sent: UpdateState[] = []
  let skipped: string | undefined = s.skipped ?? undefined
  const calls = { check: 0, download: 0, install: 0 }
  let release: () => void = () => {}
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
  const fake = s.fake ?? undefined
  const svc = new UpdateService({
    currentVersion: s.current ?? DEFAULT_CURRENT,
    platform: 'win32',
    arch: 'x64',
    env: fake ? { ANTLER_UPDATE_FAKE: fake } : {},
    // 사용할 수 없는 빌드 = 미패키지. 개발 장치(가짜 상태)도 미패키지에서만 켜진다
    isPackaged: s.enabled ?? !fake,
    getSkipped: () => skipped,
    setSkipped: (v) => (skipped = v),
    send: (state) => sent.push(plain(state)),
    createAutoBackend: () => backend,
    downloadsDir: () => '',
    openPath: async () => '',
    showItem: () => {}
  })
  const results: unknown[] = []
  let started: Promise<UpdateState> | null = null
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
        await svc.install()
        results.push({ op: 'install', state: plain(svc.get()) })
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
  return { name: s.name, sent, results, skipped: skipped ?? null, calls }
}

export async function runScenarios(list: Scenario[]): Promise<unknown[]> {
  const out: unknown[] = []
  for (const s of list) out.push(await runScenario(s))
  return out
}
