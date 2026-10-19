import { describe, expect, it } from 'vitest'
import { filterCommands, type CommandLike } from '../src/renderer/src/lib/commandFilter'
import { fuzzyMatch, rankByQuery } from '../src/renderer/src/lib/fuzzy'
import { keyLabel, matchesShortcut, platformKey, shortcutParts, shortcutText } from '../src/renderer/src/lib/shortcuts'

describe('fuzzyMatch', () => {
  it('빈 질의는 항상 일치한다', () => {
    expect(fuzzyMatch('', '아무거나')).toEqual({ score: 0, indices: [] })
  })

  it('대소문자를 구분하지 않고 부분열로 일치한다', () => {
    expect(fuzzyMatch('rnc', 'Release Notes Cache')?.indices).toEqual([0, 8, 14])
    expect(fuzzyMatch('RNC', 'release notes cache')).not.toBeNull()
  })

  it('순서가 다르거나 없는 글자는 일치하지 않는다', () => {
    expect(fuzzyMatch('ba', 'abc')).toBeNull()
    expect(fuzzyMatch('x', 'abc')).toBeNull()
    expect(fuzzyMatch('abcd', 'abc')).toBeNull()
  })

  it('부분 문자열 일치가 흩어진 부분열 일치보다 점수가 높다', () => {
    const sub = fuzzyMatch('fetch', 'Auto fetch')!
    const seq = fuzzyMatch('fetch', 'file edit tool chain home')!
    expect(sub.score).toBeGreaterThan(seq.score)
  })

  it('앞쪽·단어 시작 일치가 더 높다', () => {
    expect(fuzzyMatch('theme', 'theme dark')!.score).toBeGreaterThan(fuzzyMatch('theme', 'dark theme')!.score)
    expect(fuzzyMatch('dark', 'dark theme')!.score).toBeGreaterThan(fuzzyMatch('ark', 'dark theme')!.score)
  })

  it('한글을 음절 단위로 맞추고 일치 위치를 돌려준다', () => {
    expect(fuzzyMatch('테다', '테마: 다크')?.indices).toEqual([0, 4])
    expect(fuzzyMatch('다크', '테마: 다크')?.indices).toEqual([4, 5])
  })
})

describe('rankByQuery', () => {
  const items = [
    { title: '테마: 시스템', keywords: 'theme system' },
    { title: '테마: 다크', keywords: 'theme dark' },
    { title: '테마: 라이트', keywords: 'theme light' },
    { title: '새로고침', keywords: 'fetch refresh' },
    { title: '언어: English', keywords: 'language' }
  ]
  const f = (x: (typeof items)[number]): { title: string; keywords: string } => x

  it('빈 질의는 입력 순서를 그대로 둔다', () => {
    expect(rankByQuery(items, '  ', f).map((r) => r.item.title)).toEqual(items.map((i) => i.title))
  })

  it('제목 또는 키워드에 일치하는 항목만 남긴다', () => {
    expect(rankByQuery(items, 'dark', f).map((r) => r.item.title)).toEqual(['테마: 다크'])
    expect(rankByQuery(items, 'fetch', f).map((r) => r.item.title)).toEqual(['새로고침'])
  })

  it('공백으로 나눈 낱말이 모두 일치해야 한다', () => {
    expect(rankByQuery(items, '테마 라이', f).map((r) => r.item.title)).toEqual(['테마: 라이트'])
    expect(rankByQuery(items, '테마 fetch', f)).toEqual([])
  })

  it('점수가 같으면 입력 순서를 유지한다', () => {
    const same = ['테마: 가', '테마: 나', '테마: 다'].map((title) => ({ title, keywords: '' }))
    expect(rankByQuery(same, '테마', f).map((r) => r.item.title)).toEqual(['테마: 가', '테마: 나', '테마: 다'])
  })

  it('같은 질의면 더 짧은 제목이 앞선다', () => {
    expect(rankByQuery(items, '테마', f)[0]!.item.title).toBe('테마: 다크')
  })

  it('제목 일치가 키워드로만 일치한 것보다 앞선다', () => {
    const list = [
      { title: 'Update check', keywords: 'release notes' },
      { title: 'Release notes', keywords: '' }
    ]
    expect(rankByQuery(list, 'release', f2).map((r) => r.item.title)).toEqual(['Release notes', 'Update check'])
    function f2(x: (typeof list)[number]): { title: string; keywords: string } {
      return x
    }
  })

  it('제목에서 일치한 위치만 indices 로 돌려준다', () => {
    const [r] = rankByQuery(items, 'dark', f)
    expect(r!.indices).toEqual([])
    const [r2] = rankByQuery(items, '다크', f)
    expect(r2!.indices).toEqual([4, 5])
  })
})

describe('filterCommands', () => {
  const cmd = (id: string, label: string, extra: Partial<CommandLike> = {}): CommandLike => ({
    id,
    category: 'general',
    label,
    available: true,
    ...extra
  })
  const list: CommandLike[] = [
    cmd('a', 'Search commits'),
    cmd('b', 'Refresh', { available: false }),
    cmd('c', 'Command palette', { paletteHidden: true, shortcut: { code: 'KeyP', shift: true } }),
    cmd('d', 'Switch repository')
  ]

  it('상황에 맞지 않거나 팔레트에서 숨긴 명령은 뺀다', () => {
    expect(filterCommands(list, '').map((r) => r.item.id)).toEqual(['a', 'd'])
    expect(filterCommands(list, 'refresh')).toEqual([])
    expect(filterCommands(list, 'palette')).toEqual([])
  })

  it('질의로 거르고 점수순으로 돌려준다', () => {
    expect(filterCommands(list, 'repo').map((r) => r.item.id)).toEqual(['d'])
    expect(filterCommands(list, 'sea').map((r) => r.item.id)).toEqual(['a'])
  })
})

describe('shortcuts', () => {
  const ev = (o: Partial<Record<'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey', boolean>> & { code: string; key?: string }) => ({
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    altKey: false,
    ...o
  })

  it('Ctrl 또는 ⌘ 이 필요하고 Shift·Alt 는 정확히 같아야 한다', () => {
    expect(matchesShortcut(ev({ ctrlKey: true, code: 'KeyP' }), { code: 'KeyP' })).toBe(true)
    expect(matchesShortcut(ev({ metaKey: true, code: 'KeyP' }), { code: 'KeyP' })).toBe(true)
    expect(matchesShortcut(ev({ code: 'KeyP' }), { code: 'KeyP' })).toBe(false)
    // Ctrl+Shift+P 는 Ctrl+P(저장소 전환)가 아니다
    expect(matchesShortcut(ev({ ctrlKey: true, shiftKey: true, code: 'KeyP' }), { code: 'KeyP' })).toBe(false)
    expect(matchesShortcut(ev({ ctrlKey: true, shiftKey: true, code: 'KeyP' }), { code: 'KeyP', shift: true })).toBe(true)
    // Ctrl+B 와 Ctrl+Alt+B 는 서로 다른 조합
    expect(matchesShortcut(ev({ ctrlKey: true, altKey: true, code: 'KeyB' }), { code: 'KeyB' })).toBe(false)
    expect(matchesShortcut(ev({ ctrlKey: true, altKey: true, code: 'KeyB' }), { code: 'KeyB', alt: true })).toBe(true)
  })

  it('자판 배열: e.key 가 ASCII 면 글자로, 아니면 e.code 로 판정한다', () => {
    const P = { code: 'KeyP' }
    // Dvorak: 글자 P 는 물리 KeyR 위치
    expect(matchesShortcut(ev({ ctrlKey: true, code: 'KeyR', key: 'p' }), P)).toBe(true)
    // Dvorak 에서 물리 KeyP 는 글자 L 이므로 Ctrl+P 가 아니다
    expect(matchesShortcut(ev({ ctrlKey: true, code: 'KeyP', key: 'l' }), P)).toBe(false)
    // 한글 IME: key 가 'ㅔ' 이거나 'Process' 여도 물리 키 P 로 동작
    expect(matchesShortcut(ev({ ctrlKey: true, code: 'KeyP', key: 'ㅔ' }), P)).toBe(true)
    expect(matchesShortcut(ev({ ctrlKey: true, code: 'KeyP', key: 'Process' }), P)).toBe(true)
    // Shift 가 눌려 key 가 대문자여도 소문자로 비교
    expect(matchesShortcut(ev({ ctrlKey: true, shiftKey: true, code: 'KeyP', key: 'P' }), { code: 'KeyP', shift: true })).toBe(true)
    // 독일어: Ctrl+/ 는 Shift+7 로 입력된다(key '/')
    expect(matchesShortcut(ev({ ctrlKey: true, shiftKey: true, code: 'Digit7', key: '/' }), { code: 'Slash' })).toBe(true)
    // mac Option+B 는 key 가 '∫' → code 로 판정
    expect(matchesShortcut(ev({ metaKey: true, altKey: true, code: 'KeyB', key: '∫' }), { code: 'KeyB', alt: true })).toBe(true)
  })

  it('플랫폼별로 표기한다', () => {
    expect(shortcutText({ code: 'KeyB', alt: true }, false)).toBe('Ctrl+Alt+B')
    expect(shortcutText({ code: 'KeyB', alt: true }, true)).toBe('⌥⌘B')
    expect(shortcutParts({ code: 'KeyP', shift: true }, false)).toEqual(['Ctrl', 'Shift', 'P'])
    expect(shortcutParts({ code: 'KeyP', shift: true }, true)).toEqual(['⇧', '⌘', 'P'])
    expect(keyLabel('Slash')).toBe('/')
    expect(platformKey('Alt', true)).toBe('⌥')
    expect(platformKey('Alt', false)).toBe('Alt')
    expect(platformKey('PageUp', true)).toBe('PageUp')
  })
})
