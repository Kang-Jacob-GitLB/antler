import type { JiraUser, JiraWorklog } from '@shared/types'
import { fnv1a, laneVar, LANE_COUNT } from '../../../lib/laneColor'
import { useJiraStore } from '../../../stores/jiraStore'

/** 사용자 id → 색(레인 팔레트). 같은 사람은 칩·블록·표에서 항상 같은 색 */
export const userColor = (id: string): string => laneVar(fnv1a(id) % LANE_COUNT)

/** 블록·목록의 색: 칩·표의 점과 같은 사용자 색 */
export function makeColorOf(): (authorId: string) => string {
  return userColor
}

/** 달력·표 공용 입력(범위·거른 항목·고른 사용자) */
export interface WorklogPaneProps {
  /** 격자에 깔리는 날짜들 */
  gridDays: string[]
  /** 합계·표의 대상 날짜 */
  days: string[]
  logs: JiraWorklog[]
  users: JiraUser[]
  anchor: string
  today: string
  locale: 'ko' | 'en'
}

/** 이슈 키 버튼: 누르면 이슈를 연다(돌아오면 워크로그 화면 그대로) */
export function IssueKey({ issueKey, title }: { issueKey: string; title?: string }): React.JSX.Element {
  return (
    <button
      className="wl-key jira-key mono"
      title={title ?? issueKey}
      onClick={(e) => {
        e.stopPropagation()
        useJiraStore.getState().openIssue(issueKey)
      }}
    >
      {issueKey}
    </button>
  )
}
