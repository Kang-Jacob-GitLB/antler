import { useMemo, useRef } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useT } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { useDetailStore } from '../stores/detailStore'
import { useWipStore, wipEntries, type WipEntry } from '../stores/wipStore'
import { FileRow } from './FileRow'
import './DetailPanel.css'

const FILE_ROW_H = 26
const HEAD_ROW_H = 32
const NOTE_ROW_H = 28

/** 패널의 한 줄: 영역 제목 · 파일 · 안내 */
type PanelRow =
  | { kind: 'head'; key: string; area: 'unstaged' | 'staged'; n: number }
  | { kind: 'entry'; key: string; entry: WipEntry; entryIndex: number }
  | { kind: 'note'; key: string; n: number }

/**
 * 작업 트리(WIP) 변경 패널. GitKraken 의 오른쪽 "Changes" 패널처럼 스테이지 안 된 파일(추적 안 되는 파일 포함)과
 * 스테이지된 파일을 나눠 보여 준다. 읽기 전용이라 스테이지·폐기 버튼은 없고, 파일을 누르면 중앙에 diff 가 열린다.
 */
export function WipPanel({ width }: { width: number }): React.JSX.Element {
  const t = useT()
  const status = useWipStore((s) => s.status)
  const error = useWipStore((s) => s.error)
  const branch = useAppStore((s) => s.head?.branch ?? null)
  const activeFile = useDetailStore((s) => s.file)
  const activeArea = useDetailStore((s) => s.wipArea)
  const openWipFile = useDetailStore((s) => s.openWipFile)
  const scrollRef = useRef<HTMLDivElement>(null)

  const entries = useMemo(() => wipEntries(status), [status])
  // 번역 함수(t)는 렌더마다 새로 만들어지므로 rows 는 t 에 의존하지 않는다(문구는 렌더 때 붙인다)
  const rows = useMemo<PanelRow[]>(() => {
    if (!status) return []
    const out: PanelRow[] = []
    // 추적 안 되는 파일은 "스테이지 안 된 파일"에 함께 센다(GitKraken 과 같다)
    out.push({ kind: 'head', key: 'h:unstaged', area: 'unstaged', n: status.counts.unstaged + status.counts.untracked })
    let i = 0
    for (const e of entries) {
      if (e.area === 'staged') break
      out.push({ kind: 'entry', key: `${e.area}:${e.file.path}`, entry: e, entryIndex: i++ })
    }
    if (status.untrackedMore > 0) out.push({ kind: 'note', key: 'n:more', n: status.untrackedMore })
    out.push({ kind: 'head', key: 'h:staged', area: 'staged', n: status.counts.staged })
    for (const e of entries.slice(i)) out.push({ kind: 'entry', key: `${e.area}:${e.file.path}`, entry: e, entryIndex: i++ })
    return out
  }, [status, entries])

  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i]?.kind === 'head' ? HEAD_ROW_H : rows[i]?.kind === 'note' ? NOTE_ROW_H : FILE_ROW_H),
    getItemKey: (i) => rows[i]?.key ?? i,
    overscan: 12
  })

  const isActive = (e: WipEntry): boolean => !!activeFile && activeArea === e.area && activeFile.path === e.file.path

  const onKeyDown = (ev: React.KeyboardEvent): void => {
    // 수정 키 조합(Alt+↑↓ 등)은 앱 전역 단축키 몫이다
    if (ev.altKey || ev.ctrlKey || ev.metaKey) return
    if (ev.key !== 'ArrowDown' && ev.key !== 'ArrowUp' && ev.key !== 'Enter') return
    ev.preventDefault()
    if (!entries.length) return
    const cur = entries.findIndex(isActive)
    const next = ev.key === 'Enter' ? Math.max(0, cur) : Math.max(0, Math.min(entries.length - 1, cur + (ev.key === 'ArrowDown' ? 1 : -1)))
    const e = entries[next]
    if (!e) return
    void openWipFile(e.file, e.area)
    const rowIndex = rows.findIndex((r) => r.kind === 'entry' && r.entryIndex === next)
    if (rowIndex >= 0) v.scrollToIndex(rowIndex, { align: 'auto' })
  }

  const total = status ? status.counts.staged + status.counts.unstaged + status.counts.untracked : 0

  return (
    <aside className="detail" style={{ width }}>
      <div className="detail__meta-block">
        <h2 className="detail__subject">{t('wip.title')}</h2>
        <div className="wip-meta">
          <span className="wip-meta__branch">{branch ? t('wip.onBranch', { branch }) : t('wip.detached')}</span>
          {!!status?.conflicted && <span className="wip-meta__conflict">{t('wip.conflicts', { n: status.conflicted })}</span>}
        </div>
        <div className="wip-meta__note">{t('wip.readonly')}</div>
      </div>

      <div className="file-list">
        {error && <div className="file-list__note">{t('error.generic', { msg: error })}</div>}
        <div ref={scrollRef} className="file-list__scroll" tabIndex={0} onKeyDown={onKeyDown}>
          {status && total === 0 && <div className="file-list__empty">{t('wip.empty')}</div>}
          <div style={{ height: v.getTotalSize(), position: 'relative' }}>
            {v.getVirtualItems().map((vi) => {
              const row = rows[vi.index]
              if (!row) return null
              return (
                <div key={vi.key} className="file-list__slot" style={{ transform: `translateY(${vi.start}px)`, height: vi.size }}>
                  {row.kind === 'head' ? (
                    <div className="wip-head">{t(row.area === 'staged' ? 'wip.staged' : 'wip.unstaged', { n: row.n.toLocaleString() })}</div>
                  ) : row.kind === 'note' ? (
                    <div className="wip-note">{t('wip.untrackedMore', { n: row.n.toLocaleString() })}</div>
                  ) : (
                    <FileRow
                      file={row.entry.file}
                      active={isActive(row.entry)}
                      {...(row.entry.area === 'untracked' ? { tag: t('wip.untracked') } : {})}
                      onOpen={(f) => void openWipFile(f, row.entry.area)}
                    />
                  )}
                </div>
              )
            })}
          </div>
        </div>
      </div>
    </aside>
  )
}
