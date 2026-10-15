import type { SubmoduleStatus } from '@shared/types'

/** 인덱스의 gitlink(모드 160000) 조회. 커밋 전 상태(`git add` 직후)도 반영하려고 ls-tree 가 아니라 인덱스를 본다. */
export const GITLINK_ARGS = ['ls-files', '-s', '-z']
/**
 * `.gitmodules` 를 설정 파일로 읽는다. 파일을 설정으로 읽을 뿐이라 명령을 실행하지 않는다.
 * `git submodule` 은 하위 저장소 설정까지 건드리므로 쓰지 않는다.
 */
export const GITMODULES_ARGS = ['config', '-z', '--file', '.gitmodules', '--get-regexp', '^submodule\\.']

export interface Gitlink {
  path: string
  /** 상위 저장소가 기록한 커밋 */
  sha: string
}

export interface ModuleEntry {
  name: string
  url?: string
  branch?: string
}

/** `ls-files -s -z` 출력에서 gitlink 만 뽑는다. 레코드: `160000 <sha> <stage>\t<path>`. 충돌(stage>0)은 경로당 하나만 남긴다. */
export function parseGitlinks(buf: Buffer | string): Gitlink[] {
  const seen = new Set<string>()
  const out: Gitlink[] = []
  for (const rec of buf.toString('utf8').split('\0')) {
    if (!rec.startsWith('160000 ')) continue
    const tab = rec.indexOf('\t')
    if (tab < 0) continue
    const sha = rec.slice(0, tab).split(' ')[1]
    const path = rec.slice(tab + 1)
    if (!sha || !path || seen.has(path)) continue
    seen.add(path)
    out.push({ path, sha })
  }
  return out
}

/**
 * `config -z --get-regexp` 출력(`key\nvalue\0`)에서 경로 → 모듈 정보를 만든다.
 * 키는 `submodule.<name>.<변수>` — 이름에 '.' 이 들어갈 수 있으므로 마지막 '.' 로 자른다. URL 은 표시용 텍스트일 뿐이다.
 */
export function parseGitmodules(buf: Buffer | string): Map<string, ModuleEntry> {
  const prefix = 'submodule.'
  const byName = new Map<string, { path?: string; url?: string; branch?: string }>()
  for (const rec of buf.toString('utf8').split('\0')) {
    if (!rec) continue
    const nl = rec.indexOf('\n')
    const key = nl < 0 ? rec : rec.slice(0, nl)
    const value = nl < 0 ? '' : rec.slice(nl + 1)
    if (!key.startsWith(prefix)) continue
    const dot = key.lastIndexOf('.')
    if (dot <= prefix.length) continue
    const name = key.slice(prefix.length, dot)
    const variable = key.slice(dot + 1).toLowerCase()
    const cur = byName.get(name) ?? {}
    if (variable === 'path') cur.path = value
    else if (variable === 'url') cur.url = value
    else if (variable === 'branch') cur.branch = value
    byName.set(name, cur)
  }
  const out = new Map<string, ModuleEntry>()
  for (const [name, v] of byName) {
    if (!v.path) continue
    out.set(v.path, { name, ...(v.url ? { url: v.url } : {}), ...(v.branch ? { branch: v.branch } : {}) })
  }
  return out
}

/** `rev-parse --absolute-git-dir HEAD` 출력(줄 단위)에서 git-dir 과 HEAD 해시를 읽는다. HEAD 가 없으면 해시는 없다. */
export function parseProbe(stdout: string): { gitDir?: string; head?: string } {
  const lines = stdout.split('\n').map((l) => l.replace(/\r$/, ''))
  const gitDir = lines[0] || undefined
  const second = lines[1] ?? ''
  const head = /^[0-9a-f]{40}([0-9a-f]{24})?$/i.test(second) ? second.toLowerCase() : undefined
  return { ...(gitDir ? { gitDir } : {}), ...(head ? { head } : {}) }
}

export function submoduleStatus(recorded: string, head: string | undefined): SubmoduleStatus {
  if (!head) return 'unknown'
  return head === recorded ? 'clean' : 'modified'
}
