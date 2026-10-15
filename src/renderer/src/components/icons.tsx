// 선형 아이콘 세트(16px 그리드, currentColor). 아이콘 라이브러리 의존성을 피하려고 필요한 것만 둔다.
import type { SVGProps } from 'react'

type P = SVGProps<SVGSVGElement> & { size?: number }

function Svg({ size = 16, children, ...rest }: P): React.JSX.Element {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  )
}

export const IconPlus = (p: P) => (
  <Svg {...p}>
    <path d="M8 3v10M3 8h10" />
  </Svg>
)
export const IconRefresh = (p: P) => (
  <Svg {...p}>
    <path d="M13 8a5 5 0 1 1-1.5-3.55" />
    <path d="M13 2.5v3h-3" />
  </Svg>
)
export const IconBranch = (p: P) => (
  <Svg {...p}>
    <circle cx="4.5" cy="3.5" r="1.5" />
    <circle cx="4.5" cy="12.5" r="1.5" />
    <circle cx="11.5" cy="5" r="1.5" />
    <path d="M4.5 5v6M11.5 6.5c0 2.5-3 2.5-6.2 4.6" />
  </Svg>
)
export const IconTag = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 2.5h5l6 6-5 5-6-6z" />
    <circle cx="5.5" cy="5.5" r="0.9" fill="currentColor" stroke="none" />
  </Svg>
)
export const IconCloud = (p: P) => (
  <Svg {...p}>
    <path d="M4.5 12.5h7a2.5 2.5 0 0 0 .3-5 3.8 3.8 0 0 0-7.3-.7A2.9 2.9 0 0 0 4.5 12.5z" />
  </Svg>
)
export const IconClose = (p: P) => (
  <Svg {...p}>
    <path d="M4 4l8 8M12 4l-8 8" />
  </Svg>
)
export const IconCopy = (p: P) => (
  <Svg {...p}>
    <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
    <path d="M10.5 3.5v-.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.5" />
  </Svg>
)
export const IconSearch = (p: P) => (
  <Svg {...p}>
    <circle cx="7" cy="7" r="4.2" />
    <path d="M10.2 10.2L13.5 13.5" />
  </Svg>
)
export const IconSun = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="2.8" />
    <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M3.4 12.6l1-1M11.6 4.4l1-1" />
  </Svg>
)
export const IconMoon = (p: P) => (
  <Svg {...p}>
    <path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5z" />
  </Svg>
)
export const IconMonitor = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="3" width="12" height="8" rx="1.2" />
    <path d="M6 13.5h4M8 11v2.5" />
  </Svg>
)
export const IconGlobe = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="5.8" />
    <path d="M2.2 8h11.6M8 2.2c1.8 1.7 2.6 3.6 2.6 5.8S9.8 12.1 8 13.8C6.2 12.1 5.4 10.2 5.4 8S6.2 3.9 8 2.2z" />
  </Svg>
)
export const IconChevronDown = (p: P) => (
  <Svg {...p}>
    <path d="M4.5 6.5L8 10l3.5-3.5" />
  </Svg>
)
export const IconFunnel = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 3h11l-4.2 5v4.5l-2.6 1V8z" />
  </Svg>
)
export const IconCheck = (p: P) => (
  <Svg {...p}>
    <path d="M3 8.5l3.2 3L13 4.5" />
  </Svg>
)
export const IconFolder = (p: P) => (
  <Svg {...p}>
    <path d="M2 4.5a1 1 0 0 1 1-1h3.2l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
  </Svg>
)
export const IconStash = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 6.5h11v6a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z" />
    <path d="M1.8 4h12.4v2.5H1.8zM6.5 9h3" />
  </Svg>
)
export const IconWorktree = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="2.5" width="5" height="4" rx="1" />
    <rect x="9" y="9.5" width="5" height="4" rx="1" />
    <path d="M4.5 6.5v3.5a1.5 1.5 0 0 0 1.5 1.5h3" />
  </Svg>
)
export const IconChevronRight = (p: P) => (
  <Svg {...p}>
    <path d="M6.5 4.5L10 8l-3.5 3.5" />
  </Svg>
)
export const IconRepo = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 3a1 1 0 0 1 1-1h8v10h-8a1 1 0 0 0-1 1z" />
    <path d="M3.5 13a1 1 0 0 0 1 1h8v-2" />
  </Svg>
)
