// 작성자 아바타(GitHub 프로필 사진) 주소 규칙. main·renderer 가 함께 쓰는 순수 함수라 네트워크를 쓰지 않는다.
export const AVATAR_ORIGIN = 'https://avatars.githubusercontent.com'

export interface NoreplyIdentity {
  login: string
  /** 사용자 숫자 ID(`<id>+<login>@…` 형식일 때만) */
  id?: number
}

const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})(?:\[bot\])?$/
const NOREPLY_RE = /^(?:(\d{1,12})\+)?([^@+\s]{1,50})@users\.noreply\.github\.com$/i

/**
 * GitHub noreply 이메일에서 사용자를 읽는다: `<id>+<login>@users.noreply.github.com` · `<login>@users.noreply.github.com`.
 * 로그인 형식이 아니거나 해석할 수 없으면 null.
 */
export function parseNoreply(email: string): NoreplyIdentity | null {
  const m = NOREPLY_RE.exec(email.trim())
  if (!m) return null
  const login = m[2]!
  if (!LOGIN_RE.test(login)) return null
  const id = m[1] ? Number(m[1]) : undefined
  // 봇(`name[bot]`)은 로그인으로는 이미지 주소를 만들 수 없어 ID 가 있어야 한다
  if (id === undefined && login.endsWith('[bot]')) return null
  return id !== undefined && Number.isSafeInteger(id) && id > 0 ? { login, id } : { login }
}

/** 크기(`s=`)를 붙이기 전의 아바타 기본 주소. ID 가 있으면 `/u/<id>`, 없으면 `/<login>` */
export function avatarBaseUrl(login: string, id?: number): string {
  return id !== undefined ? `${AVATAR_ORIGIN}/u/${id}` : `${AVATAR_ORIGIN}/${encodeURIComponent(login)}`
}

/**
 * GitHub API 의 avatar_url 을 기본 주소로 정리한다(쿼리 제거, 호스트 고정). 다른 호스트·형식이면 null.
 * 숫자 ID 를 알면 그것으로 `/u/<id>` 를 만든다.
 */
export function normalizeAvatarUrl(url: unknown, login: string, id?: number): string | null {
  if (typeof url !== 'string' || url.length > 300) return null
  let u: URL
  try {
    u = new URL(url)
  } catch {
    return null
  }
  if (u.origin !== AVATAR_ORIGIN || u.username || u.password) return null
  const fromPath = /^\/u\/(\d{1,12})$/.exec(u.pathname)
  const uid = id ?? (fromPath ? Number(fromPath[1]) : undefined)
  return uid !== undefined ? avatarBaseUrl(login, uid) : `${AVATAR_ORIGIN}${u.pathname}`
}

/** 요청 픽셀 크기를 몇 단계로 묶는다 — 같은 작성자의 이미지를 크기마다 따로 받지 않도록 */
const SIZE_BUCKETS = [32, 48, 64, 96, 128, 192]
export function avatarPx(cssPx: number, dpr: number): number {
  const need = Math.ceil(cssPx * Math.max(1, dpr))
  return SIZE_BUCKETS.find((b) => b >= need) ?? SIZE_BUCKETS[SIZE_BUCKETS.length - 1]!
}

/** 기본 주소 + 크기 → 이미지 src */
export function avatarSrc(base: string, px: number): string {
  return `${base}${base.includes('?') ? '&' : '?'}s=${px}`
}
