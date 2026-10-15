// 제목 표시줄 오버레이 색: 모달 dim 때 배경막(--backdrop)과 같은 비율로 섞인다.
import { describe, expect, it } from 'vitest'
import { titleBarOverlay } from '../src/main/titleBar'

describe('titleBarOverlay', () => {
  it('평소에는 툴바 색 그대로', () => {
    expect(titleBarOverlay(true)).toMatchObject({ color: '#1C1815', symbolColor: '#EDE6D8' })
    expect(titleBarOverlay(false)).toMatchObject({ color: '#EFE8DA', symbolColor: '#2A211B' })
  })

  it('dim 이면 다크는 검정 45%, 라이트는 rgb(42 33 27) 18% 를 섞는다', () => {
    // 0x1C*0.55 = 15.4 → 0f, 0x18*0.55 = 13.2 → 0d, 0x15*0.55 = 11.55 → 0c
    expect(titleBarOverlay(true, true).color).toBe('#0f0d0c')
    // 0xEF*0.82+42*0.18 = 203.54 → cc
    expect(titleBarOverlay(false, true).color.slice(0, 3)).toBe('#cc')
    expect(titleBarOverlay(true, true).height).toBe(titleBarOverlay(true).height)
  })
})
