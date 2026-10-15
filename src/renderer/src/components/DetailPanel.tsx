import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { Commit, CommitDetail, FileChange } from '@shared/types'
import { useT } from '../i18n'
import { formatAbsolute, initials } from '../lib/format'
import { fnv1a, LANE_COUNT, laneVar } from '../lib/laneColor'
import { useAppStore } from '../stores/appStore'
import { useDetailStore } from '../stores/detailStore'
import { useLogStore } from '../stores/logStore'
import { FileStatusIcon } from './FileStatusIcon'
import { IconCopy } from './icons'
import { markdownToHtml } from '../lib/markdown'
import { HtmlBody } from './github/GhBits'
import './DetailPanel.css'

const FILE_ROW_H = 26

/** 커밋 메시지 본문 — 마크다운으로 렌더링한다(정화·링크 처리는 HtmlBody) */
const CommitBody = memo(function CommitBody({ body }: { body: string }) {
  const html = useMemo(() => markdownToHtml(body), [body])
  return <HtmlBody html={html} className="detail__body" />
})

/** 경로를 (디렉터리, 파일 이름)으로 나눈다 */
function splitPath(path: string): [dir: string, base: string] {
  const i = path.lastIndexOf('/')
  return i < 0 ? ['', path] : [path.slice(0, i + 1), path.slice(i + 1)]
}

/** +/- 비율 막대(GitKraken 의 변경량 표시) */
function ChangeBar({ add, del }: { add: number; del: number }): React.JSX.Element {
  const total = add + del
  const cells = 5
  const a = total ? Math.round((add / total) * cells) : 0
  const d = total ? Math.min(cells - a, Math.max(del ? 1 : 0, Math.round((del / total) * cells))) : 0
  return (
    <span className="change-bar" aria-hidden="true">
      {Array.from({ length: cells }, (_, i) => (
        <span key={i} className={i < a ? 'is-add' : i < a + d ? 'is-del' : ''} />
      ))}
    </span>
  )
}

const FileRow = memo(function FileRow({
  file,
  active,
  onOpen
}: {
  file: FileChange
  active: boolean
  onOpen(f: FileChange): void
}) {
  const t = useT()
  const [dir, base] = splitPath(file.path)
  return (
    <div
      role="button"
      tabIndex={-1}
      className={`file-row${active ? ' is-active' : ''}`}
      title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
      onClick={() => onOpen(file)}
    >
      <FileStatusIcon status={file.status} />
      <span className="file-row__name">
        <span className="file-row__base">{base}</span>
        {dir && <span className="file-row__dir">{dir}</span>}
        {file.oldPath && <span className="file-row__from">← {splitPath(file.oldPath)[1]}</span>}
      </span>
      {file.binary ? (
        <span className="file-row__bin">{t('detail.binary')}</span>
      ) : (
        <span className="file-row__stat">
          {!!file.additions && <span className="is-add">+{file.additions}</span>}
          {!!file.deletions && <span className="is-del">−{file.deletions}</span>}
        </span>
      )}
    </div>
  )
})

function FileList({ detail }: { detail: CommitDetail }): React.JSX.Element {
  const t = useT()
  const scrollRef = useRef<HTMLDivElement>(null)
  const active = useDetailStore((s) => s.file)
  const openFile = useDetailStore((s) => s.openFile)
  const v = useVirtualizer({
    count: detail.files.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => FILE_ROW_H,
    overscan: 12
  })

  // 다른 커밋으로 바뀌면 맨 위부터
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [detail.hash])

  const onKeyDown = (e: React.KeyboardEvent): void => {
    // 수정 키 조합(Alt+↑↓ 등)은 앱 전역 단축키 몫이라 여기서 처리하지 않는다 — 두 번 이동하는 것을 막는다
    if (e.altKey || e.ctrlKey || e.metaKey) return
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Enter') return
    e.preventDefault()
    const i = active ? detail.files.indexOf(active) : -1
    const next = e.key === 'Enter' ? Math.max(0, i) : i + (e.key === 'ArrowDown' ? 1 : -1)
    const f = detail.files[Math.max(0, Math.min(detail.files.length - 1, next))]
    if (f) {
      void openFile(f)
      v.scrollToIndex(detail.files.indexOf(f), { align: 'auto' })
    }
  }

  return (
    <div className="file-list">
      <div className="file-list__head">
        <span>{t('detail.files', { n: detail.files.length.toLocaleString() })}</span>
        <span className="file-list__totals">
          <span className="is-add">+{detail.additions.toLocaleString()}</span>
          <span className="is-del">−{detail.deletions.toLocaleString()}</span>
          <ChangeBar add={detail.additions} del={detail.deletions} />
        </span>
      </div>
      {detail.parents.length > 1 && <div className="file-list__note">{t('detail.mergeBase')}</div>}
      {detail.truncated && <div className="file-list__note">{t('detail.filesTruncated')}</div>}
      <div ref={scrollRef} className="file-list__scroll" tabIndex={0} onKeyDown={onKeyDown}>
        {detail.files.length === 0 && <div className="file-list__empty">{t('detail.noFiles')}</div>}
        <div style={{ height: v.getTotalSize(), position: 'relative' }}>
          {v.getVirtualItems().map((vi) => {
            const f = detail.files[vi.index]!
            return (
              <div key={vi.key} className="file-list__slot" style={{ transform: `translateY(${vi.start}px)`, height: FILE_ROW_H }}>
                <FileRow file={f} active={f === active} onOpen={(x) => void openFile(x)} />
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}

/** 선택한 커밋의 메타데이터·메시지·변경 파일. 파일을 누르면 중앙에 diff 가 열린다 */
export function DetailPanel({ width }: { width: number }): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const commit: Commit | undefined = useLogStore((s) => (s.selected >= 0 ? s.commits[s.selected] : undefined))
  const reveal = useLogStore((s) => s.reveal)
  const { detail, loading, error } = useDetailStore()
  const [copied, setCopied] = useState(false)

  useEffect(() => setCopied(false), [commit?.hash])

  if (!commit) {
    return (
      <aside className="detail" style={{ width }}>
        <div className="detail__empty">{t('detail.empty')}</div>
      </aside>
    )
  }

  // 상세가 아직 안 왔으면 목록에 있는 정보로 머리 부분을 먼저 그린다
  const d = detail?.hash === commit.hash ? detail : null
  const copy = async (): Promise<void> => {
    await navigator.clipboard.writeText(commit.hash)
    setCopied(true)
  }

  return (
    <aside className="detail" style={{ width }}>
      <div className="detail__meta-block">
        <div className="detail__sha-row">
          <span className="detail__sha mono selectable">{commit.hash.slice(0, 10)}</span>
          <button className="icon-btn icon-btn--sm" title={t('detail.copy')} onClick={() => void copy()}>
            <IconCopy size={13} />
          </button>
          {copied && <span className="detail__copied">{t('detail.copied')}</span>}
          {commit.parents.length > 0 && (
            <span className="detail__parents">
              <span className="detail__parents-label">{t('detail.parents')}</span>
              {commit.parents.map((p) => (
                <button key={p} className="detail__parent mono" onClick={() => void reveal(p)} title={p}>
                  {p.slice(0, 7)}
                </button>
              ))}
            </span>
          )}
        </div>

        <h2 className="detail__subject selectable">{commit.subject}</h2>
        {d?.body && <CommitBody body={d.body} />}

        <div className="detail__person">
          <span
            className="avatar avatar--lg"
            style={{ '--avatar': laneVar(fnv1a(commit.authorEmail) % LANE_COUNT) } as React.CSSProperties}
          >
            {initials(commit.authorName)}
          </span>
          <div className="detail__person-text">
            <div className="detail__name selectable">{commit.authorName}</div>
            <div className="detail__when">
              {t('detail.authored')} {formatAbsolute(commit.authorDate, locale)}
            </div>
            {d && (d.committerEmail !== d.authorEmail || d.commitDate !== d.authorDate) && (
              <div className="detail__when">
                {t('detail.committedBy', { name: d.committerName })} {formatAbsolute(d.commitDate, locale)}
              </div>
            )}
          </div>
        </div>
      </div>

      {d ? (
        <FileList detail={d} />
      ) : (
        <div className="detail__status">{error ? t('error.generic', { msg: error }) : loading ? t('list.loading') : ''}</div>
      )}
    </aside>
  )
}
