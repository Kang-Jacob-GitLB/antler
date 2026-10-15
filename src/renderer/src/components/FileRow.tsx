import { memo } from 'react'
import type { FileChange } from '@shared/types'
import { useT } from '../i18n'
import { FileStatusIcon } from './FileStatusIcon'

/** 경로를 (디렉터리, 파일 이름)으로 나눈다 */
export function splitPath(path: string): [dir: string, base: string] {
  const i = path.lastIndexOf('/')
  return i < 0 ? ['', path] : [path.slice(0, i + 1), path.slice(i + 1)]
}

/** 변경 파일 한 줄: 상태 아이콘 · 이름/디렉터리 · +/- 통계. 커밋 상세와 작업 트리(WIP) 패널이 함께 쓴다 */
export const FileRow = memo(function FileRow({
  file,
  active,
  tag,
  onOpen
}: {
  file: FileChange
  active: boolean
  /** 이름 옆에 붙는 작은 표지(예: 추적 안 됨) */
  tag?: string
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
      {tag && <span className="file-row__tag">{tag}</span>}
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
