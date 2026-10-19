import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ConfigStore } from '../src/main/services/ConfigStore'
import { MAX_PROFILES, applySettings, snapshotSettings, validateSettings } from '../src/main/services/profiles'
import type { UiPrefs } from '../src/shared/types'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-prof-')), 'antler.config.json')
const store = (): ConfigStore => new ConfigStore(tmp())
const ids = (c: ConfigStore): string[] => (c.get().profiles ?? []).map((p) => p.id)

describe('프로필 설정 검증', () => {
  it('알 수 없는 키와 토큰·앱 전체 설정 키는 거부한다', () => {
    for (const key of ['githubClientId', 'jiraSites', 'editorExePath', 'lastRepoId', 'token', 'activeProfileId', 'groupColors', '__proto__x']) {
      expect(() => validateSettings({ [key]: 'x' }), key).toThrow()
    }
  })

  it('값의 형식·범위를 확인한다', () => {
    expect(() => validateSettings({ theme: 'neon' })).toThrow()
    expect(() => validateSettings({ locale: 'fr' })).toThrow()
    expect(() => validateSettings({ paneWidths: [1, 2] })).toThrow()
    expect(() => validateSettings({ paneWidths: [300] })).toThrow()
    expect(() => validateSettings({ paneWidths: [300, Number.NaN] })).toThrow()
    expect(() => validateSettings({ autoFetchMinutes: 1.5 })).toThrow()
    expect(() => validateSettings({ autoFetchMinutes: 5000 })).toThrow()
    expect(() => validateSettings({ editor: 'notepad' })).toThrow()
    expect(() => validateSettings({ sidebarHidden: 'yes' })).toThrow()
    expect(() => validateSettings({ visibleGroups: 'a' })).toThrow()
    expect(() => validateSettings({ visibleGroups: ['x'.repeat(201)] })).toThrow()
    expect(() => validateSettings({ visibleGroups: Array.from({ length: 51 }, (_, i) => `g${i}`) })).toThrow()
    expect(() => validateSettings([])).toThrow()
    expect(() => validateSettings(null)).toThrow()
  })

  it('올바른 설정은 복사해서 돌려주고 undefined 값은 버린다', () => {
    const input = { theme: 'dark', paneWidths: [300.4, 400], visibleGroups: ['a', 'a', 'b'], diffMode: undefined }
    const out = validateSettings(input)
    expect(out).toEqual({ theme: 'dark', paneWidths: [300, 400], visibleGroups: ['a', 'b'] })
    expect(out).not.toBe(input)
  })
})

describe('snapshot / apply', () => {
  const ui: UiPrefs = {
    theme: 'light',
    locale: 'en',
    paneWidths: [300, 500],
    diffMode: 'split',
    sidebarHidden: true,
    githubClientId: 'abc',
    lastRepoId: 'r1',
    editorExePath: '/opt/ide',
    jiraProjectKeys: ['AB']
  }

  it('스냅샷에는 프로필 키만 담긴다', () => {
    const snap = snapshotSettings(ui)
    expect(snap).toEqual({ theme: 'light', locale: 'en', paneWidths: [300, 500], diffMode: 'split', sidebarHidden: true })
    expect(snap.paneWidths).not.toBe(ui.paneWidths)
  })

  it('적용하면 프로필에 없는 선택 키는 지우고 앱 전체 설정은 그대로 둔다', () => {
    const next = applySettings(ui, { theme: 'dark', locale: 'ko', paneWidths: [250, 380] })
    expect(next.theme).toBe('dark')
    expect(next.diffMode).toBeUndefined()
    expect(next.sidebarHidden).toBeUndefined()
    expect(next.githubClientId).toBe('abc')
    expect(next.editorExePath).toBe('/opt/ide')
    expect(next.lastRepoId).toBe('r1')
    expect(next.jiraProjectKeys).toEqual(['AB'])
  })

  it('필수 키(테마·언어·패널 폭)가 프로필에 없으면 현재 값을 유지한다', () => {
    const next = applySettings(ui, { diffMode: 'unified' })
    expect(next).toMatchObject({ theme: 'light', locale: 'en', paneWidths: [300, 500], diffMode: 'unified' })
  })
})

describe('ConfigStore 프로필', () => {
  it('현재 설정으로 만들고 저장 파일에 남으며 다시 열어도 유지된다', () => {
    const file = tmp()
    const c = new ConfigStore(file)
    c.setUi({ theme: 'dark', diffMode: 'split', paneWidths: [300, 420] })
    const after = c.saveProfile({ name: '  집  ' })
    const p = after.profiles![0]!
    expect(p.name).toBe('집')
    expect(p.settings).toMatchObject({ theme: 'dark', diffMode: 'split', paneWidths: [300, 420] })
    expect(after.ui.activeProfileId).toBe(p.id)
    expect(JSON.parse(readFileSync(file, 'utf8')).profiles).toHaveLength(1)
    expect(new ConfigStore(file).get().ui.activeProfileId).toBe(p.id)
  })

  it('토큰·앱 전체 설정은 프로필에 담기지 않는다', () => {
    const c = store()
    c.setUi({ githubClientId: 'client-1', jiraProjectKeys: ['AB'] })
    c.setEditorExePath('/opt/ide')
    const p = c.saveProfile({ name: 'a' }).profiles![0]!
    expect(Object.keys(p.settings).sort()).toEqual(['locale', 'paneWidths', 'theme'])
    expect(JSON.stringify(p)).not.toMatch(/client-1|ide|AB/)
  })

  it('전환하면 설정이 바뀌고, 떠나는 프로필에는 그때까지의 변경이 저장된다', () => {
    const c = store()
    c.setUi({ theme: 'dark', paneWidths: [260, 400] })
    const a = c.saveProfile({ name: 'A' }).profiles![0]!
    c.setUi({ theme: 'light', paneWidths: [320, 500], sidebarHidden: true })
    const b = c.saveProfile({ name: 'B' }).profiles![1]!
    expect(c.get().ui.activeProfileId).toBe(b.id)

    let cfg = c.applyProfile(a.id)
    expect(cfg.ui).toMatchObject({ theme: 'dark', paneWidths: [260, 400], activeProfileId: a.id })
    expect(cfg.ui.sidebarHidden).toBeUndefined()

    // A 에서 바꾼 것은 A 로 돌아올 때 유지된다
    c.setUi({ diffMode: 'split' })
    cfg = c.applyProfile(b.id)
    expect(cfg.ui).toMatchObject({ theme: 'light', paneWidths: [320, 500], sidebarHidden: true })
    expect(cfg.ui.diffMode).toBeUndefined()
    cfg = c.applyProfile(a.id)
    expect(cfg.ui.diffMode).toBe('split')
  })

  it('전환해도 토큰 쪽 설정(client ID·Jira)과 에디터 경로는 그대로다', () => {
    const c = store()
    c.setUi({ githubClientId: 'cid', lastRepoId: 'r1' })
    c.setEditorExePath('/opt/ide')
    const a = c.saveProfile({ name: 'A' }).profiles![0]!
    c.setUi({ theme: 'dark' })
    const b = c.saveProfile({ name: 'B' }).profiles![1]!
    c.applyProfile(a.id)
    const cfg = c.applyProfile(b.id)
    expect(cfg.ui).toMatchObject({ githubClientId: 'cid', lastRepoId: 'r1', editorExePath: '/opt/ide' })
  })

  it('복제는 설정을 복사하고 활성 프로필 복제는 현재 변경도 담는다', () => {
    const c = store()
    const a = c.saveProfile({ name: 'A' }).profiles![0]!
    c.setUi({ diffMode: 'split' })
    const cfg = c.duplicateProfile(a.id, 'A 복사')
    const copy = cfg.profiles![1]!
    expect(copy.id).not.toBe(a.id)
    expect(copy.settings.diffMode).toBe('split')
    expect(cfg.ui.activeProfileId).toBe(a.id)
    expect(() => c.duplicateProfile(a.id, 'a 복사')).toThrow(/같은 이름/)
  })

  it('삭제하면 목록에서 빠지고 활성 표시도 지운다', () => {
    const c = store()
    const a = c.saveProfile({ name: 'A' }).profiles![0]!
    const cfg = c.deleteProfile(a.id)
    expect(cfg.profiles).toEqual([])
    expect(cfg.ui.activeProfileId).toBeUndefined()
    expect(() => c.deleteProfile(a.id)).toThrow()
    expect(() => c.applyProfile(a.id)).toThrow()
  })

  it('이름 변경과 덮어쓰기', () => {
    const c = store()
    const a = c.saveProfile({ name: 'A' }).profiles![0]!
    c.saveProfile({ name: 'B' })
    expect(() => c.saveProfile({ id: a.id, name: 'b' })).toThrow(/같은 이름/)
    expect(c.saveProfile({ id: a.id, name: 'A2' }).profiles![0]!.name).toBe('A2')
    c.setUi({ theme: 'dark' })
    expect(c.saveProfile({ id: a.id }).profiles![0]!.settings.theme).toBe('dark')
    expect(c.saveProfile({ id: a.id, settings: { theme: 'light' } }).profiles![0]!.settings).toEqual({ theme: 'light' })
  })

  it('개수·이름·입력 상한을 지킨다', () => {
    const c = store()
    for (let i = 0; i < MAX_PROFILES; i++) c.saveProfile({ name: `p${i}` })
    expect(() => c.saveProfile({ name: 'one more' })).toThrow(/최대/)
    expect(() => c.duplicateProfile(ids(c)[0]!, 'x')).toThrow(/최대/)
    const d = store()
    expect(() => d.saveProfile({ name: '' })).toThrow()
    expect(() => d.saveProfile({ name: '   ' })).toThrow()
    expect(() => d.saveProfile({ name: 'x'.repeat(51) })).toThrow()
    expect(() => d.saveProfile({ name: 'a\nb' })).toThrow()
    expect(() => d.saveProfile({ name: 'ok', extra: 1 } as never)).toThrow()
    expect(() => d.saveProfile({ name: 'ok', settings: { githubClientId: 'x' } as never })).toThrow()
    expect(() => d.saveProfile(null as never)).toThrow()
  })

  it('renderer 는 활성 프로필 표시·에디터 실행 파일 경로를 setUi 로 바꿀 수 없다', () => {
    const c = store()
    expect(() => c.setUi({ editorExePath: '/tmp/evil' } as never)).toThrow()
    expect(() => c.setUi({ activeProfileId: 'x' } as never)).toThrow()
    expect(() => c.setUi({ editor: 'emacs' } as never)).toThrow()
    expect(() => c.setUi({ visibleGroups: [1] } as never)).toThrow()
    expect(c.setUi({ editor: 'cursor', visibleGroups: ['a'] }).ui).toMatchObject({ editor: 'cursor', visibleGroups: ['a'] })
  })

  it('손상된 프로필 항목은 읽을 때 버리고 없는 프로필을 가리키는 활성 표시도 지운다', () => {
    const file = tmp()
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        sources: [],
        repos: [],
        ui: { theme: 'dark', locale: 'ko', paneWidths: [248, 380], activeProfileId: 'gone' },
        profiles: [
          { id: 'ok', name: '정상', settings: { theme: 'light' } },
          { id: 'bad1', name: '나쁨', settings: { githubToken: 'secret' } },
          { id: 'ok', name: '중복 id', settings: {} },
          { id: 'bad2', name: '', settings: {} },
          'junk'
        ]
      })
    )
    const cfg = new ConfigStore(file).get()
    expect(cfg.profiles?.map((p) => p.id)).toEqual(['ok'])
    expect(cfg.ui.activeProfileId).toBeUndefined()
  })

  it('프로필이 없던 예전 설정 파일도 그대로 읽는다', () => {
    const file = tmp()
    writeFileSync(file, JSON.stringify({ version: 1, sources: [], repos: [], ui: { theme: 'dark', locale: 'ko', paneWidths: [248, 380] } }))
    const cfg = new ConfigStore(file).get()
    expect(cfg.profiles).toBeUndefined()
    expect(cfg.ui.theme).toBe('dark')
  })
})
