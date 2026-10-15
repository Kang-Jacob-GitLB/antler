// renderer 가 보낸 GitHub IPC 인자 검증. IPC 인자는 신뢰하지 않는다.
import type { GhListQuery } from '@shared/types'

const MAX_FILTER_LEN = 100
const MAX_PAT_LEN = 255
const MAX_ID_LEN = 200
const MAX_PAGE = 1000
const STATES = new Set(['open', 'closed', 'all'])
// eslint-disable-next-line no-control-regex
const CONTROL_RE = /[\u0000-\u001f\u007f]/

export function parseNumber(n: unknown): number {
  if (typeof n !== 'number' || !Number.isInteger(n) || n <= 0 || n >= 1e9) throw new Error('잘못된 번호입니다')
  return n
}

export function parseId(v: unknown, what: string): string {
  if (typeof v !== 'string' || !v || v.length > MAX_ID_LEN || CONTROL_RE.test(v)) throw new Error(`잘못된 ${what}입니다`)
  return v
}

function parseFilter(v: unknown, what: string): string | undefined {
  if (v === undefined || v === null || v === '') return undefined
  if (typeof v !== 'string' || v.length > MAX_FILTER_LEN || CONTROL_RE.test(v)) throw new Error(`잘못된 ${what}입니다`)
  return v
}

/** 목록 질의를 검증하고 정규화한다(빈 필터는 제거, page 기본값 1) */
export function parseListQuery(q: unknown): GhListQuery {
  const i = q as Partial<GhListQuery> | null
  if (!i || typeof i !== 'object' || typeof i.state !== 'string' || !STATES.has(i.state)) throw new Error('잘못된 조회 조건입니다')
  const label = parseFilter(i.label, '라벨')
  const assignee = parseFilter(i.assignee, '담당자')
  let page = 1
  if (i.page !== undefined) {
    if (typeof i.page !== 'number' || !Number.isInteger(i.page) || i.page < 1 || i.page > MAX_PAGE) throw new Error('잘못된 페이지입니다')
    page = i.page
  }
  return { state: i.state, ...(label ? { label } : {}), ...(assignee ? { assignee } : {}), page }
}

/** PAT: 앞뒤 공백을 자르고, 보이는 ASCII 만(공백·제어 문자 없음) 255자까지 */
export function parsePat(v: unknown): string {
  if (typeof v !== 'string') throw new Error('토큰이 올바르지 않습니다')
  const t = v.trim()
  if (!t || t.length > MAX_PAT_LEN || !/^[\x21-\x7e]+$/.test(t)) throw new Error('토큰 형식이 올바르지 않습니다(공백 없이 255자 이하)')
  return t
}

/** gh:openExternal 허용 대상: origin 이 정확히 https://github.com 인 URL 만(자격 증명·다른 호스트 거부). 정규화된 href 를 돌려준다 */
export function allowedExternalUrl(raw: unknown): string | null {
  if (typeof raw !== 'string' || raw.length > 2048) return null
  let u: URL
  try {
    u = new URL(raw)
  } catch {
    return null
  }
  if (u.protocol !== 'https:' || u.origin !== 'https://github.com' || u.username || u.password) return null
  return u.href
}
