import { useEffect, useRef } from 'react'
import type { ExternalAction } from '@shared/types'
import { useT, type MessageKey } from '../i18n'
import { externalSupported, openExternal, openOnGithub, sourceKindOf } from '../lib/external'
import { useExternalMenuStore } from '../stores/externalMenuStore'
import { IconCode, IconFolder, IconGithub, IconTerminal } from './icons'

export const EXTERNAL_ITEMS: { action: ExternalAction; label: MessageKey; icon: typeof IconFolder }[] = [
  { action: 'fileManager', label: 'ext.fileManager', icon: IconFolder },
  { action: 'editor', label: 'ext.editor', icon: IconCode },
  { action: 'terminal', label: 'ext.terminal', icon: IconTerminal }
]

/** 파일 행·diff 머리줄·툴바 저장소 이름이 여는 작은 메뉴(앱에 하나). 위치·대상은 externalMenuStore 가 정한다 */
export function ExternalMenu(): React.JSX.Element | null {
  const menu = useExternalMenuStore((s) => s.menu)
  if (!menu) return null
  return <Body />
}

function Body(): React.JSX.Element {
  const t = useT()
  const menu = useExternalMenuStore((s) => s.menu)!
  const close = useExternalMenuStore((s) => s.close)
  const ref = useRef<HTMLDivElement>(null)
  const kind = sourceKindOf(menu.repoId)

  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as Node)) close()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        close()
      }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey, true)
    // 기준이 되던 요소가 움직이면 닫는다
    document.addEventListener('scroll', close, true)
    window.addEventListener('blur', close)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey, true)
      document.removeEventListener('scroll', close, true)
      window.removeEventListener('blur', close)
    }
  }, [close])

  // 화면 밖으로 나가지 않게 위치를 맞춘다
  const pos = { left: Math.max(4, Math.min(menu.x, window.innerWidth - 230)), top: Math.max(4, Math.min(menu.y, window.innerHeight - 120)) }

  return (
    <div ref={ref} className="side-ctx" role="menu" aria-label={t('ext.menu')} style={pos} onContextMenu={(e) => e.preventDefault()}>
      {kind === 'github' && (
        // 원격 전용 저장소: 작업 폴더가 없으므로 폴더·에디터·터미널 대신 GitHub 웹으로 연다
        <button
          className="side-ctx__item side-ctx__item--icon"
          role="menuitem"
          autoFocus
          onClick={() => {
            close()
            void openOnGithub(menu.repoId)
          }}
        >
          <IconGithub size={14} />
          <span>{t('ext.openGithub')}</span>
        </button>
      )}
      {kind !== 'github' && EXTERNAL_ITEMS.map(({ action, label, icon: Icon }, i) => {
        const ok = externalSupported(kind, action)
        return (
          <button
            key={action}
            className="side-ctx__item side-ctx__item--icon"
            role="menuitem"
            disabled={!ok}
            title={ok ? undefined : t('ext.sshOnlyEditor')}
            autoFocus={i === 0 && ok}
            onClick={() => {
              close()
              void openExternal(menu.repoId, action, menu.path)
            }}
          >
            <Icon size={14} />
            <span>{t(label)}</span>
          </button>
        )
      })}
    </div>
  )
}
