import type { Worktree } from '@shared/types'

/** `git worktree list --porcelain -z` 인자. `-z` 는 git 2.36+ 가 필요하다. */
export const WORKTREE_ARGS = ['worktree', 'list', '--porcelain', '-z']

/**
 * `--porcelain -z` 출력을 해석한다. 속성은 NUL 로 끝나고 레코드 사이에는 빈 필드(NUL NUL)가 들어간다.
 * 경로에 개행·한글·공백이 있어도 안전하다. 첫 레코드가 주 작업 트리다.
 */
export function parseWorktrees(buf: Buffer | string): Worktree[] {
  const out: Worktree[] = []
  let cur: Worktree | null = null

  for (const field of buf.toString('utf8').split('\0')) {
    if (field === '') {
      if (cur) out.push(cur)
      cur = null
      continue
    }
    const sp = field.indexOf(' ')
    const key = sp === -1 ? field : field.slice(0, sp)
    const value = sp === -1 ? '' : field.slice(sp + 1)
    if (key === 'worktree') {
      if (cur) out.push(cur)
      cur = { path: value, head: null, branch: null, detached: false, bare: false, locked: false, prunable: false, isMain: out.length === 0 }
      continue
    }
    if (!cur) continue
    switch (key) {
      case 'HEAD':
        // 커밋이 없는 새 저장소는 전부 0 인 해시를 돌려준다
        cur.head = /^0+$/.test(value) ? null : value
        break
      case 'branch':
        cur.branch = value.replace(/^refs\/heads\//, '')
        break
      case 'detached':
        cur.detached = true
        break
      case 'bare':
        cur.bare = true
        break
      case 'locked':
        cur.locked = true
        break
      case 'prunable':
        cur.prunable = true
        break
    }
  }
  if (cur) out.push(cur)
  return out
}
