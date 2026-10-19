import type { GitflowRole } from '@shared/gitflow'
import { classifyBranch } from '@shared/gitflow'
import { useT } from '../i18n'
import { branchKey } from '../lib/laneColor'
import { useGitflow } from '../stores/repoExtrasStore'
import './FlowBadge.css'

const LETTER: Record<GitflowRole, string> = { feature: 'F', release: 'R', hotfix: 'H', bugfix: 'B', support: 'S', develop: 'D', master: 'M' }

/** 브랜치 이름(리모트면 'origin/…' 포함)의 Gitflow 범주. Gitflow 를 쓰지 않는 저장소면 null */
function useFlowRole(name: string, remote: boolean): GitflowRole | null {
  const flow = useGitflow()
  return flow ? classifyBranch(branchKey(name, remote), flow) : null
}

/** 사이드바 브랜치 행 끝의 범주 배지(해당 없으면 아무것도 그리지 않는다) */
export function BranchFlowBadge({ name, remote }: { name: string; remote: boolean }): React.JSX.Element | null {
  const role = useFlowRole(name, remote)
  return role ? <FlowBadge role={role} /> : null
}

/** 커밋 목록 브랜치 칩의 범주 점. label 은 리모트 이름을 뗀 브랜치 이름이다 */
export function ChipFlowDot({ label }: { label: string }): React.JSX.Element | null {
  const role = useFlowRole(label, false)
  return role ? <FlowDot role={role} /> : null
}

/** 사이드바 브랜치 행의 범주 배지(색 + 한 글자). develop·master 는 채워서 강조한다 */
export function FlowBadge({ role }: { role: GitflowRole }): React.JSX.Element {
  const t = useT()
  const main = role === 'develop' || role === 'master'
  return (
    <span className={`flow-badge flow-badge--${role}${main ? ' is-main' : ''}`} title={t(`flow.role.${role}`)}>
      {LETTER[role]}
    </span>
  )
}

/** 커밋 목록 브랜치 칩 안의 범주 색 점 */
export function FlowDot({ role }: { role: GitflowRole }): React.JSX.Element {
  const t = useT()
  return <span className={`flow-dot flow-badge--${role}`} title={t(`flow.role.${role}`)} />
}
