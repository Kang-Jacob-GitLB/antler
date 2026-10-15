import { useEffect, useRef, useState } from 'react'
import { useT } from '../i18n'
import { useAvatarStore } from '../stores/avatarStore'
import { useGithubStore } from '../stores/githubStore'
import { useLogStore } from '../stores/logStore'
import './AvatarBlockNotice.css'

/**
 * 작성자 열 머리의 작은 안내 점: GitHub 프로필 사진을 가져오지 못하는 이유(한도 소진·비공개 저장소·조회 상한)가 있을 때만 나타난다.
 * 호버하면 title 로, 누르면 작은 말풍선으로 설명하고 비로그인이면 GitHub 로그인 팝오버를 여는 버튼을 준다.
 */
export function AvatarBlockNotice({ locale }: { locale: string }): React.JSX.Element | null {
  const t = useT()
  const blocked = useAvatarStore((s) => s.blocked)
  const repoId = useLogStore((s) => s.repoId)
  const setAuthOpen = useGithubStore((s) => s.setAuthPopoverOpen)
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null)
  const btn = useRef<HTMLButtonElement>(null)
  const pop = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!pos) return
    const onDown = (e: MouseEvent): void => {
      const n = e.target as Node
      if (!pop.current?.contains(n) && !btn.current?.contains(n)) setPos(null)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setPos(null)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [pos])

  // 한도가 이미 풀린 시각이면 지난 안내이므로 숨긴다(다음 응답이 오면 갱신된다)
  // 다른 저장소에서 받은 안내는 보이지 않는다
  if (!blocked || blocked.repoId !== repoId || (blocked.reason === 'rate-limited' && blocked.resetAt !== undefined && blocked.resetAt * 1000 <= Date.now())) return null

  const time = blocked.resetAt ? new Date(blocked.resetAt * 1000).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' }) : ''
  const text =
    blocked.reason === 'rate-limited'
      ? t(blocked.signedIn ? 'gh.avatar.rateAuth' : 'gh.avatar.rateAnon', { time })
      : blocked.reason === 'needs-login'
        ? t('gh.avatar.needsLogin')
        : t(blocked.signedIn ? 'gh.avatar.sessionCapAuth' : 'gh.avatar.sessionCapAnon')

  const toggle = (): void => {
    if (pos) return setPos(null)
    const r = btn.current?.getBoundingClientRect()
    if (r) setPos({ top: r.bottom + 6, left: Math.max(8, Math.min(r.left - 8, window.innerWidth - 300)) })
  }

  return (
    <>
      <button ref={btn} type="button" className="avatar-notice__dot" title={text} aria-label={text} aria-expanded={pos !== null} onClick={toggle} />
      {pos && (
        <div ref={pop} className="avatar-notice__pop" role="status" style={{ top: pos.top, left: pos.left }}>
          <div className="avatar-notice__text">{text}</div>
          {!blocked.signedIn && (
            <button
              type="button"
              className="btn btn--sm"
              onClick={() => {
                setPos(null)
                setAuthOpen(true)
              }}
            >
              {t('gh.avatar.signIn')}
            </button>
          )}
        </div>
      )}
    </>
  )
}
