import { useT } from '../i18n'
import { formatBytes } from '../lib/format'
import type { LfsDiff } from '../diff/lfsDiff'
import './LfsBadge.css'

/** 'LFS' 작은 배지: 파일 행·diff 머리·상태 표시줄에서 Git LFS 로 추적되는 항목을 나타낸다 */
export function LfsBadge({ title }: { title?: string }): React.JSX.Element {
  const t = useT()
  return <span className="lfs-badge" title={title ?? t('lfs.tracked')}>LFS</span>
}

/** LFS 포인터 파일 diff 대신 보여 주는 요약 카드. 실제 LFS 객체는 받지 않아 포인터에 적힌 값만 보여 준다 */
export function LfsCard({ lfs }: { lfs: LfsDiff }): React.JSX.Element {
  const t = useT()
  const { before, after } = lfs
  const cur = after ?? before!
  const sizeText =
    before && after
      ? t('lfs.sizeChange', { from: formatBytes(before.size), to: formatBytes(after.size) })
      : formatBytes(cur.size)
  return (
    <div className="lfs-card" role="note">
      <div className="lfs-card__head">
        <LfsBadge />
        <strong>{t('lfs.cardTitle')}</strong>
        {lfs.converted === 'to' && <span className="lfs-card__state">{t('lfs.convertedTo')}</span>}
        {lfs.converted === 'from' && <span className="lfs-card__state is-del">{t('lfs.convertedFrom')}</span>}
        {!lfs.converted && !before && <span className="lfs-card__state">{t('lfs.added')}</span>}
        {!lfs.converted && !after && <span className="lfs-card__state is-del">{t('lfs.removed')}</span>}
      </div>
      <dl className="lfs-card__grid">
        <dt>{t('lfs.size')}</dt>
        <dd>{sizeText}</dd>
        <dt>oid</dt>
        <dd className="mono" title={`sha256:${cur.oid}`}>
          sha256:{cur.oid}
        </dd>
        {before && after && before.oid !== after.oid && (
          <>
            <dt>{t('lfs.prevOid')}</dt>
            <dd className="mono" title={`sha256:${before.oid}`}>
              sha256:{before.oid}
            </dd>
          </>
        )}
      </dl>
      <p className="lfs-card__note">{t('lfs.note')}</p>
    </div>
  )
}
