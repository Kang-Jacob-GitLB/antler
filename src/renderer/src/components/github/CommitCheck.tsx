import { checkKey, scopeOf, useChecksStore } from '../../stores/checksStore'
import { useGithubStore } from '../../stores/githubStore'
import { CheckIcon } from './GhBits'

/** 커밋 행의 체크 상태 아이콘. 상태를 모르거나 체크가 없으면 아무것도 그리지 않는다 */
export function CommitCheck({ hash }: { hash: string }): React.JSX.Element | null {
  const scope = useGithubStore((s) => scopeOf(s.ref))
  const state = useChecksStore((s) => (scope ? s.states[checkKey(scope, hash)]?.state : undefined))
  if (!state || state === 'none') return null
  return <CheckIcon state={state} size={13} />
}
