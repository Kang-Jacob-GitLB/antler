// git remote URL → GitHub owner/repo. github.com 만 다룬다(GitHub Enterprise Server 같은 자체 호스트는 범위 밖 —
// 호스트마다 API 주소·인증이 달라 별도 설정이 필요하므로 지금은 github.com 이 아니면 null 로 취급한다).
import type { GhRepoRef } from '@shared/types'

const HOSTS = new Set(['github.com', 'www.github.com'])
/** 스킴별 기본 포트. 명시했다면 이 값이어야 github.com 으로 본다(ssh://git@github.com:22/… 허용) */
const DEFAULT_PORT: Record<string, string> = { 'http:': '80', 'https:': '443', 'ssh:': '22', 'git:': '9418' }

const OWNER_RE = /^[A-Za-z0-9][A-Za-z0-9_-]{0,38}$/
const REPO_RE = /^[A-Za-z0-9._-]{1,100}$/

export interface GithubSlug {
  owner: string
  repo: string
}

function toSlug(path: string): GithubSlug | null {
  // 끝의 '/' 는 허용, 중간의 빈 세그먼트·추가 세그먼트(o/r/tree/main)는 거부
  const segs = path.replace(/\/+$/, '').split('/')
  if (segs[0] === '') segs.shift()
  if (segs.length !== 2) return null
  const owner = segs[0]!
  const repo = segs[1]!.replace(/\.git$/, '')
  if (!OWNER_RE.test(owner) || !REPO_RE.test(repo) || repo === '.' || repo === '..') return null
  return { owner, repo }
}

/**
 * GitHub remote URL 을 해석한다. 지원 형태:
 * https://github.com/o/r(.git) · http(s)://user:pass@github.com/o/r(자격 증명은 버린다) ·
 * git@github.com:o/r.git · ssh://git@github.com(:22)/o/r.git · git://github.com/o/r · www.github.com · 끝 슬래시.
 */
export function parseGithubRemote(url: string): GithubSlug | null {
  const s = url.trim()
  if (!s || /[\s\0]/.test(s)) return null

  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) {
    let u: URL
    try {
      u = new URL(s)
    } catch {
      return null
    }
    const defaultPort = DEFAULT_PORT[u.protocol]
    if (defaultPort === undefined) return null
    if (!HOSTS.has(u.hostname.toLowerCase())) return null
    // URL 은 기본 포트를 비워 주므로 비어 있지 않으면 스킴의 기본 포트와 다른 포트다
    if (u.port !== '' && u.port !== defaultPort) return null
    return toSlug(u.pathname)
  }

  // scp 형식: [user@]host:path (path 가 '/' 로 시작해도 된다)
  const m = /^(?:[^@/\s]+@)?([^:/\s]+):(.+)$/.exec(s)
  if (!m || !HOSTS.has(m[1]!.toLowerCase())) return null
  return toSlug(m[2]!)
}

/** 추론 우선순위: origin → upstream → 목록에서 처음 나오는 GitHub remote */
export function pickGithubRemote(remotes: { name: string; url: string }[]): GhRepoRef | null {
  const parsed = remotes.flatMap((r) => {
    const slug = parseGithubRemote(r.url)
    return slug ? [{ name: r.name, ...slug }] : []
  })
  const hit = parsed.find((r) => r.name === 'origin') ?? parsed.find((r) => r.name === 'upstream') ?? parsed[0]
  return hit ? { owner: hit.owner, repo: hit.repo, remote: hit.name } : null
}

/** 모든 GitHub remote(origin·upstream 등)의 저장소. 같은 저장소(대소문자 무시)는 먼저 나온 remote 하나만 둔다 */
export function pickGithubRemotes(remotes: { name: string; url: string }[]): GhRepoRef[] {
  const seen = new Set<string>()
  const out: GhRepoRef[] = []
  for (const r of remotes) {
    const slug = parseGithubRemote(r.url)
    if (!slug) continue
    const key = `${slug.owner}/${slug.repo}`.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    out.push({ owner: slug.owner, repo: slug.repo, remote: r.name })
  }
  return out
}
