// Gitflow 표시용 순수 함수. 설정값은 화면에 보이는 데이터로만 쓰고 명령으로 실행하지 않는다.
import type { GitflowConfig, GitflowInfo, GitflowPrefixKind } from './types'

export const GITFLOW_PREFIX_KINDS: readonly GitflowPrefixKind[] = ['feature', 'release', 'hotfix', 'bugfix', 'support']

/** git-flow 의 기본 접두사 */
export const GITFLOW_DEFAULT_PREFIXES: Record<GitflowPrefixKind, string> = {
  feature: 'feature/',
  release: 'release/',
  hotfix: 'hotfix/',
  bugfix: 'bugfix/',
  support: 'support/'
}

/** 설정값 길이 상한. 브랜치 이름·접두사로 이보다 긴 값은 정상적이지 않다 */
export const GITFLOW_VALUE_MAX = 100

const CONTROL_RE = new RegExp("[\u0000-\u001f\u007f-\u009f\u2028\u2029]")

/** 표시해도 되는 값만 통과시킨다: 제어문자·줄바꿈이 있거나 너무 길거나 비어 있으면 null */
function cleanValue(v: string): string | null {
  if (!v || v.length > GITFLOW_VALUE_MAX || CONTROL_RE.test(v) || v.trim() === '') return null
  return v
}

/**
 * `git config --null --get-regexp ^gitflow\.` 출력(`key\nvalue\0` 반복)을 읽는다.
 * 같은 키가 여러 번 나오면 마지막 값이 이긴다. 알려진 키만 받고, 값이 안전하지 않으면 그 키를 버린다.
 * 쓸 만한 키가 하나도 없으면 null(설정 없음).
 */
export function parseGitflowConfig(raw: string): GitflowConfig | null {
  const cfg: GitflowConfig = { prefixes: {} }
  let any = false
  for (const rec of raw.split('\0')) {
    const nl = rec.indexOf('\n')
    if (nl < 0) continue
    const key = rec.slice(0, nl).toLowerCase()
    const value = cleanValue(rec.slice(nl + 1))
    if (value === null) continue
    if (key === 'gitflow.branch.master') cfg.master = value
    else if (key === 'gitflow.branch.develop') cfg.develop = value
    else if (key.startsWith('gitflow.prefix.')) {
      const kind = key.slice('gitflow.prefix.'.length) as GitflowPrefixKind
      if (!GITFLOW_PREFIX_KINDS.includes(kind)) continue
      cfg.prefixes[kind] = value
    } else continue
    any = true
  }
  return any ? cfg : null
}

/**
 * 표시에 쓸 Gitflow 정보. 설정이 있으면 그 값(빠진 접두사는 기본값)을 쓰고,
 * 없으면 기본 접두사가 실제 브랜치에 쓰이는 경우에만 감지한 것으로 본다. 둘 다 아니면 null.
 * active: 설정이 있거나 develop 브랜치가 있을 때만 true — 아니면 접두사 범주 배지만 보인다.
 * names 는 리모트 이름을 뗀 브랜치 이름이다.
 */
export function resolveGitflow(cfg: GitflowConfig | null, names: Iterable<string>): GitflowInfo | null {
  const prefixes = { ...GITFLOW_DEFAULT_PREFIXES, ...cfg?.prefixes }
  const list = [...names]
  const develop = cfg?.develop ?? 'develop'
  if (cfg) return { source: 'config', active: true, master: cfg.master ?? null, develop, prefixes }
  if (!list.some((n) => GITFLOW_PREFIX_KINDS.some((k) => n.startsWith(prefixes[k]) && n.length > prefixes[k].length))) return null
  return { source: 'detected', active: list.includes(develop), master: null, develop, prefixes }
}

export type GitflowRole = GitflowPrefixKind | 'develop' | 'master'

/** 브랜치 이름(리모트 이름 제외)의 Gitflow 범주. 해당 없으면 null */
export function classifyBranch(name: string, flow: GitflowInfo): GitflowRole | null {
  if (name === flow.develop) return 'develop'
  if (flow.active && (flow.master ? name === flow.master : name === 'master' || name === 'main')) return 'master'
  let best: GitflowPrefixKind | null = null
  for (const k of GITFLOW_PREFIX_KINDS) {
    const p = flow.prefixes[k]
    if (name.length > p.length && name.startsWith(p) && (best === null || p.length > flow.prefixes[best].length)) best = k
  }
  return best
}
