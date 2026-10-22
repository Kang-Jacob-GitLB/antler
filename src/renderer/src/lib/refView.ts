// Solo / Hide(그래프 보기 범위) 순수 로직. 저장소는 건드리지 않고 어떤 ref 를 보여 줄지만 다룬다.
import { isRemoteAll, isValidScopeRef, SCOPE_MAX_ITEMS } from '@shared/logScope'
import type { LogScope, RefsSnapshot, RepoViewPrefs, ViewRef } from '@shared/types'

export interface RefView {
  solo: ViewRef[]
  hide: ViewRef[]
}

export const EMPTY_VIEW: RefView = { solo: [], hide: [] }

/** 리모트 전체(refs/remotes/<name>/*) 항목 */
export function remoteAllRef(name: string): ViewRef {
  return { id: `refs/remotes/${name}/*`, label: `${name}/*`, kind: 'remoteAll' }
}

/** 해시로만 가리킬 수 있는 항목(스태시·워크트리)은 숨길 수 없다 — 기본 보기에 원래 없다 */
export const canHide = (ref: ViewRef): boolean => ref.kind !== 'stash' && ref.kind !== 'commit'

export const inList = (list: readonly ViewRef[], id: string): boolean => list.some((r) => r.id === id)

/** 리모트 전체 항목 c(refs/remotes/<name>/*)가 id 를 덮는지. 이름에 '/' 가 있어도 git 의 --exclude 패턴과 같은 접두사 규칙이다 */
const covers = (c: string, id: string): boolean => c !== id && isRemoteAll(c) && id.startsWith(c.slice(0, -1))

/** id 가 숨김 목록에 있거나, 그 리모트 전체가 숨겨져 있으면 true */
export function isHidden(hide: readonly ViewRef[], id: string): boolean {
  return hide.some((r) => r.id === id || covers(r.id, id))
}

/** 숨김 목록에서 id 를 가리는 항목(그 ref 자신, 또는 그 리모트 전체)을 뺀다. 가리는 게 없으면 같은 배열을 돌려준다 */
export function unhidden(hide: readonly ViewRef[], id: string): readonly ViewRef[] {
  const hit = (r: ViewRef): boolean => r.id === id || covers(r.id, id)
  return hide.some(hit) ? hide.filter((r) => !hit(r)) : hide
}

/** 목록에 있으면 빼고 없으면 더한다 */
export function toggled(list: readonly ViewRef[], ref: ViewRef): ViewRef[] {
  return inList(list, ref.id) ? list.filter((r) => r.id !== ref.id) : [...list, ref].slice(0, SCOPE_MAX_ITEMS)
}

/**
 * HEAD 가 가리키는 ref 의 전체 이름. head.branch 는 `symbolic-ref --short HEAD` 라 로컬(feature/x)과
 * 원격 추적(origin/main — 원격 전용 저장소의 HEAD)을 이름만으론 구분할 수 없다. 스냅샷에 실제로 있는 ref 로 가린다:
 * 같은 이름의 로컬 브랜치가 있으면 그것, 없고 원격 브랜치가 있으면 그 fullName, 아무것도 없으면 refs/heads/ 로 본다.
 * (git 이 모호할 때 붙이는 heads/·remotes/ 접두사도 받는다)
 */
export function headRefId(headBranch: string | null, refs: Pick<RefsSnapshot, 'local' | 'remotes'> | null | undefined): string | null {
  if (!headBranch) return null
  if (refs) {
    if (headBranch.startsWith('heads/')) return `refs/${headBranch}`
    if (headBranch.startsWith('remotes/')) return `refs/${headBranch}`
    if (!refs.local.some((b) => b.name === headBranch)) {
      for (const r of refs.remotes) {
        const hit = r.branches.find((b) => b.name === headBranch)
        if (hit) return hit.fullName
      }
    }
  }
  return `refs/heads/${headBranch}`
}

/**
 * main 에 보낼 범위. solo 가 있으면 hide 는 적용하지 않는다.
 * 현재 브랜치(HEAD 가 가리키는 ref)를 숨기면 HEAD 로 다시 닿지 않도록 시작점에서 HEAD 를 뺀다.
 * headRef 는 headRefId() 의 결과 — 주지 않으면 refs/heads/<headBranch> 로 본다.
 */
export function buildScope(view: RefView, headBranch: string | null, headRef?: string | null): LogScope | undefined {
  // main 이 거부할 항목은 보내기 전에 버린다(한 항목 때문에 그래프 전체가 오류가 되지 않게)
  const solo = view.solo.map((r) => r.id).filter(isValidScopeRef)
  if (solo.length) return { solo }
  const hide = view.hide.map((r) => r.id).filter((id) => isValidScopeRef(id) && id.startsWith('refs/'))
  if (!hide.length) return undefined
  const headId = headRef ?? (headBranch ? `refs/heads/${headBranch}` : null)
  return { hide, ...(headId && hide.includes(headId) ? { includeHead: false } : {}) }
}

/** 저장돼 있던 값을 믿지 않고 형식·종류를 다시 걸러 낸다 */
export function sanitizeView(saved: RepoViewPrefs | undefined | null): RefView {
  const clean = (list: unknown, hideOnly: boolean): ViewRef[] => {
    if (!Array.isArray(list)) return []
    const seen = new Set<string>()
    const out: ViewRef[] = []
    for (const r of list as Partial<ViewRef>[]) {
      if (!r || typeof r !== 'object' || !isValidScopeRef(r.id) || typeof r.label !== 'string' || seen.has(r.id)) continue
      const kind = r.kind ?? 'branch'
      if (hideOnly && (kind === 'stash' || kind === 'commit' || !r.id.startsWith('refs/'))) continue
      seen.add(r.id)
      out.push({ id: r.id, label: r.label.slice(0, 200), kind })
      if (out.length >= SCOPE_MAX_ITEMS) break
    }
    return out
  }
  return { solo: clean(saved?.solo, false), hide: clean(saved?.hide, true) }
}

/** 지금 저장소에 없는 ref(삭제·이름 변경·사라진 리모트·버려진 스태시)를 목록에서 뺀다. 바뀐 게 없으면 같은 객체를 돌려준다 */
export function pruneView(view: RefView, refs: RefsSnapshot): RefView {
  const live = new Set<string>()
  for (const b of refs.local) live.add(b.fullName)
  for (const r of refs.remotes) {
    live.add(remoteAllRef(r.name).id)
    for (const b of r.branches) live.add(b.fullName)
  }
  for (const t of refs.tags) live.add(t.fullName)
  for (const s of refs.stashes) live.add(s.hash)
  const keep = (r: ViewRef): boolean => r.kind === 'commit' || live.has(r.id)
  const solo = view.solo.filter(keep)
  const hide = view.hide.filter(keep)
  return solo.length === view.solo.length && hide.length === view.hide.length ? view : { solo, hide }
}
