import { mkdtempSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { IPC_CHANNELS } from '../src/shared/ipc'
import type { AppConfig } from '../src/shared/types'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { DEFAULT_AUTO_FETCH_MINUTES, FetchService } from '../src/main/services/FetchService'
import { RepoService } from '../src/main/services/RepoService'
import { fetchPrefsOf, FetchPrefsSender, type FetchPrefsMessage } from '../src/sidecar/fetchPrefs'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-fetchprefs-')), 'antler.config.json')
const FP_A = 'a'.repeat(64)
const FP_B = 'b'.repeat(64)
const SRC = join(__dirname, '..', 'src')

describe('사이드카 fetchPrefs', () => {
  it('첫 줄은 기본 주기(5분)가 적용된 값과 빈 승인 맵이고, gen 은 1 부터 오른다', () => {
    const sent: FetchPrefsMessage[] = []
    const c = new ConfigStore(tmp())
    const s = new FetchPrefsSender((m) => sent.push(m))
    s.push(c.get())
    expect(sent).toEqual([{ t: 'fetchPrefs', gen: 1, autoFetchMinutes: DEFAULT_AUTO_FETCH_MINUTES, approvals: {}, trusted: {} }])
    expect(DEFAULT_AUTO_FETCH_MINUTES).toBe(5)
  })

  it('값이 바뀔 때만 보낸다: 주기·승인·신뢰 지문이 바뀌면 보내고, 창 위치 같은 무관한 쓰기는 보내지 않는다', () => {
    const sent: FetchPrefsMessage[] = []
    const c = new ConfigStore(tmp())
    const s = new FetchPrefsSender((m) => sent.push(m))
    c.onChange = (cfg) => s.push(cfg)
    s.push(c.get())
    c.setUi({ diffMode: 'split' })
    expect(sent).toHaveLength(1)
    c.setUi({ autoFetchMinutes: 10 })
    expect(sent.at(-1)).toMatchObject({ gen: 2, autoFetchMinutes: 10 })
    const r = c.addRepo({ sourceId: 'local', path: '/tmp/x', name: 'x' })
    expect(sent).toHaveLength(2) // 저장소를 더해도 승인 맵은 그대로
    c.setFetchApproval(r.id, FP_A)
    expect(sent.at(-1)).toMatchObject({ gen: 3, approvals: { [r.id]: FP_A }, trusted: {} })
    c.setTrustedConfig(r.id, FP_B)
    expect(sent.at(-1)).toMatchObject({ gen: 4, approvals: { [r.id]: FP_A }, trusted: { [r.id]: FP_B } })
    // 같은 값을 다시 쓰면 보내지 않는다
    c.setFetchApproval(r.id, FP_A)
    expect(sent).toHaveLength(4)
    // 저장소를 지우면 승인도 사라지고 그 사실이 간다(Rust 가 오버레이를 정리할 수 있게)
    c.removeRepo(r.id)
    expect(sent.at(-1)).toMatchObject({ gen: 5, approvals: {}, trusted: {} })
  })

  it('설정에 없는 저장소의 승인은 담지 않고, 음수·무한·NaN 주기는 0(끔)이다', () => {
    const c = new ConfigStore(tmp())
    const r = c.addRepo({ sourceId: 'local', path: '/tmp/x', name: 'x' })
    const cfg: AppConfig = { ...c.get(), fetchApprovals: { [r.id]: FP_A, ghost: FP_B }, trustedConfigs: { ghost: FP_B } }
    expect(fetchPrefsOf(cfg)).toEqual({ autoFetchMinutes: 5, approvals: { [r.id]: FP_A }, trusted: {} })
    for (const bad of [-3, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(fetchPrefsOf({ ...cfg, ui: { ...cfg.ui, autoFetchMinutes: bad } }).autoFetchMinutes, String(bad)).toBe(0)
    }
    expect(fetchPrefsOf({ ...cfg, ui: { ...cfg.ui, autoFetchMinutes: 0 } }).autoFetchMinutes).toBe(0)
  })

  it('onChange 에서 미러 다음에, 시작 때는 ready 전에 보낸다(순서는 소스로 고정한다)', () => {
    const src = readFileSync(join(SRC, 'sidecar', 'legacy.ts'), 'utf8')
    expect(src).toMatch(/config\.onChange = \(c\) => \{\s*mirror\.push\(c\)\s*fetchPrefs\.push\(c\)\s*\}/)
    const start = src.indexOf('mirror.push(config.get())')
    expect(start).toBeGreaterThan(0)
    expect(src.slice(start, src.indexOf('let netConfig'))).toContain('fetchPrefs.push(config.get())')
    // 기존 미러 보내기는 그대로다(MirrorSender 를 건드리지 않았다)
    expect(src).toContain('const mirror = new MirrorSender(out)')
  })
})

describe('승인 쓰기 길', () => {
  it('승인을 쓰는 코드는 Electron 판 ConfigStore·FetchService 뿐이다: renderer 의 채널로는 쓸 수 없고 사이드카에는 쓰는 코드가 없다(Rust 는 fetch_host.rs 한 곳)', () => {
    // 채널 허용 목록에 승인·신뢰를 쓰는 채널이 없다
    for (const ch of IPC_CHANNELS) expect(ch, ch).not.toMatch(/approv|trust|fetchApprove/i)
    const writers: string[] = []
    const walk = (dir: string): void => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name)
        if (statSync(p).isDirectory()) walk(p)
        else if (/\.tsx?$/.test(name) && /\b(setFetchApproval|setTrustedConfig)\(/.test(readFileSync(p, 'utf8'))) writers.push(p.slice(SRC.length + 1).replace(/\\/g, '/'))
      }
    }
    walk(SRC)
    expect(writers.sort()).toEqual(['main/services/ConfigStore.ts', 'main/services/FetchService.ts'])
    // 사이드카는 `fetchApprove` 줄을 받지 않는다(PR 10 — Rust 가 설정에 직접 쓴다)
    expect(readFileSync(join(SRC, 'sidecar', 'index.ts'), 'utf8')).not.toMatch(/fetchApprove/)
  })
})

describe('FetchService.owns(소유 술어)', () => {
  const made: { fetcher: FetchService; svc: RepoService }[] = []
  afterEach(async () => {
    vi.useRealTimers()
    for (const m of made.splice(0)) {
      m.fetcher.dispose()
      await m.svc.dispose()
    }
  })

  function setup(owns?: (id: string) => boolean): { fetcher: FetchService; repoId: string; statuses: unknown[] } {
    const config = new ConfigStore(tmp())
    const r = config.addRepo({ sourceId: 'local', path: join(tmpdir(), 'antler-owns-nonexistent'), name: 'r' })
    const svc = new RepoService(config)
    const statuses: unknown[] = []
    const fetcher = new FetchService(config, svc, (s) => statuses.push(s), () => {}, null, owns)
    made.push({ fetcher, svc })
    return { fetcher, repoId: r.id, statuses }
  }

  it('기본은 전부 소유한다(Electron 판 동작 불변): fetch 는 상태를 돌려주고 자동 fetch 가 예약된다', async () => {
    vi.useFakeTimers()
    const { fetcher, repoId, statuses } = setup()
    const base = vi.getTimerCount()
    fetcher.setActive(repoId)
    expect(vi.getTimerCount()).toBe(base + 1)
    vi.useRealTimers()
    const st = await fetcher.fetch(repoId, { manual: false })
    expect(st.repoId).toBe(repoId) // 거부(reject)가 아니라 상태다(없는 경로라 error 가 채워진다)
    expect(statuses.length).toBeGreaterThan(0)
  })

  it('소유하지 않은 저장소는 fetch 가 오류이고 상태·이벤트가 생기지 않으며, setActive 는 null 로 취급해 예약하지 않는다', async () => {
    vi.useFakeTimers()
    const { fetcher, repoId, statuses } = setup(() => false)
    const base = vi.getTimerCount()
    fetcher.setActive(repoId)
    expect(vi.getTimerCount()).toBe(base)
    vi.useRealTimers()
    await expect(fetcher.fetch(repoId, { manual: true })).rejects.toThrow(/맡지 않/)
    expect(statuses).toEqual([])
    expect(fetcher.status(repoId).fetching).toBe(false)
  })

  it('활성이던 소유 저장소 다음에 소유하지 않은 저장소를 고르면 활성이 풀린다(예약이 사라진다)', () => {
    vi.useFakeTimers()
    const owned = new Set<string>()
    const { fetcher, repoId } = setup((id) => owned.has(id))
    owned.add(repoId)
    const base = vi.getTimerCount()
    fetcher.setActive(repoId)
    expect(vi.getTimerCount()).toBe(base + 1)
    owned.delete(repoId)
    fetcher.setActive(repoId) // 같은 id 라도 이제 소유하지 않는다 — null 로 바뀌며 예약이 사라진다
    expect(vi.getTimerCount()).toBe(base)
  })
})
