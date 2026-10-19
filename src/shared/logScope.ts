import type { LogScope } from './types'

/** 보기 범위 한 목록의 항목 수 상한(main 검증과 renderer 저장이 같이 쓴다) */
export const SCOPE_MAX_ITEMS = 200

// 보기 범위 항목 형식: ref 이름에 쓸 수 없는 문자(공백·제어 문자·~ ^ : ? * [ \)는 모두 거른다
const SCOPE_REF_RE = /^refs\/(heads|tags|remotes)\/[^\s\x00-\x1f\x7f~^:?*[\\]+$/
// 리모트 이름에는 '/' 가 들어 있을 수 있다(team/alice). 이름 부분은 ref 이름과 같은 규칙을 따른다
const SCOPE_REMOTE_ALL_RE = /^refs\/remotes\/([^\s\x00-\x1f\x7f~^:?*[\\]+)\/\*$/
export const SCOPE_HASH_RE = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/
/** ref 이름에 쓸 수 없는 모양: '..', '@{', 빈 구성 요소('//'), 끝 '/', '.lock', '.' 로 시작하는 구성 요소 */
const BAD_REF_SHAPE_RE = /\.\.|@\{|\/\/|\/$|\.lock$|\/\./

/** 리모트 전체 패턴(refs/remotes/<name>/*) 인지. 이름에 '/' 가 있어도 된다 */
export function isRemoteAll(id: string): boolean {
  const m = SCOPE_REMOTE_ALL_RE.exec(id)
  return !!m && !BAD_REF_SHAPE_RE.test(id.slice(0, -2))
}

/** LogScope 항목으로 쓸 수 있는 형식인지: 전체 ref 이름 · 40/64자 해시 · 리모트 전체 패턴 */
export function isValidScopeRef(item: unknown): item is string {
  if (typeof item !== 'string' || item.length > 1024) return false
  if (SCOPE_HASH_RE.test(item) || isRemoteAll(item)) return true
  return SCOPE_REF_RE.test(item) && !BAD_REF_SHAPE_RE.test(item)
}

/** 범위가 비어 있으면(모든 ref 를 그대로 보는 기본 보기) true */
export function isEmptyScope(scope: LogScope | undefined): boolean {
  return !scope || (!scope.solo?.length && !scope.hide?.length)
}

/**
 * 범위의 정규 키. 같은 범위면 항목 순서와 무관하게 같은 문자열이다(커서·활동 캐시 키, effect 의존성).
 * 비어 있는 범위는 ''.
 */
export function scopeKey(scope: LogScope | undefined): string {
  if (!scope || isEmptyScope(scope)) return ''
  const solo = [...new Set(scope.solo ?? [])].sort()
  // solo 가 있으면 hide 는 적용되지 않는다
  if (solo.length) return `solo\0${solo.join('\0')}`
  const hide = [...new Set(scope.hide ?? [])].sort()
  return `hide\0${scope.includeHead === false ? 'nohead' : 'head'}\0${hide.join('\0')}`
}
