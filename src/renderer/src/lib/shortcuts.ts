// 단축키 정의·판정·표기(순수 함수). 모든 단축키는 Ctrl(맥은 ⌘)을 기본으로 누르고, 거기에 Shift·Alt 가 더해진다.
// 글자 키는 물리 키 위치(KeyboardEvent.code)로 판정해 한글 입력 상태에서도 같은 키로 동작한다.

export interface Shortcut {
  /** KeyboardEvent.code (예: 'KeyP', 'Slash') */
  code: string
  shift?: boolean
  alt?: boolean
}

export interface KeyEventLike {
  ctrlKey: boolean
  metaKey: boolean
  shiftKey: boolean
  altKey: boolean
  code: string
}

/** Ctrl 또는 ⌘ 이 눌렸고, Shift·Alt 상태가 정의와 정확히 같을 때만 일치한다 */
export function matchesShortcut(e: KeyEventLike, sc: Shortcut): boolean {
  return (e.ctrlKey || e.metaKey) && e.code === sc.code && e.shiftKey === (sc.shift === true) && e.altKey === (sc.alt === true)
}

/** 'KeyP' → 'P', 'Slash' → '/', 'Digit1' → '1' */
export function keyLabel(code: string): string {
  if (code.startsWith('Key')) return code.slice(3)
  if (code.startsWith('Digit')) return code.slice(5)
  const named: Record<string, string> = { Slash: '/', Comma: ',', Period: '.', Backslash: '\\', Minus: '-', Equal: '=' }
  return named[code] ?? code
}

/** 키캡 하나씩의 표기. 맥은 ⌥⇧⌘ 순서(예: ⌥⌘B), 그 외는 Ctrl+Alt+Shift 순서 */
export function shortcutParts(sc: Shortcut, mac: boolean): string[] {
  const key = keyLabel(sc.code)
  if (mac) return [...(sc.alt ? ['⌥'] : []), ...(sc.shift ? ['⇧'] : []), '⌘', key]
  return ['Ctrl', ...(sc.alt ? ['Alt'] : []), ...(sc.shift ? ['Shift'] : []), key]
}

/** 툴팁용 한 줄 표기: 맥 ⌥⌘B, 그 외 Ctrl+Alt+B */
export function shortcutText(sc: Shortcut, mac: boolean): string {
  return shortcutParts(sc, mac).join(mac ? '' : '+')
}

/** 표시 전용 키 이름(Alt·Ctrl)을 맥 기호로 바꾼다. 화살표 같은 나머지는 그대로 */
export function platformKey(token: string, mac: boolean): string {
  if (!mac) return token
  return ({ Alt: '⌥', Ctrl: '⌃', Shift: '⇧', Mod: '⌘' } as Record<string, string>)[token] ?? token
}
