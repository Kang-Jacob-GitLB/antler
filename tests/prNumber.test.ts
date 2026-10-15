import { describe, expect, it } from 'vitest'
import { parsePrNumber } from '../src/shared/prNumber'

describe('parsePrNumber', () => {
  it('squash 머지 제목 끝의 (#N)', () => {
    expect(parsePrNumber('feat(update): 앱 안 업데이트 — 새 릴리스 알림 스낵바 (#16)')).toBe(16)
    expect(parsePrNumber('feat(sidebar): 서브모듈 섹션 (#15)  ')).toBe(15)
  })
  it('Merge pull request #N from …', () => {
    expect(parsePrNumber('Merge pull request #42 from user/feature-x')).toBe(42)
    expect(parsePrNumber('Merge pull request #7')).toBe(7)
  })
  it('여러 개면 마지막', () => {
    expect(parsePrNumber('fix: 되돌리기 (#12) (#34)')).toBe(34)
    expect(parsePrNumber('Revert "feat: x (#5)" (#9)')).toBe(9)
  })
  it('괄호 없는 언급·문장 중간은 무시', () => {
    expect(parsePrNumber('fix: #123 처리')).toBeNull()
    expect(parsePrNumber('see issue #5')).toBeNull()
    expect(parsePrNumber('fix: 이슈 #5 해결')).toBeNull()
    expect(parsePrNumber('fix (#12) 후속 정리')).toBeNull()
    expect(parsePrNumber('Merge pull request #12abc from x')).toBeNull()
    expect(parsePrNumber('Please Merge pull request #3 from x')).toBeNull()
  })
  it('잘못된 값', () => {
    expect(parsePrNumber('x (#0)')).toBeNull()
    expect(parsePrNumber('x (#)')).toBeNull()
    expect(parsePrNumber('x (#1234567890123)')).toBeNull()
    expect(parsePrNumber('')).toBeNull()
  })
})
