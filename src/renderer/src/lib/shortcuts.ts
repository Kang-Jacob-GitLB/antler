// 단축키 정의·판정·표기(순수 함수). 모든 단축키는 Ctrl(맥은 ⌘)을 기본으로 누르고, 거기에 Shift·Alt 가 더해진다.
// 판정은 e.key 가 ASCII 문자(a-z, '/')면 그 글자로(Dvorak·Colemak·독일어 배열에서도 글자 그대로),
// 아니면(한글 IME 의 'ㅔ'·'Process', mac Option 조합 등) 물리 키 위치 e.code 로 한다.

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
  key?: string
}

/** 'KeyP' → 'p', 'Slash' → '/'. ASCII 글자로 판정할 수 없는 코드면 null */
function asciiOf(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase()
  return code === 'Slash' ? '/' : null
}

/** Ctrl 또는 ⌘ 이 눌렸고, Shift·Alt 상태가 정의와 정확히 같을 때만 일치한다 */
export function matchesShortcut(e: KeyEventLike, sc: Shortcut): boolean {
  if (!(e.ctrlKey || e.metaKey) || e.altKey !== (sc.alt === true)) return false
  const want = asciiOf(sc.code)
  const key = e.key && e.key.length === 1 ? e.key.toLowerCase() : ''
  if (want && /^[a-z/]$/.test(key)) {
    if (key !== want) return false
    // '/' 는 배열에 따라 Shift 와 함께 입력된다(독일어 Shift+7). 글자 키만 Shift 를 엄격히 본다
    return want === '/' || e.shiftKey === (sc.shift === true)
  }
  return e.code === sc.code && e.shiftKey === (sc.shift === true)
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
