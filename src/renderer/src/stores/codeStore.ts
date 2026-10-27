import { create } from 'zustand'
import type { CodeBlobResult, CodeTreeEntry } from '@shared/types'
import { api, errorMessage, isAbort, newRequestId } from '../lib/api'
import { useDetailStore } from './detailStore'
import { useFileViewStore } from './fileViewStore'
import { useGithubStore } from './githubStore'
import { useJiraStore } from './jiraStore'
import { useLaunchpadStore } from './launchpadStore'

export type CodeKind = 'dir' | 'file'

export interface CodeOpenOptions {
  /** 'HEAD' · 전체 ref 이름(refs/heads/…·refs/tags/…) · 커밋 해시. rev 가 있으면 무시한다 */
  ref?: string
  /** 이미 커밋 해시를 알면(resolveRev 를 건너뛴다) */
  rev?: string
  /** 머리줄에 보일 이름(브랜치·태그 이름 · HEAD · 짧은 해시). 없으면 ref 에서 만든다 */
  label?: string
  /** 처음 보여 줄 경로. 폴더인지 파일인지는 트리에서 알아낸다 */
  path?: string
}

export interface ReadmeState {
  /** 저장소 최상위 기준 경로 */
  path: string
  /** 파일 이름(상자 머리에 보인다) */
  name: string
  result: CodeBlobResult | null
  loading: boolean
  error: string | null
}

interface CodeState {
  /** 열려 있는 저장소(null 이면 닫힘) */
  repoId: string | null
  /** 사용자가 고른 ref(다시 resolve 할 때 쓰지는 않고 표시·선택 표시용) */
  ref: string
  /** 머리줄 이름 */
  label: string
  /** 보고 있는 커밋 해시. resolve 중이면 null */
  rev: string | null
  resolveError: string | null

  /** 현재 선택 경로('' = 최상위)와 종류 */
  path: string
  kind: CodeKind
  /** 디렉터리 경로 → 항목(지연 로드한 것만 있다) */
  tree: Map<string, CodeTreeEntry[]>
  treeLoading: Set<string>
  treeError: Map<string, string>
  expanded: Set<string>

  blob: CodeBlobResult | null
  blobLoading: boolean
  blobError: string | null
  /** 폴더를 볼 때 그 폴더의 README */
  readme: ReadmeState | null
  /** 마크다운 파일을 "코드"(원문)로 보는지(기본은 미리보기) */
  mdCode: boolean
  /** 문서를 연 뒤 스크롤할 #앵커(MarkdownDoc 가 처리하고 비운다) */
  anchor: string | null
  /** 요청한 경로가 이 시점에 없어 가까운 폴더로 물러났다는 안내 */
  missingPath: string | null

  open(repoId: string, opts?: CodeOpenOptions): void
  close(): void
  /** 다른 리비전으로 바꾼다(가능하면 현재 경로 유지) */
  switchRev(ref: string, label: string): void
  /** 경로로 이동: 조상 폴더를 펼치고 폴더면 목록, 파일이면 내용을 보인다. hash 는 문서 안 앵커 */
  goto(path: string, hash?: string): Promise<void>
  selectDir(path: string): Promise<void>
  selectFile(path: string): Promise<void>
  toggleDir(path: string): void
  expandDir(path: string): void
  collapseDir(path: string): void
  setMdCode(v: boolean): void
  setAnchor(a: string | null): void
}

const README_NAMES = ['readme.md', 'readme.markdown', 'readme']

/** 짧은 해시 */
export const shortRev = (h: string): string => h.slice(0, 7)

/** ref 전체 이름 → 사람이 보는 이름 */
export function refLabel(ref: string): string {
  return ref.replace(/^refs\/(heads|tags|remotes)\//, '')
}

const normPath = (p: string): string => p.split('/').filter(Boolean).join('/')
const parentOf = (p: string): string => p.slice(0, Math.max(0, p.lastIndexOf('/')))

const INITIAL = {
  repoId: null,
  ref: 'HEAD',
  label: 'HEAD',
  rev: null,
  resolveError: null,
  path: '',
  kind: 'dir',
  tree: new Map<string, CodeTreeEntry[]>(),
  treeLoading: new Set<string>(),
  treeError: new Map<string, string>(),
  expanded: new Set<string>(),
  blob: null,
  blobLoading: false,
  blobError: null,
  readme: null,
  mdCode: false,
  anchor: null,
  missingPath: null
} satisfies Partial<CodeState>

type TreeOutcome = { entries: CodeTreeEntry[] } | 'missing' | null

export const useCodeStore = create<CodeState>((set, get) => {
  /** 열기·리비전 전환마다 올라가는 세대. 늦게 도착한 응답이 새 화면을 덮지 않게 한다 */
  let epoch = 0
  const pending = new Set<string>()
  let blobReq: string | null = null
  let readmeReq: string | null = null
  const inflight = new Map<string, Promise<TreeOutcome>>()

  const cancel = (id: string | null): void => {
    if (!id) return
    pending.delete(id)
    void api.invoke('req:cancel', id)
  }
  const cancelAll = (): void => {
    for (const id of pending) void api.invoke('req:cancel', id)
    pending.clear()
    blobReq = null
    readmeReq = null
    inflight.clear()
  }
  const track = (prefix: string): string => {
    const id = newRequestId(prefix)
    pending.add(id)
    return id
  }

  const withSet = <T>(s: Set<T>, f: (n: Set<T>) => void): Set<T> => {
    const n = new Set(s)
    f(n)
    return n
  }

  function loadTree(dir: string): Promise<TreeOutcome> {
    const { repoId, rev, tree } = get()
    if (!repoId || !rev) return Promise.resolve(null)
    const cached = tree.get(dir)
    if (cached) return Promise.resolve({ entries: cached })
    const running = inflight.get(dir)
    if (running) return running
    const mine = epoch
    const p = (async (): Promise<TreeOutcome> => {
      const id = track('ctree')
      set((s) => ({ treeLoading: withSet(s.treeLoading, (n) => n.add(dir)), treeError: withoutKey(s.treeError, dir) }))
      try {
        const res = await api.invoke('git:tree', repoId, { rev, dir }, id)
        if (mine !== epoch) return null
        if (res.missing) {
          set((s) => ({ treeLoading: withSet(s.treeLoading, (n) => n.delete(dir)) }))
          return 'missing'
        }
        set((s) => ({ tree: new Map(s.tree).set(dir, res.entries), treeLoading: withSet(s.treeLoading, (n) => n.delete(dir)) }))
        return { entries: res.entries }
      } catch (err) {
        if (mine !== epoch || isAbort(err)) return null
        set((s) => ({ treeLoading: withSet(s.treeLoading, (n) => n.delete(dir)), treeError: new Map(s.treeError).set(dir, errorMessage(err)) }))
        return null
      } finally {
        pending.delete(id)
        if (mine === epoch) inflight.delete(dir)
      }
    })()
    inflight.set(dir, p)
    return p
  }

  function withoutKey<V>(m: Map<string, V>, k: string): Map<string, V> {
    if (!m.has(k)) return m
    const n = new Map(m)
    n.delete(k)
    return n
  }

  async function loadReadme(entries: CodeTreeEntry[]): Promise<void> {
    const { repoId, rev } = get()
    cancel(readmeReq)
    readmeReq = null
    const file = findReadme(entries)
    if (!repoId || !rev || !file) {
      set({ readme: null })
      return
    }
    const mine = epoch
    const id = track('creadme')
    readmeReq = id
    set({ readme: { path: file.path, name: file.name, result: null, loading: true, error: null } })
    try {
      const result = await api.invoke('git:blob', repoId, { rev, path: file.path }, id)
      if (mine !== epoch || readmeReq !== id) return
      set({ readme: { path: file.path, name: file.name, result, loading: false, error: null } })
    } catch (err) {
      if (mine !== epoch || readmeReq !== id || isAbort(err)) return
      set({ readme: { path: file.path, name: file.name, result: null, loading: false, error: errorMessage(err) } })
    } finally {
      pending.delete(id)
      if (readmeReq === id) readmeReq = null
    }
  }

  async function resolveAndShow(ref: string, path: string): Promise<void> {
    const mine = epoch
    const repoId = get().repoId
    if (!repoId) return
    try {
      const rev = await api.invoke('git:resolveRev', repoId, ref)
      if (mine !== epoch) return
      set({ rev })
    } catch (err) {
      if (mine !== epoch) return
      set({ resolveError: errorMessage(err) })
      return
    }
    await get().goto(path)
  }

  return {
    ...INITIAL,

    open(repoId, opts = {}) {
      epoch++
      cancelAll()
      const ref = opts.ref ?? opts.rev ?? 'HEAD'
      set({
        ...INITIAL,
        tree: new Map(),
        treeLoading: new Set(),
        treeError: new Map(),
        expanded: new Set(),
        repoId,
        ref,
        label: opts.label ?? (opts.rev ? shortRev(opts.rev) : refLabel(ref)),
        rev: opts.rev ?? null
      })
      // 코드 화면이 보이도록 위에 덮인 화면을 걷는다(코드 화면은 그래프 바로 위층이다)
      useLaunchpadStore.getState().show(false)
      useJiraStore.getState().show(null)
      useGithubStore.getState().show(null)
      useDetailStore.getState().closeFile()
      useFileViewStore.getState().close()
      const path = normPath(opts.path ?? '')
      if (opts.rev) void get().goto(path)
      else void resolveAndShow(ref, path)
    },

    close() {
      epoch++
      cancelAll()
      set({ ...INITIAL, tree: new Map(), treeLoading: new Set(), treeError: new Map(), expanded: new Set() })
    },

    switchRev(ref, label) {
      const { repoId, path } = get()
      if (!repoId) return
      epoch++
      cancelAll()
      set({
        ref,
        label,
        rev: null,
        resolveError: null,
        tree: new Map(),
        treeLoading: new Set(),
        treeError: new Map(),
        expanded: new Set(),
        blob: null,
        blobLoading: false,
        blobError: null,
        readme: null,
        missingPath: null,
        anchor: null
      })
      void resolveAndShow(ref, path)
    },

    async goto(rawPath, hash) {
      const mine = epoch
      const path = normPath(rawPath)
      const { rev } = get()
      if (!rev) return
      if (path === '') {
        await get().selectDir('')
        if (hash && mine === epoch) set({ anchor: hash })
        return
      }
      // 이미 보고 있는 파일이면 앵커만
      const cur = get()
      if (cur.kind === 'file' && cur.path === path && cur.blob) {
        set({ anchor: hash ?? null })
        return
      }
      const parts = path.split('/')
      let dir = ''
      /** 경로를 끝까지 따라가지 못했을 때: 따라간 데까지의 폴더를 보이고 안내를 띄운다 */
      const fallback = async (): Promise<void> => {
        if (mine !== epoch) return
        await get().selectDir(dir)
        if (mine === epoch) set({ missingPath: path })
      }
      for (let i = 0; i < parts.length; i++) {
        const out = await loadTree(dir)
        if (mine !== epoch) return
        if (!out || out === 'missing') return fallback()
        const entry = out.entries.find((e) => e.name === parts[i])
        if (!entry) return fallback()
        const last = i === parts.length - 1
        if (last) {
          if (entry.kind === 'dir') await get().selectDir(path)
          else if (entry.kind === 'submodule') return fallback()
          else {
            await get().selectFile(path)
            if (hash && mine === epoch) set({ anchor: hash })
          }
          return
        }
        if (entry.kind !== 'dir') return fallback()
        dir = dir ? `${dir}/${entry.name}` : entry.name
        set((s) => ({ expanded: withSet(s.expanded, (n) => n.add(dir)) }))
      }
    },

    async selectDir(path) {
      const mine = epoch
      cancel(blobReq)
      blobReq = null
      set((s) => ({
        path,
        kind: 'dir',
        blob: null,
        blobLoading: false,
        blobError: null,
        readme: null,
        mdCode: false,
        anchor: null,
        missingPath: null,
        expanded: path ? withSet(s.expanded, (n) => n.add(path)) : s.expanded
      }))
      const out = await loadTree(path)
      if (mine !== epoch) return
      const cur = get()
      if (cur.kind !== 'dir' || cur.path !== path) return
      if (out && out !== 'missing') await loadReadme(out.entries)
    },

    async selectFile(path) {
      const mine = epoch
      const { repoId, rev } = get()
      if (!repoId || !rev) return
      cancel(blobReq)
      cancel(readmeReq)
      readmeReq = null
      const id = track('cblob')
      blobReq = id
      set((s) => {
        // 조상 폴더를 펼쳐 현재 위치가 트리에 보이게 한다
        const expanded = withSet(s.expanded, (n) => {
          for (let d = parentOf(path); d; d = parentOf(d)) n.add(d)
        })
        return { path, kind: 'file', blob: null, blobLoading: true, blobError: null, readme: null, mdCode: false, anchor: null, missingPath: null, expanded }
      })
      try {
        const blob = await api.invoke('git:blob', repoId, { rev, path }, id)
        if (mine !== epoch || blobReq !== id) return
        set({ blob, blobLoading: false })
      } catch (err) {
        if (mine !== epoch || blobReq !== id || isAbort(err)) return
        set({ blobLoading: false, blobError: errorMessage(err) })
      } finally {
        pending.delete(id)
        if (blobReq === id) blobReq = null
      }
    },

    toggleDir(path) {
      if (get().expanded.has(path)) get().collapseDir(path)
      else get().expandDir(path)
    },

    expandDir(path) {
      set((s) => ({ expanded: withSet(s.expanded, (n) => n.add(path)) }))
      void loadTree(path)
    },

    collapseDir(path) {
      set((s) => ({ expanded: withSet(s.expanded, (n) => n.delete(path)) }))
    },

    setMdCode(v) {
      set({ mdCode: v })
    },

    setAnchor(a) {
      set({ anchor: a })
    }
  }
})

/** 폴더 안 항목 중 README 로 보일 파일(우선순위: readme.md → readme.markdown → readme, 대소문자 무시) */
export function findReadme(entries: CodeTreeEntry[]): CodeTreeEntry | undefined {
  return README_NAMES.map((n) => entries.find((e) => (e.kind === 'file' || e.kind === 'symlink') && e.name.toLowerCase() === n)).find(Boolean)
}

export const isMarkdownPath = (p: string): boolean => /\.(md|markdown)$/i.test(p)
