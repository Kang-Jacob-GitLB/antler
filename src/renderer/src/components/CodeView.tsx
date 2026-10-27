import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { CodeBlobResult, CodeTreeEntry } from '@shared/types'
import { useT, type MessageKey } from '../i18n'
import { formatBytes } from '../lib/format'
import { useActiveRepo, useAppStore } from '../stores/appStore'
import { isMarkdownPath, refLabel, shortRev, useCodeStore } from '../stores/codeStore'
import { useFileViewStore } from '../stores/fileViewStore'
import { codeHtml, ROW_H, useHighlighter } from './DiffView'
import { LfsBadge } from './LfsBadge'
import { MarkdownDoc } from './MarkdownDoc'
import { IconArrowLeft, IconBlame, IconBranch, IconCheck, IconChevronDown, IconChevronRight, IconCode, IconFile, IconFolder, IconHistory, IconRepo, IconSubmodule, IconTag } from './icons'
import './DiffView.css'
import './LfsBadge.css'
import './CodeView.css'

const parentOf = (p: string): string => p.slice(0, Math.max(0, p.lastIndexOf('/')))
const baseOf = (p: string): string => p.slice(p.lastIndexOf('/') + 1)

/** 코드 탐색기: 특정 커밋 시점의 파일 트리·파일 내용·README. 중앙 영역을 차지하고 Esc·마우스 뒤로는 그래프로 돌아간다 */
export function CodeView({ hidden = false }: { hidden?: boolean }): React.JSX.Element | null {
  const t = useT()
  const repoId = useCodeStore((s) => s.repoId)
  const repo = useActiveRepo()
  if (!repoId) return null
  const repoName = repo?.name ?? ''
  return (
    <section className={`diff-view code-view${hidden ? ' is-hidden' : ''}`}>
      <header className="diff-head code-head">
        <button className="diff-head__back" title={`${t('diff.back')} (Esc)`} onClick={() => useCodeStore.getState().close()}>
          <IconArrowLeft size={14} />
          <span>{t('diff.back')}</span>
        </button>
        <RevPicker />
        <Breadcrumb repoName={repoName} />
      </header>
      <div className="code-body">
        <TreePanel repoName={repoName} />
        <MainPane repoId={repoId} />
      </div>
    </section>
  )
}

// ── 머리줄: 리비전 선택기 · 경로 ─────────────────────────

interface RevOption {
  value: string
  label: string
  hash: string
  kind: 'head' | 'branch' | 'tag'
}

function RevPicker(): React.JSX.Element {
  const t = useT()
  const refs = useAppStore((s) => s.refs)
  const ref = useCodeStore((s) => s.ref)
  const label = useCodeStore((s) => s.label)
  const rev = useCodeStore((s) => s.rev)
  const [open, setOpen] = useState(false)
  const wrapRef = useRef<HTMLDivElement>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([])

  const options = useMemo<RevOption[]>(() => {
    const out: RevOption[] = [{ value: 'HEAD', label: 'HEAD', hash: refs?.head.hash ?? '', kind: 'head' }]
    for (const b of refs?.local ?? []) out.push({ value: b.fullName, label: b.name, hash: b.hash, kind: 'branch' })
    for (const x of refs?.tags ?? []) if (x.targetsCommit) out.push({ value: x.fullName, label: x.name, hash: x.hash, kind: 'tag' })
    return out
  }, [refs])
  const headBranch = refs?.head.branch ?? null

  useEffect(() => {
    if (!open) return
    const i = Math.max(0, options.findIndex((o) => o.value === ref))
    itemRefs.current[i]?.focus()
    itemRefs.current[i]?.scrollIntoView({ block: 'nearest' })
    const onDown = (e: MouseEvent): void => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
    // 열릴 때 한 번만 포커스를 옮긴다
  }, [open])

  const close = (refocus: boolean): void => {
    setOpen(false)
    if (refocus) btnRef.current?.focus()
  }
  const pick = (o: RevOption): void => {
    close(true)
    if (o.value !== ref) useCodeStore.getState().switchRev(o.value, o.label)
  }
  const onMenuKey = (e: React.KeyboardEvent): void => {
    const items = itemRefs.current.filter((x): x is HTMLButtonElement => !!x)
    const i = items.indexOf(document.activeElement as HTMLButtonElement)
    const move = (n: number): void => {
      e.preventDefault()
      items[(n + items.length) % items.length]?.focus()
    }
    if (e.key === 'ArrowDown') move(i + 1)
    else if (e.key === 'ArrowUp') move(i - 1)
    else if (e.key === 'Home') move(0)
    else if (e.key === 'End') move(items.length - 1)
    else if (e.key === 'Escape' || e.key === 'Tab') {
      // Esc 는 목록만 닫는다(window 로 올라가면 코드 화면까지 닫힌다)
      e.stopPropagation()
      close(e.key === 'Escape')
    }
  }

  const short = rev ? shortRev(rev) : ''
  const icon = ref.startsWith('refs/tags/') ? <IconTag size={13} /> : ref === 'HEAD' || ref.startsWith('refs/') ? <IconBranch size={13} /> : <IconCode size={13} />
  const groups: { title: MessageKey | null; kind: RevOption['kind'][] }[] = [
    { title: null, kind: ['head'] },
    { title: 'code.revBranches', kind: ['branch'] },
    { title: 'code.revTags', kind: ['tag'] }
  ]
  let idx = -1

  return (
    <div className="code-rev" ref={wrapRef}>
      <button
        ref={btnRef}
        className="code-rev__btn"
        aria-haspopup="menu"
        aria-expanded={open}
        title={rev ? `${t('code.revPicker')}\n${rev}` : t('code.revPicker')}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown' && !open) {
            e.preventDefault()
            setOpen(true)
          }
        }}
      >
        <span className="code-rev__icon">{icon}</span>
        <span className="code-rev__name">{label}</span>
        {short && short !== label && <span className="code-rev__hash mono">{short}</span>}
        <IconChevronDown size={12} />
      </button>
      {open && (
        <div className="code-rev__pop" role="menu" aria-label={t('code.revPicker')} onKeyDown={onMenuKey}>
          {groups.map((g) => {
            const items = options.filter((o) => g.kind.includes(o.kind))
            if (items.length === 0) return null
            return (
              <div key={g.kind[0]} role="group">
                {g.title && <div className="code-rev__title">{t(g.title)}</div>}
                {items.map((o) => {
                  const i = ++idx
                  const on = o.value === ref
                  return (
                    <button
                      key={o.value}
                      ref={(el) => {
                        itemRefs.current[i] = el
                      }}
                      role="menuitemradio"
                      aria-checked={on}
                      className={`code-rev__item${on ? ' is-on' : ''}`}
                      title={o.value === 'HEAD' ? undefined : refLabel(o.value)}
                      onClick={() => pick(o)}
                    >
                      <span className="code-rev__icon">{o.kind === 'tag' ? <IconTag size={13} /> : <IconBranch size={13} />}</span>
                      <span className="code-rev__label">{o.label}</span>
                      {o.kind === 'head' && headBranch && <span className="code-rev__sub">{headBranch}</span>}
                      {o.hash && <span className="code-rev__hash mono">{shortRev(o.hash)}</span>}
                      <span className="code-rev__check">{on && <IconCheck size={14} />}</span>
                    </button>
                  )
                })}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

function Breadcrumb({ repoName }: { repoName: string }): React.JSX.Element {
  const t = useT()
  const path = useCodeStore((s) => s.path)
  const kind = useCodeStore((s) => s.kind)
  const parts = path ? path.split('/') : []
  const go = (p: string): void => void useCodeStore.getState().goto(p)
  return (
    <nav className="code-crumbs" aria-label={t('code.path')} title={path}>
      <button className="code-crumb" onClick={() => go('')}>
        {repoName || t('code.root')}
      </button>
      {parts.map((p, i) => {
        const full = parts.slice(0, i + 1).join('/')
        const isLastFile = kind === 'file' && i === parts.length - 1
        return (
          <span key={full} className="code-crumbs__seg">
            <span className="code-crumbs__sep">/</span>
            {isLastFile ? (
              <span className="code-crumb code-crumb--current">{p}</span>
            ) : (
              <button className="code-crumb" onClick={() => go(full)}>
                {p}
              </button>
            )}
          </span>
        )
      })}
    </nav>
  )
}

// ── 왼쪽 트리 ───────────────────────────────────────────

type Row = { kind: 'entry'; entry: CodeTreeEntry; depth: number } | { kind: 'note'; depth: number; text: string; error?: boolean }

function TreePanel({ repoName }: { repoName: string }): React.JSX.Element {
  const t = useT()
  const tree = useCodeStore((s) => s.tree)
  const expanded = useCodeStore((s) => s.expanded)
  const treeLoading = useCodeStore((s) => s.treeLoading)
  const treeError = useCodeStore((s) => s.treeError)
  const treeTruncated = useCodeStore((s) => s.treeTruncated)
  const path = useCodeStore((s) => s.path)
  const kind = useCodeStore((s) => s.kind)
  const rev = useCodeStore((s) => s.rev)
  const [cursor, setCursor] = useState<string | null>(null)
  const [focused, setFocused] = useState(false)
  const boxRef = useRef<HTMLDivElement>(null)

  const rows = useMemo(() => {
    const out: Row[] = []
    const walk = (dir: string, depth: number): void => {
      const entries = tree.get(dir)
      if (!entries) {
        const err = treeError.get(dir)
        if (err) out.push({ kind: 'note', depth, text: err, error: true })
        else if (treeLoading.has(dir)) out.push({ kind: 'note', depth, text: t('list.loading') })
        return
      }
      for (const entry of entries) {
        out.push({ kind: 'entry', entry, depth })
        if (entry.kind === 'dir' && expanded.has(entry.path)) walk(entry.path, depth + 1)
      }
      if (treeTruncated.has(dir)) out.push({ kind: 'note', depth, text: t('code.truncated', { n: entries.length }) })
    }
    walk('', 0)
    return out
  }, [tree, expanded, treeLoading, treeError, treeTruncated, t])

  // 선택 경로가 바뀌면(링크·브레드크럼으로 이동해도) 커서도 그 자리로
  useEffect(() => setCursor(path || null), [path])
  const cursorPath = cursor ?? path
  const cursorIdx = rows.findIndex((r) => r.kind === 'entry' && r.entry.path === cursorPath)
  useEffect(() => {
    if (cursorIdx >= 0) document.getElementById(`code-tree-${cursorIdx}`)?.scrollIntoView({ block: 'nearest' })
  }, [cursorIdx, rev])

  const store = useCodeStore.getState
  const openEntry = (e: CodeTreeEntry): void => {
    const s = store()
    if (e.kind === 'submodule') return
    if (e.kind === 'dir') {
      // 이미 보고 있는 펼쳐진 폴더를 다시 누르면 접는다
      if (s.kind === 'dir' && s.path === e.path && s.expanded.has(e.path)) s.collapseDir(e.path)
      else void s.selectDir(e.path)
    } else void s.selectFile(e.path)
  }

  const onKeyDown = (ev: React.KeyboardEvent): void => {
    if (ev.altKey || ev.ctrlKey || ev.metaKey) return
    const entryRows = rows.map((r, i) => ({ r, i })).filter((x): x is { r: Extract<Row, { kind: 'entry' }>; i: number } => x.r.kind === 'entry')
    if (entryRows.length === 0) return
    const pos = Math.max(0, entryRows.findIndex((x) => x.i === cursorIdx))
    const cur = entryRows[pos]!
    const move = (n: number): void => {
      ev.preventDefault()
      setCursor(entryRows[Math.max(0, Math.min(entryRows.length - 1, n))]!.r.entry.path)
    }
    const s = store()
    switch (ev.key) {
      case 'ArrowDown':
        return move(pos + 1)
      case 'ArrowUp':
        return move(pos - 1)
      case 'Home':
        return move(0)
      case 'End':
        return move(entryRows.length - 1)
      case 'ArrowRight': {
        ev.preventDefault()
        if (cur.r.entry.kind !== 'dir') return
        if (!s.expanded.has(cur.r.entry.path)) s.expandDir(cur.r.entry.path)
        else if (entryRows[pos + 1] && entryRows[pos + 1]!.r.depth > cur.r.depth) setCursor(entryRows[pos + 1]!.r.entry.path)
        return
      }
      case 'ArrowLeft': {
        ev.preventDefault()
        if (cur.r.entry.kind === 'dir' && s.expanded.has(cur.r.entry.path)) return s.collapseDir(cur.r.entry.path)
        for (let k = pos - 1; k >= 0; k--) {
          if (entryRows[k]!.r.depth < cur.r.depth) return setCursor(entryRows[k]!.r.entry.path)
        }
        return
      }
      case 'Enter':
      case ' ':
        ev.preventDefault()
        return openEntry(cur.r.entry)
    }
  }

  return (
    <aside className="code-tree">
      <button className={`ct-root${path === '' ? ' is-active' : ''}`} title={t('code.root')} onClick={() => void store().goto('')}>
        <IconRepo size={14} />
        <span className="ct-name">{repoName || t('code.root')}</span>
      </button>
      <div
        ref={boxRef}
        className={`ct-scroll${focused ? ' is-focused' : ''}`}
        role="tree"
        aria-label={t('code.tree')}
        aria-activedescendant={cursorIdx >= 0 ? `code-tree-${cursorIdx}` : undefined}
        tabIndex={0}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onKeyDown={onKeyDown}
      >
        {rows.map((r, i) => {
          const pad = 6 + r.depth * 14
          if (r.kind === 'note') {
            return (
              <div key={`n${i}`} className={`ct-note${r.error ? ' is-error' : ''}`} style={{ paddingLeft: pad + 34 }}>
                {r.text}
              </div>
            )
          }
          const e = r.entry
          const isDir = e.kind === 'dir'
          const isOpen = isDir && expanded.has(e.path)
          const active = e.path === path && ((isDir && kind === 'dir') || (!isDir && kind === 'file'))
          const disabled = e.kind === 'submodule'
          return (
            <div
              key={e.path}
              id={`code-tree-${i}`}
              role="treeitem"
              aria-level={r.depth + 1}
              aria-expanded={isDir ? isOpen : undefined}
              aria-selected={active}
              aria-disabled={disabled || undefined}
              className={`ct-row${active ? ' is-active' : ''}${focused && i === cursorIdx ? ' is-cursor' : ''}${disabled ? ' is-disabled' : ''}`}
              style={{ paddingLeft: pad }}
              title={disabled ? t('code.submodule') : e.kind === 'symlink' ? `${e.path}\n${t('code.symlink')}` : e.path}
              onClick={() => {
                setCursor(e.path)
                openEntry(e)
              }}
            >
              <span
                className={`ct-chev${isOpen ? ' is-open' : ''}`}
                onClick={(ev) => {
                  if (!isDir) return
                  ev.stopPropagation()
                  setCursor(e.path)
                  store().toggleDir(e.path)
                }}
              >
                {isDir && <IconChevronRight size={12} />}
              </span>
              <EntryIcon kind={e.kind} />
              <span className="ct-name">{e.name}</span>
              {e.kind === 'symlink' && <span className="ct-tag">{t('code.symlinkShort')}</span>}
            </div>
          )
        })}
        {rows.length === 0 && rev && <div className="ct-note" style={{ paddingLeft: 12 }}>{t('code.emptyDir')}</div>}
      </div>
    </aside>
  )
}

function EntryIcon({ kind }: { kind: CodeTreeEntry['kind'] }): React.JSX.Element {
  return (
    <span className={`ct-icon ct-icon--${kind}`}>
      {kind === 'dir' ? <IconFolder size={14} /> : kind === 'submodule' ? <IconSubmodule size={14} /> : <IconFile size={14} />}
    </span>
  )
}

// ── 오른쪽 내용 ─────────────────────────────────────────

function MainPane({ repoId }: { repoId: string }): React.JSX.Element {
  const t = useT()
  const rev = useCodeStore((s) => s.rev)
  const kind = useCodeStore((s) => s.kind)
  const resolveError = useCodeStore((s) => s.resolveError)
  const missingPath = useCodeStore((s) => s.missingPath)
  return (
    <div className="code-main">
      {missingPath && <div className="code-note">{t('code.pathMissing', { path: missingPath })}</div>}
      {resolveError ? (
        <div className="diff-notice is-error">{t('error.generic', { msg: resolveError })}</div>
      ) : !rev ? (
        <div className="diff-notice">{t('list.loading')}</div>
      ) : kind === 'dir' ? (
        <FolderPane repoId={repoId} rev={rev} />
      ) : (
        <FilePane repoId={repoId} rev={rev} />
      )}
    </div>
  )
}

function FolderPane({ repoId, rev }: { repoId: string; rev: string }): React.JSX.Element {
  const t = useT()
  const path = useCodeStore((s) => s.path)
  const entries = useCodeStore((s) => s.tree.get(s.path))
  const loading = useCodeStore((s) => s.treeLoading.has(s.path))
  const error = useCodeStore((s) => s.treeError.get(s.path))
  const truncated = useCodeStore((s) => s.treeTruncated.has(s.path))
  const readme = useCodeStore((s) => s.readme)
  const go = (p: string): void => void useCodeStore.getState().goto(p)

  return (
    <div className="code-scroll">
      <div className="cf-box">
        {error ? (
          <div className="diff-notice is-error">{t('error.generic', { msg: error })}</div>
        ) : !entries ? (
          <div className="diff-notice">{loading ? t('list.loading') : ''}</div>
        ) : (
          <div className="cf-table" role="list">
            {path && (
              <button className="cf-row" role="listitem" onClick={() => go(parentOf(path))}>
                <span className="ct-icon ct-icon--dir">
                  <IconFolder size={15} />
                </span>
                <span className="cf-name">..</span>
              </button>
            )}
            {entries.map((e) => (
              <button
                key={e.path}
                className={`cf-row${e.kind === 'submodule' ? ' is-disabled' : ''}`}
                role="listitem"
                disabled={e.kind === 'submodule'}
                title={e.kind === 'submodule' ? t('code.submodule') : e.path}
                onClick={() => go(e.path)}
              >
                <span className={`ct-icon ct-icon--${e.kind}`}>
                  {e.kind === 'dir' ? <IconFolder size={15} /> : e.kind === 'submodule' ? <IconSubmodule size={15} /> : <IconFile size={15} />}
                </span>
                <span className="cf-name">{e.name}</span>
                {e.kind === 'symlink' && <span className="ct-tag">{t('code.symlink')}</span>}
                {e.kind === 'submodule' && <span className="ct-tag">{t('code.submoduleShort')}</span>}
              </button>
            ))}
            {entries.length === 0 && <div className="diff-notice">{t('code.emptyDir')}</div>}
            {truncated && <div className="diff-notice">{t('code.truncated', { n: entries.length })}</div>}
          </div>
        )}
      </div>
      {readme && (
        <section className="cv-box">
          <header className="cv-box__head">
            <IconFile size={14} />
            <button className="cv-box__name" onClick={() => go(readme.path)}>
              {readme.name}
            </button>
          </header>
          <div className="cv-box__body">
            {readme.loading ? (
              <div className="cv-muted">{t('list.loading')}</div>
            ) : readme.error ? (
              <div className="cv-muted is-error">{t('error.generic', { msg: readme.error })}</div>
            ) : readme.result ? (
              <BlobDoc repoId={repoId} rev={rev} blob={readme.result} path={readme.path} />
            ) : null}
          </div>
        </section>
      )}
    </div>
  )
}

/** README 상자 안: 마크다운이면 렌더, 확장자 없는 README 는 글자 그대로, 읽을 수 없으면 한 줄 안내 */
function BlobDoc({ repoId, rev, blob, path }: { repoId: string; rev: string; blob: CodeBlobResult; path: string }): React.JSX.Element {
  const t = useT()
  if (blob.kind !== 'text') return <div className="cv-muted">{blobNotice(t, blob)}</div>
  const text = blob.text ?? ''
  return isMarkdownPath(path) ? <MarkdownDoc source={text} dir={parentOf(path)} repoId={repoId} rev={rev} /> : <pre className="cv-plain selectable">{text}</pre>
}

function blobNotice(t: ReturnType<typeof useT>, b: CodeBlobResult): string {
  switch (b.kind) {
    case 'binary':
      return t('code.binary')
    case 'tooLarge':
      return t('code.tooLarge', { size: formatBytes(b.size) })
    case 'lfs':
      return t('code.lfsNote')
    case 'missing':
      return t('code.missing')
    default:
      return ''
  }
}

// ── 파일 ────────────────────────────────────────────────

function splitLines(text: string): string[] {
  if (text === '') return []
  const ls = text.split('\n')
  if (ls[ls.length - 1] === '') ls.pop()
  return ls.map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l))
}

function FilePane({ repoId, rev }: { repoId: string; rev: string }): React.JSX.Element {
  const t = useT()
  const path = useCodeStore((s) => s.path)
  const blob = useCodeStore((s) => s.blob)
  const loading = useCodeStore((s) => s.blobLoading)
  const error = useCodeStore((s) => s.blobError)
  const mdCode = useCodeStore((s) => s.mdCode)
  const entry = useCodeStore((s) => s.tree.get(parentOf(s.path))?.find((e) => e.path === s.path))
  const lines = useMemo(() => (blob?.kind === 'text' ? splitLines(blob.text ?? '') : []), [blob])
  const isMd = isMarkdownPath(path)
  const preview = isMd && blob?.kind === 'text' && !mdCode

  return (
    <div className="cv-file">
      <div className="cv-fhead">
        <IconFile size={14} />
        <strong className="cv-fhead__name" title={path}>
          {baseOf(path)}
        </strong>
        {blob && blob.kind !== 'missing' && (
          <span className="cv-fhead__meta">
            {formatBytes(blob.size)}
            {blob.kind === 'text' && ` · ${t('code.lines', { n: lines.length.toLocaleString() })}`}
          </span>
        )}
        {blob?.kind === 'lfs' && <LfsBadge />}
        {entry?.kind === 'symlink' && <span className="ct-tag">{t('code.symlink')}</span>}
        <span className="cv-fhead__spacer" />
        {isMd && blob?.kind === 'text' && (
          <div className="seg" role="radiogroup" aria-label={t('code.mdMode')}>
            {([false, true] as const).map((code) => (
              <button
                key={String(code)}
                role="radio"
                aria-checked={mdCode === code}
                className={`seg__btn${mdCode === code ? ' is-on' : ''}`}
                onClick={() => useCodeStore.getState().setMdCode(code)}
              >
                {code ? t('code.source') : t('code.preview')}
              </button>
            ))}
          </div>
        )}
        <button className="btn btn--sm" onClick={() => useFileViewStore.getState().open('history', repoId, path, rev)}>
          <IconHistory size={13} /> {t('fileView.openHistory')}
        </button>
        <button className="btn btn--sm" onClick={() => useFileViewStore.getState().open('blame', repoId, path, rev)}>
          <IconBlame size={13} /> {t('fileView.openBlame')}
        </button>
      </div>
      {error ? (
        <div className="diff-notice is-error">{t('error.generic', { msg: error })}</div>
      ) : loading || !blob ? (
        <div className="diff-notice">{loading ? t('list.loading') : ''}</div>
      ) : blob.kind === 'text' ? (
        preview ? (
          <div className="code-scroll">
            <section className="cv-box cv-box--doc">
              <div className="cv-box__body">
                <MarkdownDoc source={blob.text ?? ''} dir={parentOf(path)} repoId={repoId} rev={rev} />
              </div>
            </section>
          </div>
        ) : (
          <>
            {blob.invalidUtf8 && <div className="code-note">{t('code.invalidUtf8')}</div>}
            {lines.length === 0 ? <div className="diff-notice">{t('blame.empty')}</div> : <CodeText key={`${rev}:${path}`} lines={lines} path={path} />}
          </>
        )
      ) : blob.kind === 'image' ? (
        <div className="cv-image">
          <img src={blob.dataUrl} alt={baseOf(path)} />
        </div>
      ) : (
        <NoticeCard blob={blob} />
      )}
    </div>
  )
}

function NoticeCard({ blob }: { blob: CodeBlobResult }): React.JSX.Element {
  const t = useT()
  return (
    <div className="cv-center">
      {blob.kind === 'lfs' && blob.lfs ? (
        <div className="lfs-card" role="note">
          <div className="lfs-card__head">
            <LfsBadge />
            <strong>{t('lfs.cardTitle')}</strong>
          </div>
          <dl className="lfs-card__grid">
            <dt>{t('lfs.size')}</dt>
            <dd>{formatBytes(blob.lfs.size)}</dd>
            <dt>oid</dt>
            <dd className="mono selectable" title={`sha256:${blob.lfs.oid}`}>
              sha256:{blob.lfs.oid}
            </dd>
          </dl>
          <p className="lfs-card__note">{t('lfs.note')}</p>
        </div>
      ) : (
        <div className="cv-card" role="note">
          <IconFile size={28} />
          <p>{blobNotice(t, blob)}</p>
          {blob.kind !== 'missing' && <span className="cv-card__sub">{formatBytes(blob.size)}</span>}
        </div>
      )}
    </div>
  )
}

/** 줄의 표시 폭(문자 수, 동아시아 전각은 2, 탭은 4) — 가로 스크롤 폭 계산용 */
const LONG = 4000
function displayWidth(line: string): number {
  let w = 0
  const s = line.length > LONG ? line.slice(0, LONG + 16) : line
  for (const ch of s) w += ch === '\t' ? 4 : /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]/.test(ch) ? 2 : 1
  return w
}

function CodeText({ lines, path }: { lines: string[]; path: string }): React.JSX.Element {
  const scrollRef = useRef<HTMLDivElement>(null)
  const hl = useHighlighter(path, lines.length)
  const v = useVirtualizer({ count: lines.length, getScrollElement: () => scrollRef.current, estimateSize: () => ROW_H, overscan: 30 })
  const numWidth = String(lines.length).length
  const widest = useMemo(() => {
    let max = 0
    for (const l of lines) if (l.length * 2 > max) max = Math.max(max, displayWidth(l))
    return max
  }, [lines])

  return (
    <div ref={scrollRef} className="diff-scroll cv-code">
      <div style={{ height: v.getTotalSize(), position: 'relative', minWidth: `calc(${widest + 2}ch + ${numWidth + 3}ch)` }}>
        {v.getVirtualItems().map((vi) => (
          <div key={vi.key} className="diff-slot" style={{ transform: `translateY(${vi.start}px)`, height: ROW_H }}>
            <div className="cv-row" style={{ gridTemplateColumns: `${numWidth + 3}ch 1fr` }}>
              <span className="diff-no">{vi.index + 1}</span>
              <span className="diff-code" dangerouslySetInnerHTML={{ __html: codeHtml(lines[vi.index]!, undefined, hl) }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
