import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { HLJSApi } from 'highlight.js'
import type { DiffLine, FileDiff } from '@shared/types'
import { splitRows, unifiedRows, type Span, type SplitCell, type SplitRow, type UnifiedRow } from '../diff/diffRows'
import { escapeHtml, highlightLine, languageFor, loadHighlighter } from '../diff/highlight'
import { useT } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { useDetailStore } from '../stores/detailStore'
import { fileViewAvailability, openFileView } from '../stores/fileViewStore'
import { useWipStore, wipEntries } from '../stores/wipStore'
import { FileStatusIcon } from './FileStatusIcon'
import { IconArrowLeft, IconBlame, IconChevronDown, IconHistory } from './icons'
import './DiffView.css'

export const ROW_H = 20
/** 이보다 긴 diff 는 강조 없이 그린다(줄 단위 강조라도 수만 줄이면 스크롤이 무거워진다) */
const HIGHLIGHT_MAX_LINES = 20_000

export interface Hl {
  hljs: HLJSApi
  lang: string
}

/**
 * 이보다 긴 줄(번들·minified 파일 등)은 강조하지 않고 앞부분만 그린다.
 * 수 MB 짜리 한 줄을 렌더 중에 동기로 강조하면 UI 가 몇 초씩 멈춘다.
 */
const LONG_LINE = 4000
const TOOLTIP_MAX = 500

/** 줄 HTML: 언어 강조 + 줄 안 바뀐 구간 <mark>. highlight.js 와 escapeHtml 모두 이스케이프한 결과만 넣는다 */
export function codeHtml(text: string, span: Span | undefined, hl: Hl | null): string {
  if (text.length > LONG_LINE) {
    return `${escapeHtml(text.slice(0, LONG_LINE))}<span class="diff-long">… (+${(text.length - LONG_LINE).toLocaleString()})</span>`
  }
  const render = (s: string): string => (s ? (hl ? highlightLine(hl.hljs, hl.lang, s) : escapeHtml(s)) : '')
  if (!span || span[0] === span[1]) return render(text)
  return `${render(text.slice(0, span[0]))}<mark class="diff-emph">${render(text.slice(span[0], span[1]))}</mark>${render(text.slice(span[1]))}`
}

const lineClass = (l: DiffLine): string => (l.t === '+' ? 'is-add' : l.t === '-' ? 'is-del' : l.t === '\\' ? 'is-note' : '')

/** 잘릴 수 있는 split 보기에서는 긴 줄 전체를 툴팁으로 보여 준다 */
const TOOLTIP_MIN = 80

const Code = ({ line, span, hl, clip }: { line: DiffLine; span?: Span; hl: Hl | null; clip?: boolean }): React.JSX.Element =>
  line.t === '\\' ? (
    <span className="diff-code diff-code--note">{line.text.trim()}</span>
  ) : (
    <span
      className="diff-code"
      title={
        clip && line.text.length > TOOLTIP_MIN
          ? line.text.length > TOOLTIP_MAX
            ? `${line.text.slice(0, TOOLTIP_MAX)}…`
            : line.text
          : undefined
      }
      dangerouslySetInnerHTML={{ __html: codeHtml(line.text, span, hl) }}
    />
  )

const UnifiedRowView = memo(function UnifiedRowView({ row, hl }: { row: UnifiedRow; hl: Hl | null }) {
  if (row.kind === 'hunk') return <HunkHeader row={row} />
  const l = row.line
  return (
    <div className={`diff-row diff-row--unified ${lineClass(l)}`}>
      <span className="diff-no">{l.o ?? ''}</span>
      <span className="diff-no">{l.n ?? ''}</span>
      <span className="diff-sign">{l.t === '\\' ? '' : l.t}</span>
      <Code line={l} span={row.span} hl={hl} />
    </div>
  )
})

function SplitSide({ cell, side, hl }: { cell?: SplitCell; side: 'o' | 'n'; hl: Hl | null }): React.JSX.Element {
  if (!cell) return <div className="diff-side is-empty" />
  const l = cell.line
  return (
    <div className={`diff-side ${lineClass(l)}`}>
      <span className="diff-no">{l[side] ?? ''}</span>
      <span className="diff-sign">{l.t === '\\' || l.t === ' ' ? '' : l.t}</span>
      <Code line={l} span={cell.span} hl={hl} clip />
    </div>
  )
}

const SplitRowView = memo(function SplitRowView({ row, hl }: { row: SplitRow; hl: Hl | null }) {
  if (row.kind === 'hunk') return <HunkHeader row={row} />
  return (
    <div className="diff-row diff-row--split">
      <SplitSide cell={row.left} side="o" hl={hl} />
      <SplitSide cell={row.right} side="n" hl={hl} />
    </div>
  )
})

function HunkHeader({ row }: { row: Extract<UnifiedRow, { kind: 'hunk' }> }): React.JSX.Element {
  const h = row.hunk
  return (
    <div className="diff-row diff-row--hunk">
      <span className="diff-hunk__range mono">
        @@ −{h.oldStart},{h.oldLines} +{h.newStart},{h.newLines} @@
      </span>
      {h.header && <span className="diff-hunk__ctx">{h.header}</span>}
    </div>
  )
}

/** 가장 긴 줄의 표시 폭(문자 수, 동아시아 전각 문자는 2) — 가로 스크롤 폭 계산용 */
function widestLine(diff: FileDiff): number {
  let max = 0
  for (const h of diff.hunks) {
    for (const l of h.lines) {
      if (l.text.length * 2 <= max) continue
      // 긴 줄은 앞 LONG_LINE 자만 그리므로 폭도 그만큼(+ 생략 표시)만 잡는다
      const text = l.text.length > LONG_LINE ? l.text.slice(0, LONG_LINE + 16) : l.text
      let w = 0
      for (const ch of text) w += ch === '\t' ? 4 : /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦]/.test(ch) ? 2 : 1
      if (w > max) max = w
    }
  }
  return max
}

export function useHighlighter(path: string | undefined, lines: number): Hl | null {
  const [hl, setHl] = useState<Hl | null>(null)
  useEffect(() => {
    setHl(null)
    const lang = path ? languageFor(path) : null
    if (!lang || lines > HIGHLIGHT_MAX_LINES) return
    let alive = true
    void loadHighlighter(lang).then((hljs) => {
      if (alive && hljs) setHl({ hljs, lang })
    })
    return () => {
      alive = false
    }
  }, [path, lines])
  return hl
}

/** unified / split 전환(설정에 저장) */
export function DiffModeToggle(): React.JSX.Element {
  const t = useT()
  const mode = useAppStore((s) => s.config?.ui.diffMode ?? 'unified')
  const setUi = useAppStore((s) => s.setUi)
  return (
    <div className="seg" role="radiogroup" aria-label={t('diff.mode')}>
      {(['unified', 'split'] as const).map((m) => (
        <button key={m} role="radio" aria-checked={mode === m} className={`seg__btn${mode === m ? ' is-on' : ''}`} onClick={() => void setUi({ diffMode: m })}>
          {t(`diff.${m}`)}
        </button>
      ))}
    </div>
  )
}

interface DiffBodyProps {
  /** 표시할 diff(아직 없으면 null) */
  diff: FileDiff | null
  loading?: boolean
  error?: string | null
  /** 바뀌면 맨 위로 스크롤한다(보통 파일 경로) */
  resetKey?: unknown
}

/** diff 본문: 가상 스크롤 행 + 지연 로드 syntax highlight. 커밋 diff 와 PR 파일 diff 가 함께 쓴다 */
export function DiffBody({ diff, loading = false, error = null, resetKey }: DiffBodyProps): React.JSX.Element {
  const t = useT()
  const mode = useAppStore((s) => s.config?.ui.diffMode ?? 'unified')
  const scrollRef = useRef<HTMLDivElement>(null)
  const lineCount = useMemo(() => diff?.hunks.reduce((n, h) => n + h.lines.length, 0) ?? 0, [diff])
  const rows = useMemo<(UnifiedRow | SplitRow)[]>(
    () => (diff ? (mode === 'split' ? splitRows(diff.hunks) : unifiedRows(diff.hunks)) : []),
    [diff, mode]
  )
  const widest = useMemo(() => (diff ? widestLine(diff) : 0), [diff])
  const hl = useHighlighter(diff?.path, lineCount)
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 30
  })

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0, left: 0 })
  }, [resetKey])

  // unified 는 가장 긴 줄까지 가로 스크롤. split 은 두 열이 항상 화면에 함께 보이도록 폭을 맞추고 긴 줄은 자른다
  const minWidth = mode === 'split' ? undefined : `calc(${widest + 2}ch + 120px)`

  return (
    <div ref={scrollRef} className={`diff-scroll diff-scroll--${mode}`}>
      {error ? (
        <div className="diff-notice is-error">{t('error.generic', { msg: error })}</div>
      ) : !diff ? (
        <div className="diff-notice">{loading ? t('list.loading') : ''}</div>
      ) : diff.binary ? (
        <div className="diff-notice">{t('diff.binary')}</div>
      ) : rows.length === 0 ? (
        <div className="diff-notice">{t('diff.noContent')}</div>
      ) : (
        <div style={{ height: v.getTotalSize(), position: 'relative', minWidth }}>
          {v.getVirtualItems().map((vi) => {
            const row = rows[vi.index]!
            return (
              <div key={vi.key} className="diff-slot" style={{ transform: `translateY(${vi.start}px)`, height: ROW_H }}>
                {mode === 'split' ? <SplitRowView row={row as SplitRow} hl={hl} /> : <UnifiedRowView row={row as UnifiedRow} hl={hl} />}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

/** hidden: 파일 히스토리·Blame 이 위에 떠 있는 동안 숨겨 둔다(언마운트하면 스크롤 위치가 사라진다) */
export function DiffView({ hidden = false }: { hidden?: boolean }): React.JSX.Element | null {
  const t = useT()
  const { file, wipArea, diff, diffLoading, diffError, detail, diffFull, fullError, repoId } = useDetailStore()
  const { closeFile, stepFile, openFile, openWipFile } = useDetailStore.getState()
  const wipStatus = useWipStore((s) => s.status)

  const current = diff && file && diff.path === file.path ? diff : null
  const lineCount = useMemo(() => current?.hunks.reduce((n, h) => n + h.lines.length, 0) ?? 0, [current])

  if (!file || (!detail && !wipArea)) return null
  // 작업 트리(WIP) 파일이면 패널에 보이는 순서의 목록에서, 커밋 파일이면 커밋의 파일 목록에서 위치를 센다
  const wipList = wipArea ? wipEntries(wipStatus) : null
  const total = wipList ? wipList.length : detail!.files.length
  const index = wipList ? wipList.findIndex((e) => e.area === wipArea && e.file.path === file.path) : detail!.files.indexOf(file)
  const slash = file.path.lastIndexOf('/')
  const dir = slash < 0 ? '' : file.path.slice(0, slash + 1)
  const base = file.path.slice(slash + 1)
  const view = fileViewAvailability(file, wipArea)
  // 커밋의 파일이면 그 커밋 시점, 작업 트리 파일이면 HEAD 기준
  const viewRev = wipArea ? null : detail!.hash

  return (
    <section className={`diff-view${hidden ? ' is-hidden' : ''}`}>
      <header className="diff-head">
        <button className="diff-head__back" title={`${t('diff.back')} (Esc)`} onClick={closeFile}>
          <IconArrowLeft size={14} />
          <span>{t('diff.back')}</span>
        </button>
        <FileStatusIcon status={file.status} />
        <div className="diff-head__path" title={file.path}>
          {file.oldPath && <span className="diff-head__from">{file.oldPath} →</span>}
          <span className="diff-head__dir">{dir}</span>
          <span className="diff-head__base">{base}</span>
        </div>
        {wipArea && <span className="diff-head__area">{t(`wip.area.${wipArea}`)}</span>}
        {!file.binary && !wipArea && (
          <span className="diff-head__stat">
            <span className="is-add">+{file.additions ?? 0}</span>
            <span className="is-del">−{file.deletions ?? 0}</span>
          </span>
        )}
        {repoId && view.history && (
          <button className="icon-btn icon-btn--sm" title={t('fileView.openHistory')} onClick={() => openFileView('history', repoId, file, viewRev)}>
            <IconHistory size={14} />
          </button>
        )}
        {repoId && view.blame && (
          <button className="icon-btn icon-btn--sm" title={t('fileView.openBlame')} onClick={() => openFileView('blame', repoId, file, viewRev)}>
            <IconBlame size={14} />
          </button>
        )}
        <DiffModeToggle />
        <span className="diff-head__pos">
          {index + 1} / {total}
        </span>
        <button className="icon-btn icon-btn--sm" title={`${t('diff.prevFile')} (Alt+↑)`} disabled={index <= 0} onClick={() => stepFile(-1)}>
          <IconChevronDown size={14} style={{ transform: 'rotate(180deg)' }} />
        </button>
        <button
          className="icon-btn icon-btn--sm"
          title={`${t('diff.nextFile')} (Alt+↓)`}
          disabled={index >= total - 1}
          onClick={() => stepFile(1)}
        >
          <IconChevronDown size={14} />
        </button>
      </header>

      {current && current.meta.length > 0 && <div className="diff-meta">{current.meta.join(' · ')}</div>}

      <DiffBody diff={current} loading={diffLoading} error={diffError} resetKey={file} />

      {current?.truncated && (
        <footer className="diff-trunc">
          <span>
            {t('diff.truncated', {
              shown: lineCount.toLocaleString(),
              total: `${current.totalLines.toLocaleString()}${current.totalIsLowerBound ? '+' : ''}`
            })}
            {fullError && <span className="diff-trunc__err"> — {t('error.generic', { msg: fullError })}</span>}
          </span>
          {/* 전체 요청으로 받았는데도 잘렸으면 더 받을 수 없다(크기 상한) — 같은 요청을 반복하지 않는다 */}
          {diffFull ? (
            <span className="diff-trunc__max">{t('diff.tooLarge')}</span>
          ) : (
            <button className="btn" disabled={diffLoading} onClick={() => void (wipArea ? openWipFile(file, wipArea, true) : openFile(file, true))}>
              {diffLoading ? t('list.loading') : t('diff.loadFull')}
            </button>
          )}
        </footer>
      )}
    </section>
  )
}
