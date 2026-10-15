import type { FileStatus } from '@shared/types'
import { useT } from '../i18n'

const CLASS: Record<FileStatus, string> = {
  A: 'added',
  M: 'modified',
  D: 'removed',
  R: 'renamed',
  C: 'renamed',
  T: 'modified',
  U: 'removed'
}

/** 변경 종류 아이콘: 추가(+) · 수정(점) · 삭제(−) · 이름 변경(→). 색은 시맨틱 토큰 */
export function FileStatusIcon({ status }: { status: FileStatus }): React.JSX.Element {
  const t = useT()
  return (
    <span className={`status-icon status-icon--${CLASS[status]}`} title={t(`status.${status}`)} aria-label={t(`status.${status}`)}>
      <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth={1.6} strokeLinecap="round">
        <rect x="1.5" y="1.5" width="11" height="11" rx="3" fill="currentColor" fillOpacity={0.16} strokeOpacity={0.7} />
        {status === 'A' && <path d="M7 4.5v5M4.5 7h5" />}
        {(status === 'M' || status === 'T') && <circle cx="7" cy="7" r="1.6" fill="currentColor" stroke="none" />}
        {status === 'D' && <path d="M4.5 7h5" />}
        {(status === 'R' || status === 'C') && <path d="M4.3 7h5.2M7.5 5l2 2-2 2" />}
        {status === 'U' && <path d="M7 4.3v3.4M7 9.6v.1" />}
      </svg>
    </span>
  )
}
