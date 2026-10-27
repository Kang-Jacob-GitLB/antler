/** code:openLink 허용 대상: https 주소이고 자격 증명(username/password)이 없는 것만. 정규화된 href 를 돌려준다 */
export function allowedLinkUrl(raw: unknown): string | null {
  if (typeof raw !== 'string' || raw.length > 4096) return null
  let u: URL
  try {
    u = new URL(raw)
  } catch {
    return null
  }
  if (u.protocol !== 'https:' || u.username || u.password) return null
  return u.href
}
