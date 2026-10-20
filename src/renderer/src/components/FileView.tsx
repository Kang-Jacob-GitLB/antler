import { memo, useEffect, useMemo, useRef } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { BlameCommit, BlameResult, FileHistoryEntry, FileStatus } from '@shared/types'
import { useT } from '../i18n'
import { DateText } from './DateText'
import { useAppStore } from '../stores/appStore'
import { useFileViewStore, type FileViewTab } from '../stores/fileViewStore'
import { useLogStore } from '../stores/logStore'
import { codeHtml, DiffBody, DiffModeToggle, ROW_H, useHighlighter } from './DiffView'
import { FileStatusIcon } from './FileStatusIcon'
import { splitPath } from './FileRow'
import { IconArrowLeft, IconBlame, IconHistory } from './icons'
import './DiffView.css'
import './FileView.css'

const ENTRY_H = 50
/** 목록 끝에서 이 개수 안으로 스크롤하면 다음 페이지를 받는다 */
const LOAD_AHEAD = 12
/** blame 줄 폭 계산에서 한 줄에 쓰는 최대 문자 수(아주 긴 줄이 가로 스크롤을 끝없이 늘리지 않게) */
const BLAME_LONG_LINE = 4000

const shortHash = (h: string): string => h.slice(0, 7)

/** 파일 히스토리·Blame 화면. 중앙 영역을 DiffView 처럼 차지하고, 뒤로가기(버튼·Esc·마우스)는 한 단계씩 물러난다 */
export function FileView(): React.JSX.Element | null {
  const t = useT()
  const tab = useFileViewStore((s) => s.tab)
  const path = useFileViewStore((s) => s.path)
  const { setTab, close } = useFileViewStore.getState()
  if (!tab) return null
  const [dir, base] = splitPath(path)

  return (
    <section className="diff-view file-view">
      <header className="diff-head">
        <button className="diff-head__back" title={`${t('diff.back')} (Esc)`} onClick={close}>
          <IconArrowLeft size={14} />
          <span>{t('diff.back')}</span>
        </button>
        <div className="seg" role="radiogroup" aria-label={t('fileView.tabs')}>
          {(['history', 'blame'] as const satisfies readonly FileViewTab[]).map((k) => (
            <button key={k} role="radio" aria-checked={tab === k} className={`seg__btn${tab === k ? ' is-on' : ''}`} onClick={() => setTab(k)}>
              {k === 'history' ? <IconHistory size={13} /> : <IconBlame size={13} />}
              <span>{t(`fileView.${k}`)}</span>
            </button>
          ))}
        </div>
        <div className="diff-head__path" title={path}>
          <span className="diff-head__dir">{dir}</span>
          <span className="diff-head__base">{base}</span>
        </div>
        {tab === 'history' && <DiffModeToggle />}
      </header>
      {tab === 'history' ? <HistoryPane /> : <BlamePane />}
    </section>
  )
}

// ── 히스토리 ─────────────────────────────────────────────

const asStatus = (s: string): FileStatus => (s === 'A' || s === 'M' || s === 'D' || s === 'R' || s === 'C' || s === 'T' ? s : 'M')

const EntryRow = memo(function EntryRow({ entry, active, locale, onPick }: { entry: FileHistoryEntry; active: boolean; locale: 'ko' | 'en'; onPick(e: FileHistoryEntry): void }) {
  const t = useT()
  return (
    <div
      role="button"
      tabIndex={-1}
      className={`fh-entry${active ? ' is-active' : ''}`}
      title={entry.oldPath ? t('fileView.renamedFrom', { from: entry.oldPath }) : entry.path}
      onClick={() => onPick(entry)}
    >
      <FileStatusIcon status={asStatus(entry.status)} />
      <div className="fh-entry__text">
        <div className="fh-entry__subject">{entry.subject}</div>
        <div className="fh-entry__meta">
          <span className="mono">{shortHash(entry.hash)}</span>
          <span className="fh-entry__author">{entry.authorName}</span>
          <span>
            <DateText epochSec={entry.authorDate} locale={locale} />
          </span>
        </div>
      </div>
    </div>
  )
})

function HistoryPane(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { entries, hasMore, historyLoading, historyError, selected, diff, diffLoading, diffError, diffFull } = useFileViewStore()
  const { loadMore, select, blameAt } = useFileViewStore.getState()
  const scrollRef = useRef<HTMLDivElement>(null)
  const v = useVirtualizer({ count: entries.length, getScrollElement: () => scrollRef.current, estimateSize: () => ENTRY_H, overscan: 10 })
  const items = v.getVirtualItems()
  const lastVisible = items.length ? items[items.length - 1]!.index : -1

  // 끝에 가까워지면 다음 페이지(요청 중이거나 오류 뒤에는 자동으로 다시 시도하지 않는다)
  useEffect(() => {
    if (hasMore && !historyLoading && !historyError && lastVisible >= entries.length - LOAD_AHEAD) void loadMore()
  }, [hasMore, historyLoading, historyError, lastVisible, entries.length, loadMore])

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.altKey || e.ctrlKey || e.metaKey) return
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    e.preventDefault()
    const i = selected ? entries.findIndex((x) => x.hash === selected.hash) : -1
    const next = entries[Math.max(0, Math.min(entries.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))]
    if (next) {
      void select(next)
      v.scrollToIndex(entries.indexOf(next), { align: 'auto' })
    }
  }

  const current = diff && selected && diff.path === selected.path ? diff : null
  const lineCount = useMemo(() => current?.hunks.reduce((n, h) => n + h.lines.length, 0) ?? 0, [current])

  return (
    <div className="fh">
      <div className="fh-list">
        <div ref={scrollRef} className="fh-list__scroll" tabIndex={0} onKeyDown={onKeyDown}>
          {entries.length === 0 && !historyLoading && !historyError && <div className="fh-note">{t('fileView.empty')}</div>}
          <div style={{ height: v.getTotalSize(), position: 'relative' }}>
            {items.map((vi) => {
              const e = entries[vi.index]!
              return (
                <div key={vi.key} className="fh-list__slot" style={{ transform: `translateY(${vi.start}px)`, height: ENTRY_H }}>
                  <EntryRow entry={e} active={selected?.hash === e.hash} locale={locale} onPick={(x) => void select(x)} />
                </div>
              )
            })}
          </div>
          {historyLoading && <div className="fh-note">{t('list.loading')}</div>}
          {historyError && (
            <div className="fh-note is-error">
              {t('error.generic', { msg: historyError })}
              <button className="btn" onClick={() => void loadMore()}>
                {t('fileView.loadMore')}
              </button>
            </div>
          )}
        </div>
      </div>

      <div className="fh-detail">
        {selected ? (
          <>
            <div className="fh-detail__head">
              <div className="fh-detail__text">
                <div className="fh-detail__subject selectable">{selected.subject}</div>
                <div className="fh-detail__meta">
                  <span className="mono selectable">{shortHash(selected.hash)}</span>
                  <span className="selectable">{selected.authorName}</span>
                  <span>
                    <DateText epochSec={selected.authorDate} locale={locale} />
                  </span>
                  {selected.oldPath && <span className="fh-detail__rename">{t('fileView.renamedFrom', { from: selected.oldPath })}</span>}
                </div>
              </div>
              {selected.status !== 'D' && (
                <button className="btn" onClick={() => blameAt(selected)}>
                  <IconBlame size={13} /> {t('fileView.blameHere')}
                </button>
              )}
              <button className="btn" onClick={() => void useLogStore.getState().reveal(selected.hash)}>
                {t('fileView.reveal')}
              </button>
            </div>
            {current && current.meta.length > 0 && <div className="diff-meta">{current.meta.join(' · ')}</div>}
            <DiffBody diff={current} loading={diffLoading} error={diffError} resetKey={selected.hash} />
            {current?.truncated && (
              <footer className="diff-trunc">
                <span>
                  {t('diff.truncated', {
                    shown: lineCount.toLocaleString(),
                    total: `${current.totalLines.toLocaleString()}${current.totalIsLowerBound ? '+' : ''}`
                  })}
                </span>
                {diffFull ? (
                  <span className="diff-trunc__max">{t('diff.tooLarge')}</span>
                ) : (
                  <button className="btn" disabled={diffLoading} onClick={() => void select(selected, true)}>
                    {diffLoading ? t('list.loading') : t('diff.loadFull')}
                  </button>
                )}
              </footer>
            )}
          </>
        ) : (
          <div className="diff-notice">{historyLoading ? t('list.loading') : historyError ? '' : entries.length ? t('fileView.pickCommit') : ''}</div>
        )}
      </div>
    </div>
  )
}

// ── Blame ───────────────────────────────────────────────

/** 가상 스크롤 한 줄. first 는 같은 커밋 묶음의 첫 줄(왼쪽에 커밋 정보를 보인다) */
interface BlameRow {
  text: string
  group: number
  first: boolean
}

function blameRows(b: BlameResult): BlameRow[] {
  const rows: BlameRow[] = []
  b.groups.forEach((g, gi) => {
    for (let i = 0; i < g.count; i++) rows.push({ text: b.lines[g.start + i] ?? '', group: gi, first: i === 0 })
  })
  return rows
}

function BlameGutter({ commit, locale }: { commit: BlameCommit; locale: 'ko' | 'en' }): React.JSX.Element {
  const t = useT()
  return (
    <button
      className="bl-gutter"
      title={t('blame.revealTitle', { summary: commit.summary, hash: commit.hash })}
      onClick={() => void useLogStore.getState().reveal(commit.hash)}
    >
      <span className="bl-gutter__hash mono">{shortHash(commit.hash)}</span>
      <span className="bl-gutter__author">{commit.authorName}</span>
      <span className="bl-gutter__date">
        <DateText epochSec={commit.authorDate} locale={locale} />
      </span>
    </button>
  )
}

function formatSize(bytes: number): string {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`
}

function BlamePane(): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const { blame, blameLoading, blameError, blameRev, blamePath, fromWip } = useFileViewStore()
  const scrollRef = useRef<HTMLDivElement>(null)
  const rows = useMemo(() => (blame ? blameRows(blame) : []), [blame])
  const widest = useMemo(() => rows.reduce((m, r) => Math.max(m, Math.min(r.text.length, BLAME_LONG_LINE)), 0), [rows])
  const hl = useHighlighter(blame?.path, rows.length)
  const v = useVirtualizer({ count: rows.length, getScrollElement: () => scrollRef.current, estimateSize: () => ROW_H, overscan: 30 })
  const numWidth = String(rows.length).length

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0, left: 0 })
  }, [blame])

  const notice = blameError
    ? { text: t('error.generic', { msg: blameError }), error: true }
    : !blame
      ? { text: blameLoading ? t('list.loading') : '', error: false }
      : blame.missing
        ? { text: t('blame.missing'), error: false }
        : blame.binary
        ? { text: t('blame.binary'), error: false }
        : blame.tooLarge
          ? { text: t('blame.tooLarge', { size: formatSize(blame.size) }), error: false }
          : rows.length === 0
            ? { text: t('blame.empty'), error: false }
            : null

  return (
    <div className="bl">
      <div className="bl-bar">
        <span className="mono selectable">{blamePath}</span>
        <span>{blameRev ? t('blame.atCommit', { hash: shortHash(blameRev) }) : t('blame.atHead')}</span>
        {fromWip && !blameRev && <span className="bl-bar__note">{t('blame.wipNote')}</span>}
        {blame && blameRev === null && <span className="mono">{shortHash(blame.rev)}</span>}
      </div>
      <div ref={scrollRef} className="diff-scroll bl-scroll">
        {notice ? (
          <div className={`diff-notice${notice.error ? ' is-error' : ''}`}>{notice.text}</div>
        ) : (
          blame && (
            <div style={{ height: v.getTotalSize(), position: 'relative', minWidth: `calc(${widest + 2}ch + 360px + ${numWidth}ch)` }}>
              {v.getVirtualItems().map((vi) => {
                const row = rows[vi.index]!
                const commit = blame.commits[blame.groups[row.group]!.hash]
                return (
                  <div key={vi.key} className="diff-slot" style={{ transform: `translateY(${vi.start}px)`, height: ROW_H }}>
                    <div className={`bl-row${row.group % 2 ? ' is-alt' : ''}${row.first && vi.index > 0 ? ' is-first' : ''}`} style={{ gridTemplateColumns: `260px ${numWidth + 2}ch 1fr` }}>
                      <div className="bl-cell">{row.first && commit ? <BlameGutter commit={commit} locale={locale} /> : null}</div>
                      <span className="diff-no">{vi.index + 1}</span>
                      <span
                        className="diff-code"
                        dangerouslySetInnerHTML={{ __html: codeHtml(row.text, undefined, hl) }}
                      />
                    </div>
                  </div>
                )
              })}
            </div>
          )
        )}
      </div>
    </div>
  )
}
