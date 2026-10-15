import { useState } from 'react'
import { avatarPx, avatarSrc } from '@shared/avatar'
import { initials } from '../lib/format'
import { fnv1a, laneVar, LANE_COUNT } from '../lib/laneColor'
import { useAvatarBase } from '../stores/avatarStore'
import './AuthorAvatar.css'

/** 이미 받았거나 실패한 주소 — 행이 다시 만들어져도 깜빡이거나 같은 실패를 되풀이하지 않는다 */
const loaded = new Set<string>()
const failed = new Set<string>()

/**
 * 작성자 아바타: GitHub 프로필 사진을 알면 사진을, 모르거나 로드에 실패하면 이니셜을 보여 준다.
 * 크기는 CSS(.avatar / .avatar--lg)가 정하고, size 는 요청할 이미지 해상도를 고르는 데만 쓴다.
 */
export function AuthorAvatar({ email, name, size = 18, className = '' }: { email: string; name: string; size?: number; className?: string }): React.JSX.Element {
  const base = useAvatarBase(email)
  const [, rerender] = useState(0)
  const src = base ? avatarSrc(base, avatarPx(size, window.devicePixelRatio || 1)) : null
  return (
    <span className={`avatar ${className}`} style={{ '--avatar': laneVar(fnv1a(email) % LANE_COUNT) } as React.CSSProperties}>
      {initials(name)}
      {src && !failed.has(src) && (
        <img
          className={`avatar__img${loaded.has(src) ? ' is-loaded' : ''}`}
          src={src}
          alt=""
          crossOrigin="anonymous"
          referrerPolicy="no-referrer"
          decoding="async"
          draggable={false}
          onLoad={(e) => {
            loaded.add(src)
            e.currentTarget.classList.add('is-loaded')
          }}
          onError={() => {
            failed.add(src)
            rerender((n) => n + 1)
          }}
        />
      )}
    </span>
  )
}
