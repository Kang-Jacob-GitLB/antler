// GitHub 화면 공용 조각: 상태 아이콘, 리뷰·CI 배지, 라벨, 아바타, 정화된 HTML 본문
import { memo, useMemo } from 'react'
import type { GhCheckState, GhLabel, GhReviewState, GhUser } from '@shared/types'
import { useT } from '../../i18n'
import { api } from '../../lib/api'
import { sanitizeHtml } from '../../lib/sanitize'
import './GitHubView.css' // HtmlBody 의 .gh-body 스타일(GitHubView 밖에서도 쓰이므로 여기서도 가져온다)

type PrState = 'open' | 'closed' | 'merged' | 'draft'

/** PR·이슈 상태 아이콘(열림 이끼 · merge 벨벳 · 닫힘 러스트 · draft 회색) */
export function GhStateIcon({ kind, state, size = 14 }: { kind: 'pull' | 'issue'; state: PrState; size?: number }): React.JSX.Element {
  const t = useT()
  return (
    <span className={`gh-state gh-state--${state}`} title={t(`gh.state.${state}`)} aria-label={t(`gh.state.${state}`)}>
      <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round">
        {kind === 'pull' ? (
          state === 'merged' ? (
            <>
              <circle cx="4.5" cy="3.5" r="1.5" />
              <circle cx="4.5" cy="12.5" r="1.5" />
              <circle cx="11.5" cy="9" r="1.5" />
              <path d="M4.5 5v6M4.5 5c0 3 2.5 4 5.5 4" />
            </>
          ) : (
            <>
              <circle cx="4.5" cy="3.5" r="1.5" />
              <circle cx="4.5" cy="12.5" r="1.5" />
              <circle cx="11.5" cy="12.5" r="1.5" />
              <path d="M4.5 5v6M11.5 11V6.5a2 2 0 0 0-2-2H7.5M9 3l-1.5 1.5L9 6" />
            </>
          )
        ) : state === 'closed' ? (
          <>
            <circle cx="8" cy="8" r="5.8" />
            <path d="M5.5 8l1.8 1.8L10.8 6.3" />
          </>
        ) : (
          <>
            <circle cx="8" cy="8" r="5.8" />
            <circle cx="8" cy="8" r="1.2" fill="currentColor" stroke="none" />
          </>
        )}
      </svg>
    </span>
  )
}

export function ReviewBadge({ state }: { state?: GhReviewState }): React.JSX.Element | null {
  const t = useT()
  if (!state || state === 'none') return null
  const glyph = state === 'approved' ? '✓' : state === 'changes_requested' ? '±' : state === 'commented' ? '💬' : '…'
  return (
    <span className={`gh-badge gh-badge--review-${state}`} title={t(`gh.review.${state}`)}>
      {glyph}
    </span>
  )
}

export function ChecksBadge({ state }: { state?: GhCheckState }): React.JSX.Element | null {
  const t = useT()
  if (!state || state === 'none') return null
  return <span className={`gh-dot gh-dot--${state}`} title={t(`gh.checks.${state}`)} aria-label={t(`gh.checks.${state}`)} />
}

/** GitHub 라벨: 라벨 색은 GitHub 데이터이므로 테마 토큰과 섞어 채도를 눌러 쓴다 */
export function LabelChip({ label }: { label: GhLabel }): React.JSX.Element {
  const color = /^[0-9a-f]{6}$/i.test(label.color) ? `#${label.color}` : 'var(--text-muted)'
  return (
    <span className="gh-label" style={{ '--label': color } as React.CSSProperties}>
      {label.name}
    </span>
  )
}

export function Avatar({ user, size = 18 }: { user: GhUser; size?: number }): React.JSX.Element {
  // avatars.githubusercontent.com 만 CSP 로 허용된다. 크기 지정으로 작은 이미지만 받는다
  const src = /^https:\/\/avatars\.githubusercontent\.com\//.test(user.avatarUrl)
    ? `${user.avatarUrl}${user.avatarUrl.includes('?') ? '&' : '?'}s=${size * 2}`
    : ''
  return src ? (
    <img className="gh-avatar" src={src} width={size} height={size} alt="" title={user.login} loading="lazy" referrerPolicy="no-referrer" />
  ) : (
    <span className="gh-avatar gh-avatar--blank" style={{ width: size, height: size }} title={user.login} />
  )
}

/**
 * GitHub 가 렌더링한 본문 HTML. 허용 목록으로 다시 정화한 뒤 넣고, 링크는 앱 안에서 이동하지 않고
 * main 을 거쳐 기본 브라우저로 연다(github.com 만 허용).
 */
export const HtmlBody = memo(function HtmlBody({
  html,
  className = '',
  jiraSite
}: {
  html: string
  className?: string
  /** Jira 본문이면 연결한 사이트 주소: 상대 링크의 기준이 되고, 링크는 jira:openExternal(그 사이트 호스트만)로 연다 */
  jiraSite?: string
}) {
  const clean = useMemo(() => sanitizeHtml(html, jiraSite ? `${jiraSite}/` : undefined), [html, jiraSite])
  const open = (e: React.SyntheticEvent): void => {
    const a = (e.target as HTMLElement).closest('[data-href]')
    if (!a) return
    e.preventDefault()
    void api.invoke(jiraSite ? 'jira:openExternal' : 'gh:openExternal', a.getAttribute('data-href')!).catch(() => {})
  }
  return (
    <div
      className={`gh-body selectable ${className}`}
      onClick={open}
      onKeyDown={(e) => e.key === 'Enter' && open(e)}
      dangerouslySetInnerHTML={{ __html: clean }}
    />
  )
})

/** 상대 시간(GitHub 목록용) */
export function useRelative(): (epoch: number) => string {
  const t = useT()
  return (epoch) => {
    const s = Date.now() / 1000 - epoch
    if (s < 3600) return t('gh.ago.min', { n: Math.max(1, Math.round(s / 60)) })
    if (s < 86400) return t('gh.ago.hour', { n: Math.round(s / 3600) })
    if (s < 86400 * 30) return t('gh.ago.day', { n: Math.round(s / 86400) })
    return new Date(epoch * 1000).toLocaleDateString()
  }
}

export function prState(p: { state: 'open' | 'closed' | 'merged'; draft?: boolean }): PrState {
  return p.state === 'open' && p.draft ? 'draft' : p.state
}
