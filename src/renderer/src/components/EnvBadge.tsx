import type { SourceKind } from '@shared/types'
import { api } from '../lib/api'
import { useAppStore } from '../stores/appStore'

const LOCAL_LABEL: Record<string, string> = { win32: 'WIN', darwin: 'MAC', linux: 'LINUX' }

export function envLabel(kind: SourceKind): string {
  if (kind === 'local') return LOCAL_LABEL[api.platform] ?? 'LOCAL'
  return kind.toUpperCase()
}

/** 환경 배지. sourceId 를 주면 그 환경에 사용자가 고른 색(ui.sourceColors)을 쓴다 */
export function EnvBadge({ kind, sourceId }: { kind: SourceKind; sourceId?: string }): React.JSX.Element {
  const hue = useAppStore((s) => (sourceId ? s.config?.ui.sourceColors?.[sourceId] : undefined))
  return (
    <span
      className={`env-badge env-badge--${kind}${hue !== undefined ? ' env-badge--custom' : ''}`}
      style={hue !== undefined ? ({ '--env-hue': hue } as React.CSSProperties) : undefined}
    >
      {envLabel(kind)}
    </span>
  )
}
