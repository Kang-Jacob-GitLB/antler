// Jira 화면 공용 조각
import type { JiraIssueSummary, JiraStatusCategory } from '@shared/types'
import './JiraView.css'

/** 상태 분류. Jira 가 모르는 값(undefined 등)을 주면 '할 일'로 본다 */
export function statusCat(category: string): JiraStatusCategory {
  return category === 'done' || category === 'indeterminate' ? category : 'new'
}

/** 상태 칩. statusCategory(new | indeterminate | done)로 색을 정한다 */
export function JiraStatus({ name, category }: { name: string; category: string }): React.JSX.Element | null {
  if (!name) return null
  const cat = statusCat(category)
  const tone = cat === 'done' ? 'done' : cat === 'indeterminate' ? 'active' : 'todo'
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
