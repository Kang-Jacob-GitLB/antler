// 야간 자동 릴리스의 버전 계산(scripts/next-version.mjs)
import { describe, expect, it } from 'vitest'
// @ts-expect-error — 순수 JS 스크립트(타입 선언 없음)
import { bumpVersion, classify, decideBump, releaseNotes } from '../scripts/next-version.mjs'

const c = (subject: string, body = '') => ({ hash: 'x', subject, body })

describe('next-version', () => {
  it('커밋 종류별 올림: feat=minor, fix·perf·style=patch, 그 밖은 없음', () => {
    expect(classify('feat(graph): 미니맵').bump).toBe('minor')
    expect(classify('fix: 버그 (#21)').bump).toBe('patch')
    expect(classify('perf: 콜드 스타트').bump).toBe('patch')
    expect(classify('style: 상태바').bump).toBe('patch')
    for (const s of ['ci: 워크플로', 'docs: README', 'chore: v0.2.0', 'test: 격리', 'refactor: 정리', '그냥 메시지']) {
      expect(classify(s).bump).toBeNull()
    }
  })

  it('! 나 BREAKING CHANGE 는 major', () => {
    expect(classify('feat!: 설정 형식 변경').bump).toBe('major')
    expect(classify('fix(api)!: 응답 변경').bump).toBe('major')
    expect(classify('refactor: 내부', 'BREAKING CHANGE: 옛 설정 파일 미지원').bump).toBe('major')
  })

  it('여러 커밋 중 가장 큰 올림을 고른다', () => {
    expect(decideBump([c('ci: a'), c('docs: b')])).toBeNull()
    expect(decideBump([c('fix: a'), c('style: b')])).toBe('patch')
    expect(decideBump([c('fix: a'), c('feat: b'), c('ci: c')])).toBe('minor')
    expect(decideBump([c('feat: a'), c('feat!: b')])).toBe('major')
  })

  it('버전 올림', () => {
    expect(bumpVersion('v1.0.0', 'patch')).toBe('1.0.1')
    expect(bumpVersion('v1.0.7', 'minor')).toBe('1.1.0')
    expect(bumpVersion('v1.4.2', 'major')).toBe('2.0.0')
    expect(() => bumpVersion('v1.0.0-rc.1', 'patch')).toThrow()
  })

  it('릴리스 노트는 종류별로 묶고 내부 변경은 따로 둔다', () => {
    const notes = releaseNotes('1.1.0', [c('feat(sidebar): 서브모듈 섹션 (#15)'), c('fix: 아바타 (#21)'), c('ci: 야간 릴리스 (#25)')])
    expect(notes.split('\n')[0]).toBe('v1.1.0')
    expect(notes).toContain('## 새 기능\n\n- **sidebar**: 서브모듈 섹션 (#15)')
    expect(notes).toContain('## 버그 수정\n\n- 아바타 (#21)')
    expect(notes).toContain('## 그 밖의 변경(내부)\n\n- 야간 릴리스 (#25)')
  })
})
