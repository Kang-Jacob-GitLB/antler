import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import { ConfigStore, checkEditor, checkVisibleGroups } from '../src/main/services/ConfigStore'

const tmp = (): string => join(mkdtempSync(join(tmpdir(), 'antler-cfg-')), 'antler.config.json')

describe('ConfigStore.setUi 검증', () => {
  it('renderer 는 에디터 실행 파일 경로·건너뛴 업데이트 버전을 setUi 로 바꿀 수 없다', () => {
    const c = new ConfigStore(tmp())
    expect(() => c.setUi({ editorExePath: '/tmp/evil' } as never)).toThrow()
    expect(() => c.setUi({ skippedUpdateVersion: '9.9.9' } as never)).toThrow()
  })

  it('에디터·보일 그룹의 형식을 확인한다', () => {
    const c = new ConfigStore(tmp())
    expect(() => c.setUi({ editor: 'emacs' } as never)).toThrow()
    expect(() => c.setUi({ visibleGroups: [1] } as never)).toThrow()
    expect(c.setUi({ editor: 'cursor', visibleGroups: ['a'] }).ui).toMatchObject({ editor: 'cursor', visibleGroups: ['a'] })
    // 빈 목록 = 제한 없음(키를 지운다)
    expect(c.setUi({ visibleGroups: [] }).ui.visibleGroups).toBeUndefined()
  })

  it('checkVisibleGroups 는 모양·크기를 확인하고 중복을 없앤다', () => {
    expect(() => checkVisibleGroups('a')).toThrow()
    expect(() => checkVisibleGroups(['x'.repeat(201)])).toThrow()
    expect(() => checkVisibleGroups(Array.from({ length: 51 }, (_, i) => `g${i}`))).toThrow()
    expect(checkVisibleGroups(['a', 'a', 'b'])).toEqual(['a', 'b'])
  })

  it('checkEditor 는 알려진 에디터만 받는다', () => {
    expect(() => checkEditor('notepad')).toThrow()
    expect(checkEditor('jetbrains')).toBe('jetbrains')
  })
})

describe('ConfigStore.load', () => {
  it('예전 버전이 남긴 profiles·ui.activeProfileId 는 무시하고 다음 저장 때 파일에서 사라진다', () => {
    const file = tmp()
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        sources: [],
        repos: [],
        ui: { theme: 'dark', locale: 'ko', paneWidths: [248, 380], activeProfileId: 'p1', visibleGroups: ['a'] },
        profiles: [{ id: 'p1', name: '집', settings: { theme: 'light' } }]
      })
    )
    const c = new ConfigStore(file)
    const cfg = c.get() as unknown as { profiles?: unknown; ui: Record<string, unknown> }
    expect(cfg.profiles).toBeUndefined()
    expect(cfg.ui.activeProfileId).toBeUndefined()
    expect(cfg.ui).toMatchObject({ theme: 'dark', visibleGroups: ['a'] })

    c.setUi({ diffMode: 'split' })
    const saved = JSON.parse(readFileSync(file, 'utf8')) as { profiles?: unknown; ui: Record<string, unknown> }
    expect(saved.profiles).toBeUndefined()
    expect(saved.ui.activeProfileId).toBeUndefined()
    expect(saved.ui.diffMode).toBe('split')
  })
})

describe('ConfigStore.onChange', () => {
  it('기본은 아무 일도 하지 않아 설정을 쓰는 동작이 그대로다', () => {
    const file = tmp()
    const c = new ConfigStore(file)
    c.setUi({ diffMode: 'split' })
    expect(JSON.parse(readFileSync(file, 'utf8')).ui.diffMode).toBe('split')
    expect(c.get().ui.diffMode).toBe('split')
  })

  it('쓰기가 끝난 뒤 새 설정으로 불린다(저장소 추가·삭제·UI 설정 모두)', () => {
    const file = tmp()
    const c = new ConfigStore(file)
    const seen: { repos: number; mem: number }[] = []
    c.onChange = (cfg) => {
      // 불린 시점에 디스크와 메모리가 이미 새 상태다
      expect(JSON.parse(readFileSync(file, 'utf8')).repos).toHaveLength(cfg.repos.length)
      seen.push({ repos: cfg.repos.length, mem: c.get().repos.length })
    }
    const r = c.addRepo({ sourceId: 'local', path: '/tmp/x', name: 'x' })
    c.setUi({ diffMode: 'split' })
    c.removeRepo(r.id)
    expect(seen).toEqual([
      { repos: 1, mem: 1 },
      { repos: 1, mem: 1 },
      { repos: 0, mem: 0 }
    ])
  })

  it('알림이 던져도 저장된 변경은 실패하지 않는다', () => {
    const c = new ConfigStore(tmp())
    c.onChange = () => {
      throw new Error('boom')
    }
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    expect(c.setUi({ diffMode: 'split' }).ui.diffMode).toBe('split')
    expect(c.get().ui.diffMode).toBe('split')
    err.mockRestore()
  })
})
