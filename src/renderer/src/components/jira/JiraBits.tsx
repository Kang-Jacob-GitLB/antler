// Jira 화면 공용 조각
import type { JiraIssueSummary } from '@shared/types'
import './JiraView.css'

/** 상태 칩. statusCategory(new | indeterminate | done)로 색을 정한다 */
export function JiraStatus({ name, category }: { name: string; category: string }): React.JSX.Element | null {
  if (!name) return null
  const tone = category === 'done' ? 'done' : category === 'indeterminate' ? 'active' : 'todo'
  return <span className={`jira-status jira-status--${tone}`}>{name}</span>
}

/** ISO 시각 → epoch seconds(해석할 수 없으면 0) */
export function isoToEpoch(iso: string): number {
  const ms = Date.parse(iso)
  return Number.isFinite(ms) ? ms / 1000 : 0
}

/** 목록 행의 보조 정보 한 줄: 유형 · 우선순위 · 담당자 */
export function issueSub(i: JiraIssueSummary, unassigned: string): string {
  return [i.issueType, i.priority, i.assignee || unassigned].filter(Boolean).join(' · ')
}
