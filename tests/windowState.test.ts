import { describe, expect, it } from 'vitest'
import { restoreBounds, sanitizeWindowState } from '../src/main/windowState'

const main = { x: 0, y: 0, width: 1920, height: 1040 }
const right = { x: 1920, y: 0, width: 2560, height: 1400 }

describe('sanitizeWindowState', () => {
  it('정수 좌표·양수 크기만 받고 maximized 는 true 일 때만 남긴다', () => {
    expect(sanitizeWindowState({ x: -8, y: 10, width: 1200, height: 800, maximized: false })).toEqual({ x: -8, y: 10, width: 1200, height: 800 })
    expect(sanitizeWindowState({ x: 0, y: 0, width: 1200, height: 800, maximized: true })?.maximized).toBe(true)
    expect(sanitizeWindowState({ x: 0, y: 0, width: 0, height: 800 })).toBeUndefined()
    expect(sanitizeWindowState({ x: 0.5, y: 0, width: 100, height: 100 })).toBeUndefined()
    expect(sanitizeWindowState({ x: '1', y: 0, width: 100, height: 100 })).toBeUndefined()
    expect(sanitizeWindowState(null)).toBeUndefined()
  })
})

describe('restoreBounds', () => {
  it('저장한 값이 없으면 기본(undefined)', () => {
    expect(restoreBounds(undefined, [main])).toBeUndefined()
  })

  it('화면 안에 있으면 그대로 연다(두 번째 모니터 포함)', () => {
    expect(restoreBounds({ x: 100, y: 50, width: 1200, height: 800 }, [main, right])).toEqual({ x: 100, y: 50, width: 1200, height: 800 })
    expect(restoreBounds({ x: 2000, y: 100, width: 1600, height: 900 }, [main, right])).toEqual({ x: 2000, y: 100, width: 1600, height: 900 })
  })

  it('화면 밖으로 걸치면 안으로 밀고, 화면보다 크면 줄인다', () => {
    expect(restoreBounds({ x: 1500, y: 600, width: 1200, height: 800 }, [main])).toEqual({ x: 720, y: 240, width: 1200, height: 800 })
    expect(restoreBounds({ x: 0, y: 0, width: 3000, height: 2000 }, [main])).toEqual({ x: 0, y: 0, width: 1920, height: 1040 })
  })

  it('모니터를 빼서 어느 화면에도 걸치지 않으면 첫 화면 가운데', () => {
    expect(restoreBounds({ x: 2200, y: 100, width: 1200, height: 800 }, [main])).toEqual({ x: 360, y: 120, width: 1200, height: 800 })
  })
})
