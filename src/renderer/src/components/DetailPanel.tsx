import { useEffect, useState } from 'react'
import { useT } from '../i18n'
import { formatAbsolute, initials } from '../lib/format'
import { fnv1a, LANE_COUNT, laneVar } from '../lib/laneColor'
import { useAppStore } from '../stores/appStore'
import { useLogStore } from '../stores/logStore'
import { IconCopy } from './icons'
import './DetailPanel.css'

/** 선택한 커밋의 메타데이터. 변경 파일·diff 는 M3 에서 이 패널 아래에 붙는다. */
export function DetailPanel({ width }: { width: number }): React.JSX.Element {
  const t = useT()
  const locale = useAppStore((s) => s.config?.ui.locale ?? 'ko')
  const commit = useLogStore((s) => (s.selected >= 0 ? s.commits[s.selected] : undefined))
  const select = useLogStore((s) => s.select)
  const commits = useLogStore((s) => s.commits)
  const [copied, setCopied] = useState(false)

  useEffect(() => setCopied(false), [commit?.hash])

  if (!commit) {
    return (
      <aside className="detail" style={{ width }}>
        <div className="detail__empty">{t('detail.empty')}</div>
      </aside>
    )
  }

  const copy = async (): Promise<void> => {
    await navigator.clipboard.writeText(commit.hash)
    setCopied(true)
  }
  const jumpTo = (hash: string): void => {
    const i = commits.findIndex((c) => c.hash === hash)
    if (i >= 0) select(i)
  }

  return (
    <aside className="detail" style={{ width }}>
      <div className="detail__scroll">
        <div className="detail__sha-row">
          <span className="detail__sha mono selectable">{commit.hash.slice(0, 10)}</span>
          <button className="icon-btn icon-btn--sm" title={t('detail.copy')} onClick={() => void copy()}>
            <IconCopy size={13} />
          </button>
          {copied && <span className="detail__copied">{t('detail.copied')}</span>}
        </div>

        <h2 className="detail__subject selectable">{commit.subject}</h2>

        <div className="detail__person">
          <span
            className="avatar avatar--lg"
            style={{ '--avatar': laneVar(fnv1a(commit.authorEmail) % LANE_COUNT) } as React.CSSProperties}
          >
            {initials(commit.authorName)}
          </span>
          <div className="detail__person-text">
            <div className="detail__name selectable">{commit.authorName}</div>
            <div className="detail__email selectable">{commit.authorEmail}</div>
          </div>
        </div>

        <dl className="detail__meta">
          <dt>{t('detail.author')}</dt>
          <dd>{formatAbsolute(commit.authorDate, locale)}</dd>
          {commit.commitDate !== commit.authorDate && (
            <>
              <dt>{t('detail.committed')}</dt>
              <dd>{formatAbsolute(commit.commitDate, locale)}</dd>
            </>
          )}
          {commit.parents.length > 0 && (
            <>
              <dt>{t('detail.parents')}</dt>
              <dd className="detail__parents">
                {commit.parents.map((p) => (
                  <button key={p} className="detail__parent mono" onClick={() => jumpTo(p)} title={p}>
                    {p.slice(0, 7)}
                  </button>
                ))}
              </dd>
            </>
          )}
        </dl>

        <div className="detail__soon">{t('detail.filesSoon')}</div>
      </div>
    </aside>
  )
}
