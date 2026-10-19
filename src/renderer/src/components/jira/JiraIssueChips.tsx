import { useMemo } from 'react'
import { useT } from '../../i18n'
import { extractIssueKeys } from '../../lib/issueKeys'
import { useAppStore } from '../../stores/appStore'
import { useJiraStore } from '../../stores/jiraStore'
import './JiraView.css'

/**
 * 커밋 메시지·PR 제목에서 찾은 Jira 이슈 키 칩. 누르면 중앙에 이슈 상세가 열린다.
 * Jira 에 연결되어 있지 않거나 키가 없으면 아무것도 그리지 않는다.
 */
export function JiraIssueChips({ texts }: { texts: (string | undefined)[] }): React.JSX.Element | null {
  const t = useT()
  const connected = useJiraStore((s) => s.auth.connected)
  const projectKeys = useAppStore((s) => s.config?.ui.jiraProjectKeys)
  const key = texts.join('\n')
  const keys = useMemo(() => (connected ? extractIssueKeys(texts, projectKeys) : []), [connected, projectKeys, key])
  if (keys.length === 0) return null
  return (
    <div className="jira-chips" aria-label={t('jira.issues')}>
      {keys.map((k) => (
        <button key={k} className="jira-chip mono" title={t('jira.openIssue', { key: k })} onClick={() => useJiraStore.getState().openIssue(k)}>
          {k}
        </button>
      ))}
    </div>
  )
}
