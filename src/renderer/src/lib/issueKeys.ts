// 커밋 메시지·PR 제목에서 Jira 이슈 키(ABC-123)를 뽑는다. 순수 함수.
// - 앞뒤가 영숫자·'_'·'-' 로 이어진 토큰의 일부는 키가 아니다(CVE-2024-1234, foo-ABC-1, ABC-1x).
//   뒤에 '-글자'(ABC-123-fix)는 허용한다.
// - 프로젝트 키를 설정했으면 그 키만, 없으면 일반 패턴이되 UTF-8·SHA-256 처럼 이슈 키가 아닌 흔한 접두어는 제외한다.

const KEY_RE = /(?<![A-Za-z0-9_-])([A-Z][A-Z0-9_]{1,19})-(\d{1,7})(?![A-Za-z0-9_]|-\d)/g

/** 일반 패턴에서만 거르는 접두어(설정한 프로젝트 키로 지정하면 그대로 인식한다) */
const NOT_ISSUE_PREFIXES = new Set(['UTF', 'SHA', 'ISO', 'MD', 'RFC', 'AES', 'RSA', 'CVE', 'CWE', 'GH', 'HTTP', 'TLS', 'SSL', 'IPV', 'UTC', 'GMT', 'USB', 'ES', 'PR'])

const MAX_KEYS = 10

/** 텍스트들에서 이슈 키를 등장 순서대로(중복 제거, 최대 10개) 돌려준다 */
export function extractIssueKeys(texts: (string | undefined)[], projectKeys?: readonly string[]): string[] {
  const only = projectKeys && projectKeys.length > 0 ? new Set(projectKeys) : null
  const out: string[] = []
  for (const text of texts) {
    if (!text) continue
    for (const m of text.matchAll(KEY_RE)) {
      const prefix = m[1]!
      if (only ? !only.has(prefix) : NOT_ISSUE_PREFIXES.has(prefix)) continue
      const key = `${prefix}-${m[2]}`
      if (!out.includes(key)) out.push(key)
      if (out.length >= MAX_KEYS) return out
    }
  }
  return out
}

/** 설정 입력("ABC, DEF" 등)을 프로젝트 키 목록으로. 형식이 맞지 않는 항목은 버린다 */
export function parseProjectKeys(input: string): string[] {
  return [...new Set(input.split(/[\s,;]+/).map((s) => s.trim().toUpperCase()).filter((s) => /^[A-Z][A-Z0-9_]{1,19}$/.test(s)))].slice(0, 50)
}
