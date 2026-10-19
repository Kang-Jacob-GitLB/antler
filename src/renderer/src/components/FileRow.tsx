import { memo } from 'react'
import type { FileChange } from '@shared/types'
import { useT } from '../i18n'
import { useAppStore } from '../stores/appStore'
import { showExternalMenuAt, useExternalMenuStore } from '../stores/externalMenuStore'
import { FileStatusIcon } from './FileStatusIcon'
import { LfsBadge } from './LfsBadge'
import { IconBlame, IconHistory, IconMore } from './icons'

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
  lfs,
  onOpen,
  onHistory,
  onBlame
}: {
  file: FileChange
  active: boolean
  /** 이름 옆에 붙는 작은 표지(예: 추적 안 됨) */
  tag?: string
  /** Git LFS 로 추적되는 파일이면 'LFS' 배지를 붙인다 */
  lfs?: boolean
  onOpen(f: FileChange): void
  /** 있으면 마우스를 올릴 때 파일 히스토리·Blame 버튼을 보여 준다 */
  onHistory?(f: FileChange): void
  onBlame?(f: FileChange): void
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
      onContextMenu={(e) => {
        const repoId = useAppStore.getState().activeRepoId
        if (!repoId) return
        e.preventDefault()
        useExternalMenuStore.getState().show({ x: e.clientX, y: e.clientY, repoId, path: file.path })
      }}
    >
      <FileStatusIcon status={file.status} />
      <span className="file-row__name">
        <span className="file-row__base">{base}</span>
        {dir && <span className="file-row__dir">{dir}</span>}
        {file.oldPath && <span className="file-row__from">← {splitPath(file.oldPath)[1]}</span>}
      </span>
      {lfs && <LfsBadge />}
      {tag && <span className="file-row__tag">{tag}</span>}
      <span className="file-row__actions" onClick={(e) => e.stopPropagation()}>
        {onHistory && (
          <button className="icon-btn icon-btn--sm" title={t('fileView.openHistory')} onClick={() => onHistory(file)}>
            <IconHistory size={13} />
          </button>
        )}
        {onBlame && (
          <button className="icon-btn icon-btn--sm" title={t('fileView.openBlame')} onClick={() => onBlame(file)}>
            <IconBlame size={13} />
          </button>
        )}
        <button
          className="icon-btn icon-btn--sm"
          title={t('ext.menu')}
          aria-label={t('ext.menu')}
          onClick={(e) => {
            const repoId = useAppStore.getState().activeRepoId
            if (repoId) showExternalMenuAt(e.currentTarget, repoId, file.path)
          }}
        >
          <IconMore size={13} />
        </button>
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
