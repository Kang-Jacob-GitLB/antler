// Jira 화면 공용 조각
import type { JiraIssueSummary, JiraStatusCategory } from '@shared/types'
import './JiraView.css'

/** 상태 분류. Jira 가 모르는 값(undefined 등)을 주면 '할 일'로 본다 */
export function statusCat(category: string): JiraStatusCategory {
  return category === 'done' || category === 'indeterminate' ? category : 'new'
}

type TypeTone = 'epic' | 'task' | 'bug' | 'story' | 'other'

/** 유형 이름 → 색 분류. 유형 이름은 사이트 언어로 오므로 영문·한글을 함께 본다 */
function typeTone(type: string): TypeTone {
  const t = type.trim().toLowerCase()
  if (t === 'epic' || t === '에픽') return 'epic'
  if (t === 'task' || t === '작업' || t === 'sub-task' || t === 'subtask' || t === '하위 작업') return 'task'
  if (t === 'bug' || t === '버그') return 'bug'
  if (t === 'story' || t === '스토리') return 'story'
  return 'other'
}

/** 색 네모 안의 흰 기호(16px 격자): 에픽 번개 · 작업 체크 · 버그 점 · 스토리 책갈피 · 그 밖 가로줄 */
const TYPE_GLYPH: Record<TypeTone, React.JSX.Element> = {
  epic: <path d="M9 3.5 5.5 8.6h2.6L7 12.5l3.5-5.1H7.9z" fill="#fff" />,
  task: <path d="m5 8.2 2 2 4-4.2" fill="none" stroke="#fff" strokeWidth={1.7} strokeLinecap="round" strokeLinejoin="round" />,
  bug: <circle cx="8" cy="8" r="2.6" fill="#fff" />,
  story: <path d="M5.5 4h5v8L8 10.2 5.5 12z" fill="#fff" />,
  other: <path d="M5 8h6" stroke="#fff" strokeWidth={1.7} strokeLinecap="round" />
}

/**
 * 유형 아이콘(Jira 와 같은 색 네모): 에픽 보라 · 작업 파랑 · 버그 빨강 · 스토리 초록 · 그 밖 회색.
 * 이름은 툴팁으로, withName 이면 옆에 글자로도 보인다
 */
export function JiraType({ type, withName }: { type: string; withName?: boolean }): React.JSX.Element | null {
  if (!type) return null
  const tone = typeTone(type)
  return (
    <span className={`jira-type jira-type--${tone}`} title={type}>
      <svg width={16} height={16} viewBox="0 0 16 16" role="img" aria-label={type}>
        <rect x="1" y="1" width="14" height="14" rx="3" className="jira-type__bg" />
        {TYPE_GLYPH[tone]}
      </svg>
      {withName && <span className="jira-type__name">{type}</span>}
    </span>
  )
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

/** 목록 행의 보조 정보 한 줄: 우선순위 · 담당자(유형은 JiraType 칩으로 따로 보인다) */
export function issueSub(i: JiraIssueSummary, unassigned: string): string {
  return [i.priority, i.assignee || unassigned].filter(Boolean).join(' · ')
}
