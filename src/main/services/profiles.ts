// 프로필(이름 붙인 설정 묶음)의 순수 로직: 설정에서 스냅샷 만들기, 검증, 적용.
// 토큰(GitHub·Jira)과 앱 전체 설정(OAuth client ID, Jira 사이트, 에디터 실행 파일 경로, 마지막 저장소, 그룹 색 등)은
// PROFILE_KEYS 에 없으므로 어떤 프로필에도 들어가지 않고 프로필을 바꿔도 그대로다.
import type { EditorKind, Profile, ProfileSettings, UiPrefs } from '@shared/types'

export const MAX_PROFILES = 20
export const MAX_PROFILE_NAME = 50
const MAX_SETTINGS_JSON = 20_000

/** 프로필에 담기는 설정 키(이 밖의 키는 거부한다) */
export const PROFILE_KEYS = [
  'theme',
  'locale',
  'paneWidths',
  'diffMode',
  'sidebarHidden',
  'detailHidden',
  'detailMetaHeight',
  'minimapCollapsed',
  'graphInitials',
  'autoFetchMinutes',
  'editor',
  'visibleGroups'
] as const satisfies readonly (keyof ProfileSettings & keyof UiPrefs)[]

/** 없을 수 없는 키: 프로필에 값이 없으면 현재 값을 유지한다 */
const REQUIRED: readonly string[] = ['theme', 'locale', 'paneWidths']

export const EDITORS: readonly EditorKind[] = ['vscode', 'cursor', 'jetbrains', 'custom']

const bad = (what: string): never => {
  throw new Error(`잘못된 프로필 ${what}입니다`)
}

const isBool = (v: unknown): v is boolean => typeof v === 'boolean'
const isNum = (v: unknown, lo: number, hi: number): v is number => typeof v === 'number' && Number.isFinite(v) && v >= lo && v <= hi

/** 보일 저장소 그룹 목록의 모양 검사(프로필·config:setUi 가 함께 쓴다) */
export function checkVisibleGroups(v: unknown): string[] {
  if (!Array.isArray(v) || v.length > 50) return bad('그룹 목록')
  const out: string[] = []
  for (const g of v) {
    if (typeof g !== 'string' || !g.trim() || g.length > 200 || g.includes('\0')) return bad('그룹 이름')
    if (!out.includes(g)) out.push(g)
  }
  return out
}

export function checkEditor(v: unknown): EditorKind {
  if (!EDITORS.includes(v as EditorKind)) return bad('에디터')
  return v as EditorKind
}

/** 이름: 앞뒤 공백을 지운 1~50자, 제어 문자 없음 */
export function checkProfileName(v: unknown): string {
  if (typeof v !== 'string') return bad('이름')
  const name = v.trim()
  if (!name || name.length > MAX_PROFILE_NAME || /[\u0000-\u001f\u007f]/.test(name)) return bad('이름(1~50자)')
  return name
}

/** 알 수 없는 키·잘못된 값은 거부하고, 통과한 값만 복사해 돌려준다 */
export function validateSettings(v: unknown): ProfileSettings {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return bad('설정')
  const out: Record<string, unknown> = {}
  for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
    if (val === undefined) continue
    switch (k) {
      case 'theme':
        out[k] = ['system', 'dark', 'light'].includes(val as string) ? val : bad('테마')
        break
      case 'locale':
        out[k] = ['ko', 'en'].includes(val as string) ? val : bad('언어')
        break
      case 'paneWidths':
        out[k] = Array.isArray(val) && val.length === 2 && val.every((n) => isNum(n, 100, 1200)) ? val.map((n: number) => Math.round(n)) : bad('패널 폭')
        break
      case 'diffMode':
        out[k] = ['unified', 'split'].includes(val as string) ? val : bad('diff 방식')
        break
      case 'sidebarHidden':
      case 'detailHidden':
      case 'minimapCollapsed':
      case 'graphInitials':
        out[k] = isBool(val) ? val : bad(k)
        break
      case 'detailMetaHeight':
        out[k] = isNum(val, 0, 4000) ? Math.round(val) : bad('상세 머리 높이')
        break
      case 'autoFetchMinutes':
        out[k] = isNum(val, 0, 1440) && Number.isInteger(val) ? val : bad('자동 fetch 주기')
        break
      case 'editor':
        out[k] = checkEditor(val)
        break
      case 'visibleGroups':
        {
          // 빈 목록 = 제한 없음(키를 두지 않는다)
          const g = checkVisibleGroups(val)
          if (g.length) out[k] = g
        }
        break
      default:
        return bad(`설정 키(${k.slice(0, 40)})`)
    }
  }
  if (JSON.stringify(out).length > MAX_SETTINGS_JSON) return bad('설정 크기')
  return out as ProfileSettings
}

/** 현재 UI 설정에서 프로필에 담을 키만 뽑는다 */
export function snapshotSettings(ui: UiPrefs): ProfileSettings {
  const out: Record<string, unknown> = {}
  for (const k of PROFILE_KEYS) {
    const val = ui[k]
    if (val !== undefined) out[k] = Array.isArray(val) ? [...val] : val
  }
  return out as ProfileSettings
}

/** 프로필 설정을 UI 설정에 덮어쓴다. 프로필에 없는 선택 키는 지워서(기본값) 프로필 그대로 재현한다 */
export function applySettings(ui: UiPrefs, settings: ProfileSettings): UiPrefs {
  const next: Record<string, unknown> = { ...ui }
  for (const k of PROFILE_KEYS) {
    if (settings[k] !== undefined) next[k] = settings[k]
    else if (!REQUIRED.includes(k)) delete next[k]
  }
  return next as unknown as UiPrefs
}

export const sameName = (a: string, b: string): boolean => a.toLowerCase() === b.toLowerCase()

/** 디스크에서 읽은 프로필 목록 정리: 모양이 틀린 항목·중복은 버린다 */
export function sanitizeProfiles(raw: unknown): Profile[] {
  if (!Array.isArray(raw)) return []
  const out: Profile[] = []
  for (const p of raw as Partial<Profile>[]) {
    if (out.length >= MAX_PROFILES) break
    try {
      if (!p || typeof p.id !== 'string' || !p.id || p.id.length > 100) continue
      const name = checkProfileName(p.name)
      if (out.some((o) => o.id === p.id || sameName(o.name, name))) continue
      out.push({ id: p.id, name, settings: validateSettings(p.settings) })
    } catch {
      /* 잘못된 항목은 건너뛴다 */
    }
  }
  return out
}
