import type { SourceKind } from '@shared/types'
import { api } from '../lib/api'

const LOCAL_LABEL: Record<string, string> = { win32: 'WIN', darwin: 'MAC', linux: 'LINUX' }

export function envLabel(kind: SourceKind): string {
  if (kind === 'local') return LOCAL_LABEL[api.platform] ?? 'LOCAL'
  return kind.toUpperCase()
}

export function EnvBadge({ kind }: { kind: SourceKind }): React.JSX.Element {
  return <span className={`env-badge env-badge--${kind}`}>{envLabel(kind)}</span>
}
