import { describe, expect, it } from 'vitest'
import { GROUP_HUES, groupHue, pickHue, syncGroupColors } from '../src/renderer/src/lib/groupColor'

describe('group colors', () => {
  it('새 그룹은 아직 안 쓰인 색을 받는다', () => {
    const used = GROUP_HUES.slice(0, 11)
    expect(pickHue('아무거나', used)).toBe(GROUP_HUES[11])
  })

  it('팔레트가 다 쓰이면 팔레트 안에서 고른다', () => {
    expect(GROUP_HUES).toContain(pickHue('x', GROUP_HUES))
  })

  it('팔레트 크기만큼의 그룹은 모두 다른 색', () => {
    const groups = Array.from({ length: GROUP_HUES.length }, (_, i) => `g${i}`)
    const colors = syncGroupColors(groups, undefined)!
    expect(new Set(Object.values(colors)).size).toBe(GROUP_HUES.length)
  })

  it('한 번 정해진 색은 유지하고, 없어진 그룹은 지우며, 바뀐 게 없으면 null', () => {
    const a = syncGroupColors(['회사', '개인'], undefined)!
    const b = syncGroupColors(['회사', '개인', '오픈소스'], a)!
    expect(b['회사']).toBe(a['회사'])
    expect(b['개인']).toBe(a['개인'])
    expect(new Set(Object.values(b)).size).toBe(3)
    expect(syncGroupColors(['회사', '개인', '오픈소스'], b)).toBeNull()
    const c = syncGroupColors(['회사'], b)!
    expect(Object.keys(c)).toEqual(['회사'])
  })

  it('미리보기 색은 저장될 색과 같다', () => {
    const colors = syncGroupColors(['회사', '개인'], undefined)!
    const preview = groupHue('새그룹', colors)
    expect(syncGroupColors(['회사', '개인', '새그룹'], colors)!['새그룹']).toBe(preview)
  })
})
