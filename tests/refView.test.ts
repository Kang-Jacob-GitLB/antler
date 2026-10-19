import { describe, expect, it } from 'vitest'
import { buildScope, canHide, isHidden, pruneView, remoteAllRef, sanitizeView, toggled, unhidden, type RefView } from '../src/renderer/src/lib/refView'
import type { RefsSnapshot, ViewRef } from '../src/shared/types'

const ref = (id: string, kind: ViewRef['kind'] = 'branch'): ViewRef => ({ id, label: id, kind })
const H40 = 'a'.repeat(40)

describe('buildScope', () => {
  it('둘 다 비어 있으면 기본 보기(undefined)', () => {
    expect(buildScope({ solo: [], hide: [] }, 'main')).toBeUndefined()
  })
  it('solo 가 있으면 hide 는 적용하지 않는다', () => {
    expect(buildScope({ solo: [ref('refs/heads/a')], hide: [ref('refs/heads/b')] }, 'main')).toEqual({ solo: ['refs/heads/a'] })
  })
  it('현재 브랜치를 숨기면 includeHead=false', () => {
    expect(buildScope({ solo: [], hide: [ref('refs/heads/main')] }, 'main')).toEqual({ hide: ['refs/heads/main'], includeHead: false })
    expect(buildScope({ solo: [], hide: [ref('refs/heads/x')] }, 'main')).toEqual({ hide: ['refs/heads/x'] })
    expect(buildScope({ solo: [], hide: [ref('refs/heads/main')] }, null)).toEqual({ hide: ['refs/heads/main'] })
  })
})

describe('isHidden / unhidden', () => {
  const hide = [remoteAllRef('origin'), ref('refs/tags/t1', 'tag')]
  it('리모트 전체가 숨겨지면 그 리모트의 브랜치도 숨겨진 것', () => {
    expect(isHidden(hide, 'refs/remotes/origin/main')).toBe(true)
    expect(isHidden(hide, 'refs/remotes/origin/a/b')).toBe(true)
    expect(isHidden(hide, 'refs/remotes/up/main')).toBe(false)
    expect(isHidden(hide, 'refs/tags/t1')).toBe(true)
    expect(isHidden(hide, 'refs/heads/main')).toBe(false)
  })
  it('이름에 / 가 든 리모트 전체도 그 브랜치를 덮는다', () => {
    const h = [remoteAllRef('team/alice')]
    expect(isHidden(h, 'refs/remotes/team/alice/feat/x')).toBe(true)
    expect(isHidden(h, 'refs/remotes/team/bob/x')).toBe(false)
    expect(unhidden(h, 'refs/remotes/team/alice/x')).toEqual([])
  })
  it('unhidden 은 자기 자신 또는 덮는 리모트 전체를 뺀다', () => {
    expect(unhidden(hide, 'refs/remotes/origin/main').map((r) => r.id)).toEqual(['refs/tags/t1'])
    expect(unhidden(hide, 'refs/tags/t1').map((r) => r.id)).toEqual(['refs/remotes/origin/*'])
    expect(unhidden(hide, 'refs/heads/none')).toBe(hide)
  })
})

describe('toggled / canHide', () => {
  it('있으면 빼고 없으면 더한다', () => {
    const a = ref('refs/heads/a')
    expect(toggled([], a)).toEqual([a])
    expect(toggled([a], a)).toEqual([])
  })
  it('스태시·워크트리 해시는 숨길 수 없다', () => {
    expect(canHide(ref(H40, 'stash'))).toBe(false)
    expect(canHide(ref(H40, 'commit'))).toBe(false)
    expect(canHide(ref('refs/heads/a'))).toBe(true)
  })
})

describe('sanitizeView', () => {
  it('형식이 틀린 항목·중복·해시 숨김을 걸러 낸다', () => {
    const v = sanitizeView({
      solo: [ref('refs/heads/a'), ref('refs/heads/a'), ref('--output=x'), ref(H40, 'stash'), { id: 5 } as never],
      hide: [ref('refs/tags/t', 'tag'), ref(H40, 'stash'), ref('stash@{0}', 'stash')]
    })
    expect(v.solo.map((r) => r.id)).toEqual(['refs/heads/a', H40])
    expect(v.hide.map((r) => r.id)).toEqual(['refs/tags/t'])
  })
  it('저장된 값이 없거나 배열이 아니면 빈 보기', () => {
    expect(sanitizeView(undefined)).toEqual({ solo: [], hide: [] })
    expect(sanitizeView({ solo: 'x' as never })).toEqual({ solo: [], hide: [] })
  })
})

describe('pruneView', () => {
  const refs = {
    local: [{ fullName: 'refs/heads/main' }],
    remotes: [{ name: 'origin', branches: [{ fullName: 'refs/remotes/origin/main' }] }],
    tags: [{ fullName: 'refs/tags/t1' }],
    stashes: [{ hash: H40 }]
  } as unknown as RefsSnapshot

  it('사라진 ref 를 빼고, 바뀐 게 없으면 같은 객체를 돌려준다', () => {
    const same: RefView = { solo: [ref('refs/heads/main')], hide: [ref('refs/tags/t1', 'tag'), remoteAllRef('origin')] }
    expect(pruneView(same, refs)).toBe(same)
    const stale: RefView = {
      solo: [ref('refs/heads/gone'), ref(H40, 'stash'), ref('b'.repeat(40), 'stash'), ref('c'.repeat(40), 'commit')],
      hide: [ref('refs/tags/old', 'tag'), remoteAllRef('up'), ref('refs/remotes/origin/main', 'remote')]
    }
    const pruned = pruneView(stale, refs)
    expect(pruned.solo.map((r) => r.id)).toEqual([H40, 'c'.repeat(40)])
    expect(pruned.hide.map((r) => r.id)).toEqual(['refs/remotes/origin/main'])
  })
})
