import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { GhLaunchpadItem, JiraIssueSummary, Repo } from '../src/shared/types'
import { MAX_LAUNCHPAD_PINS } from '../src/shared/launchpad'
import { ConfigStore } from '../src/main/services/ConfigStore'
import {
  emptyFilter,
  jiraEntries,
  pruneSnoozes,
  scopeKey,
  scopeRepos,
  setSnooze,
  snoozeUntil,
  toEntries,
  togglePin,
  visibleEntries,
  type LpEntry
} from '../src/renderer/src/lib/launchpad'

const user = { login: 'me', avatarUrl: '' }
const item = (over: Partial<GhLaunchpadItem> = {}): GhLaunchpadItem => ({
  section: 'authored',
  kind: 'pull',
  repoIds: ['r1'],
  owner: 'Acme',
  repo: 'App',
  number: 1,
  title: 'Fix login',
  state: 'open',
  draft: false,
  author: user,
  labels: [],
  comments: 0,
  updatedAt: 1000,
  createdAt: 900,
  htmlUrl: '',
  ...over
})
const repos: Repo[] = [
  { id: 'r1', sourceId: 'local', path: '/a', name: 'app', group: 'Work' },
  { id: 'r2', sourceId: 'local', path: '/b', name: 'lib', group: 'Work' },
  { id: 'r3', sourceId: 'local', path: '/c', name: 'blog' }
]
const NOW = 1_000_000

describe('toEntries / jiraEntries', () => {
  it('저장소 이름·그룹을 앱의 저장소에서 붙이고 키는 소문자로 정규화한다', () => {
    const [e] = toEntries([item()], repos)
    expect(e).toMatchObject({ key: 'acme/app#1', repoName: 'app', group: 'Work', slug: 'Acme/App', section: 'authored' })
    expect(toEntries([item({ repoIds: ['r3'] })], repos)[0]!.group).toBeUndefined()
  })

  it('Jira 이슈를 epoch seconds 와 jira: 키로 바꾼다', () => {
    const j: JiraIssueSummary = { key: 'ABC-7', summary: 'Do it', status: 'In Progress', statusCategory: 'indeterminate', issueType: 'Task', priority: 'High', assignee: 'Me', updated: '2026-01-02T03:04:05.000+0000', dueDate: '' }
    const [e] = jiraEntries([j, { ...j, key: 'ABC-8', updated: 'bad' }])
    expect(e).toMatchObject({ key: 'jira:ABC-7', section: 'jira', kind: 'jira', jiraKey: 'ABC-7', updatedAt: Date.parse('2026-01-02T03:04:05.000Z') / 1000 })
    expect(jiraEntries([{ ...j, updated: 'bad' }])[0]!.updatedAt).toBe(0)
  })
})

describe('toEntries 범위 거르기', () => {
  const items = [item({ number: 1, repoIds: ['r1'] }), item({ number: 2, repoIds: ['r3'] }), item({ number: 3, repoIds: ['r9', 'r2'] }), item({ number: 4, repoIds: ['r9'] })]
  const nums = (scope: Parameters<typeof toEntries>[2]): (number | undefined)[] => toEntries(items, repos, scope).map((e) => e.number)

  it('전체는 등록된 저장소에 속한 항목만(제거된 저장소 r9 만 가진 항목은 빠진다)', () => {
    expect(nums({ kind: 'all' })).toEqual([1, 2, 3])
  })
  it('그룹·저장소 하나는 같은 결과에서 거르기만 한다', () => {
    expect(nums({ kind: 'group', name: 'Work' })).toEqual([1, 3])
    expect(nums({ kind: 'repo', id: 'r3' })).toEqual([2])
  })
  it('같은 GitHub 저장소를 여러 번 등록했으면 범위 안의 저장소 이름·그룹을 쓴다', () => {
    const both = [item({ repoIds: ['r3', 'r1'] })]
    expect(toEntries(both, repos, { kind: 'all' })[0]).toMatchObject({ repoId: 'r3', repoName: 'blog' })
    expect(toEntries(both, repos, { kind: 'group', name: 'Work' })[0]).toMatchObject({ repoId: 'r1', group: 'Work' })
  })
})

describe('visibleEntries', () => {
  const entries: LpEntry[] = toEntries(
    [
      item({ number: 1, title: 'Old', updatedAt: 100 }),
      item({ number: 2, title: 'New', updatedAt: 300 }),
      item({ number: 3, title: 'Draft one', draft: true, updatedAt: 200 }),
      item({ number: 4, title: 'Other repo', repoIds: ['r2'], repo: 'Lib', updatedAt: 250, author: { login: 'zed', avatarUrl: '' } }),
      item({ number: 5, title: 'Needs review', section: 'review', updatedAt: 50 })
    ],
    repos
  )
  const keys = (v: { items: LpEntry[] }): number[] => v.items.map((e) => e.number!)
  const view = (f = emptyFilter, pins: string[] = [], snoozes: Record<string, number> = {}) => visibleEntries(entries, 'authored', f, pins, snoozes, NOW)

  it('섹션별로 나누고 최근 업데이트 순으로 정렬한다', () => {
    expect(keys(view())).toEqual([2, 4, 3, 1])
    expect(keys(visibleEntries(entries, 'review', emptyFilter, [], {}, NOW))).toEqual([5])
  })

  it('핀은 맨 위로(핀끼리도 최근순)', () => {
    expect(keys(view(emptyFilter, ['acme/app#1', 'acme/app#3']))).toEqual([3, 1, 2, 4])
  })

  it('스누즈 중인 항목은 숨기고 개수를 센다. 기간이 끝났으면 다시 보인다', () => {
    const v = view(emptyFilter, [], { 'acme/app#2': NOW + 1, 'acme/app#1': NOW })
    expect(keys(v)).toEqual([4, 3, 1])
    expect(v.snoozed).toBe(1)
    expect(keys(view({ ...emptyFilter, showSnoozed: true }, [], { 'acme/app#2': NOW + 1 }))).toEqual([2, 4, 3, 1])
  })

  it('초안 숨기기·저장소·텍스트 필터', () => {
    expect(keys(view({ ...emptyFilter, hideDrafts: true }))).toEqual([2, 4, 1])
    expect(keys(view({ ...emptyFilter, repoId: 'r2' }))).toEqual([4])
    expect(keys(view({ ...emptyFilter, text: ' ZED ' }))).toEqual([4])
    expect(keys(view({ ...emptyFilter, text: '#3' }))).toEqual([3])
    expect(keys(view({ ...emptyFilter, text: 'draft' }))).toEqual([3])
  })

  it('저장소 필터·초안 숨기기는 Jira 항목에 적용되지 않는다', () => {
    const all = [...entries, ...jiraEntries([{ key: 'ABC-1', summary: 'Jira thing', status: 'To Do', statusCategory: 'new', issueType: '', priority: '', assignee: '', updated: '2026-01-01T00:00:00Z', dueDate: '' }])]
    const v = visibleEntries(all, 'jira', { ...emptyFilter, repoId: 'r2', hideDrafts: true }, [], {}, NOW)
    expect(v.items.map((e) => e.key)).toEqual(['jira:ABC-1'])
    expect(visibleEntries(all, 'jira', { ...emptyFilter, text: 'abc-1' }, [], {}, NOW).items).toHaveLength(1)
  })
})

describe('핀·스누즈 상태', () => {
  it('핀 토글과 상한', () => {
    expect(togglePin([], 'a/b#1')).toEqual(['a/b#1'])
    expect(togglePin(['a/b#1', 'a/b#2'], 'a/b#1')).toEqual(['a/b#2'])
    const full = Array.from({ length: MAX_LAUNCHPAD_PINS }, (_, i) => `a/b#${i + 1}`)
    const next = togglePin(full, 'a/b#999')
    expect(next).toHaveLength(MAX_LAUNCHPAD_PINS)
    expect(next[next.length - 1]).toBe('a/b#999')
    expect(next).not.toContain('a/b#1')
  })

  it('스누즈 지정·해제·만료 정리', () => {
    const a = setSnooze({}, 'a/b#1', NOW + 10, NOW)
    expect(a).toEqual({ 'a/b#1': NOW + 10 })
    // 만료된 항목은 새 쓰기 때 정리된다
    expect(setSnooze({ 'a/b#9': NOW - 1, ...a }, 'a/b#2', NOW + 20, NOW)).toEqual({ 'a/b#1': NOW + 10, 'a/b#2': NOW + 20 })
    expect(setSnooze(a, 'a/b#1', null, NOW)).toEqual({})
    // 과거 시각은 지정되지 않는다
    expect(setSnooze({}, 'a/b#1', NOW - 5, NOW)).toEqual({})
    const same = { 'a/b#1': NOW + 1 }
    expect(pruneSnoozes(same, NOW)).toBe(same)
  })

  it('스누즈 프리셋 시각', () => {
    expect(snoozeUntil('1h', NOW)).toBe(NOW + 3_600_000)
    expect(snoozeUntil('4h', NOW)).toBe(NOW + 4 * 3_600_000)
    expect(snoozeUntil('1w', NOW)).toBe(NOW + 7 * 24 * 3_600_000)
    const tomorrow = new Date(snoozeUntil('tomorrow', Date.parse('2026-03-10T15:00:00')))
    expect([tomorrow.getDate(), tomorrow.getHours(), tomorrow.getMinutes()]).toEqual([11, 9, 0])
  })
})

describe('범위', () => {
  it('전체 · 그룹 · 저장소 하나', () => {
    expect(scopeRepos({ kind: 'all' }, repos)).toHaveLength(3)
    expect(scopeRepos({ kind: 'group', name: 'Work' }, repos).map((r) => r.id)).toEqual(['r1', 'r2'])
    expect(scopeRepos({ kind: 'repo', id: 'r3' }, repos).map((r) => r.id)).toEqual(['r3'])
    expect(scopeKey({ kind: 'group', name: 'Work' })).toBe('group:Work')
  })
})

describe('설정 저장 검증(ConfigStore.setUi)', () => {
  let dir: string
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-lp-'))
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  it('올바른 핀·스누즈는 저장하고 모양이 틀린 것은 거부한다', () => {
    const store = new ConfigStore(join(dir, 'antler.config.json'))
    const ok = store.setUi({ launchpadPins: ['acme/app#1', 'jira:ABC-7'], launchpadSnoozes: { 'acme/app#2': NOW } })
    expect(ok.ui.launchpadPins).toEqual(['acme/app#1', 'jira:ABC-7'])
    expect(new ConfigStore(join(dir, 'antler.config.json')).get().ui.launchpadSnoozes).toEqual({ 'acme/app#2': NOW })

    expect(() => store.setUi({ launchpadPins: ['Acme/App#1'] })).toThrow('Launchpad')
    expect(() => store.setUi({ launchpadPins: ['x'.repeat(300)] })).toThrow()
    expect(() => store.setUi({ launchpadPins: Array.from({ length: MAX_LAUNCHPAD_PINS + 1 }, (_, i) => `a/b#${i + 1}`) })).toThrow()
    expect(() => store.setUi({ launchpadSnoozes: { 'a/b#1': -1 } })).toThrow()
    expect(() => store.setUi({ launchpadSnoozes: { 'a/b#1': 1.5 } })).toThrow()
    expect(() => store.setUi({ launchpadSnoozes: { 'not a key': 5 } })).toThrow()
    expect(() => store.setUi({ launchpadSnoozes: [] as unknown as Record<string, number> })).toThrow()
  })
})
