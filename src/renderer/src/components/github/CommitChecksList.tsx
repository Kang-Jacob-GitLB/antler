import { useEffect, useState } from 'react'
import type { GhCheckDetail } from '@shared/types'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { formatDuration } from '../../lib/format'
import { checkKey, isFresh, scopeOf, useChecksStore } from '../../stores/checksStore'
import { useGithubStore } from '../../stores/githubStore'
import { useLogStore } from '../../stores/logStore'
import { CheckIcon } from './GhBits'

/** 커밋을 훑어 넘길 때 요청이 쏟아지지 않게 선택이 멈춘 뒤 이만큼 기다린다 */
const DEBOUNCE_MS = 200

/** 열 수 있는 링크는 github.com 뿐이다(외부 CI 의 legacy status 링크는 열지 않는다) */
const isGithubUrl = (u: string): boolean => /^https:\/\/github\.com\//.test(u)

/** 커밋 상세의 체크 목록(이름·상태·소요 시간·결론). 로그인하지 않았거나 체크가 없거나 조회에 실패하면 아무것도 그리지 않는다 */
export function CommitChecksList({ sha }: { sha: string }): React.JSX.Element | null {
  const t = useT()
  const repoId = useLogStore((s) => s.repoId)
  const ghRef = useGithubStore((s) => s.ref)
  const signedIn = useGithubStore((s) => s.auth.signedIn)
  const scope = scopeOf(ghRef)
  // 목록에서 "체크 없음"으로 확인된 지 얼마 안 된 커밋은 묻지 않는다(수명이 지나면 다시 묻는다)
  const entry = useChecksStore((s) => (scope ? s.states[checkKey(scope, sha)] : undefined))
  const [items, setItems] = useState<{ sha: string; list: GhCheckDetail[] } | null>(null)
  const skip = entry?.state === 'none' && isFresh(entry)
  const enabled = !!repoId && !!scope && signedIn && !skip

  useEffect(() => {
    if (!enabled || !repoId) return
    let live = true
    const timer = setTimeout(() => {
      api.invoke('gh:commitCheckRuns', repoId, sha).then(
        (list) => live && setItems({ sha, list }),
        () => live && setItems(null)
      )
    }, DEBOUNCE_MS)
    return () => {
      live = false
      clearTimeout(timer)
    }
  }, [enabled, repoId, sha])

  if (!enabled || items?.sha !== sha || items.list.length === 0) return null
  return (
    <div className="gh-checklist">
      <div className="gh-checklist__head">{t('gh.checks.title', { n: items.list.length })}</div>
      {items.list.map((c, i) => (
        <div key={i} className="gh-checklist__row">
          <CheckIcon state={c.state} size={13} />
          <span className="gh-checklist__name selectable" title={c.name}>
            {c.name}
          </span>
          <span className="gh-checklist__meta">
            {c.conclusion ?? t(`gh.checks.${c.state}`)}
            {c.durationSec !== undefined && ` · ${formatDuration(c.durationSec)}`}
          </span>
          {c.url &&
            (isGithubUrl(c.url) ? (
              <button type="button" className="gh-checklist__link" onClick={() => void api.invoke('gh:openExternal', c.url!).catch(() => {})}>
                {t('gh.checks.view')}
              </button>
            ) : (
              <span className="gh-checklist__meta" title={t('gh.checks.externalHint')}>
                {t('gh.checks.external')}
              </span>
            ))}
        </div>
      ))}
    </div>
  )
}
