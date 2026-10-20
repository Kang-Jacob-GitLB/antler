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
export const IconArrowLeft = (p: P) => (
  <Svg {...p}>
    <path d="M13 8H3.5M7.5 4 3.5 8l4 4" />
  </Svg>
)
/** 왼쪽 패널 토글: 창 테두리 + 왼쪽 띠(켜져 있으면 띠를 채운다) */
export const IconPanelLeft = ({ on, ...p }: P & { on?: boolean }) => (
  <Svg {...p}>
    <rect x="2" y="3" width="12" height="10" rx="1.5" />
    <path d="M6 3v10" />
    {on && <rect x="2.7" y="3.7" width="2.6" height="8.6" rx="0.6" fill="currentColor" stroke="none" />}
  </Svg>
)
/** 오른쪽 패널 토글 */
export const IconPanelRight = ({ on, ...p }: P & { on?: boolean }) => (
  <Svg {...p}>
    <rect x="2" y="3" width="12" height="10" rx="1.5" />
    <path d="M10 3v10" />
    {on && <rect x="10.7" y="3.7" width="2.6" height="8.6" rx="0.6" fill="currentColor" stroke="none" />}
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
export const IconInfo = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="5.8" />
    <path d="M8 7.4v3.6" />
    <circle cx="8" cy="5.1" r="0.5" fill="currentColor" />
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
/** 줄 보기 */
export const IconRows = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 4h11M2.5 8h11M2.5 12h11" />
  </Svg>
)
/** 칸반 보기: 높이가 다른 세 열 */
export const IconBoard = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="2.5" width="3.2" height="11" rx="0.8" />
    <rect x="6.4" y="2.5" width="3.2" height="7.5" rx="0.8" />
    <rect x="10.8" y="2.5" width="3.2" height="9.5" rx="0.8" />
  </Svg>
)
export const IconEye = (p: P) => (
  <Svg {...p}>
    <path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z" />
    <circle cx="8" cy="8" r="1.8" />
  </Svg>
)
export const IconEyeOff = (p: P) => (
  <Svg {...p}>
    <path d="M6.2 3.8A6.6 6.6 0 0 1 8 3.5c4 0 6.5 4.5 6.5 4.5a11 11 0 0 1-2 2.5M9.7 12.2A6.6 6.6 0 0 1 8 12.5C4 12.5 1.5 8 1.5 8a11 11 0 0 1 2.8-3.2" />
    <path d="M2.5 2.5l11 11" />
  </Svg>
)
/** Solo: 과녁 — 이 ref 하나에 집중해서 본다 */
export const IconSolo = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="5.5" />
    <circle cx="8" cy="8" r="1.7" fill="currentColor" />
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
export const IconMore = (p: P) => (
  <Svg {...p}>
    <circle cx="3.5" cy="8" r="1.1" fill="currentColor" stroke="none" />
    <circle cx="8" cy="8" r="1.1" fill="currentColor" stroke="none" />
    <circle cx="12.5" cy="8" r="1.1" fill="currentColor" stroke="none" />
  </Svg>
)
export const IconCode = (p: P) => (
  <Svg {...p}>
    <path d="M5.5 4.5L2 8l3.5 3.5M10.5 4.5L14 8l-3.5 3.5M9 3l-2 10" />
  </Svg>
)
export const IconGear = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="2" />
    <path d="M8 1.8v1.6M8 12.6v1.6M1.8 8h1.6M12.6 8h1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1" />
    <circle cx="8" cy="8" r="4.4" />
  </Svg>
)
export const IconUser = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="5.5" r="2.5" />
    <path d="M3 13.5c.6-2.6 2.6-4 5-4s4.4 1.4 5 4" />
  </Svg>
)
/**
 * Jira 공식 아이콘(Atlassian @atlaskit/logo 23.3.1 의 jira/icon, 브랜드 파랑 #1868DB 타일). Jira 연동 화면을 가리키는 용도로만 쓰고
 * Atlassian 브랜드 지침대로 모양·색을 바꾸지 않는다(그래서 currentColor 를 따르지 않는다)
 */
export const IconJira = ({ size = 16 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <path fill="#1868DB" d="M0 6a6 6 0 0 1 6-6h12a6 6 0 0 1 6 6v12a6 6 0 0 1-6 6H6a6 6 0 0 1-6-6z" />
    <path
      fill="#FFFFFF"
      d="M9.051 15.434H7.734c-1.988 0-3.413-1.218-3.413-3h7.085c.367 0 .605.26.605.63v7.13c-1.772 0-2.96-1.435-2.96-3.434zm3.5-3.543h-1.318c-1.987 0-3.413-1.196-3.413-2.978h7.085c.367 0 .627.239.627.608v7.13c-1.772 0-2.981-1.435-2.981-3.434zm3.52-3.522h-1.317c-1.987 0-3.413-1.217-3.413-3h7.085c.367 0 .605.262.605.61v7.129c-1.771 0-2.96-1.435-2.96-3.434z"
    />
  </svg>
)
export const IconTerminal = (p: P) => (
  <Svg {...p}>
    <rect x="1.8" y="2.8" width="12.4" height="10.4" rx="1.6" />
    <path d="M4.5 6.5L6.5 8l-2 1.5M8 10h3" />
  </Svg>
)
export const IconServer = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="2.5" width="12" height="4.5" rx="1.2" />
    <rect x="2" y="9" width="12" height="4.5" rx="1.2" />
    <path d="M4.5 4.75h.01M4.5 11.25h.01" strokeWidth={2} />
  </Svg>
)
export const IconRepo = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 3a1 1 0 0 1 1-1h8v10h-8a1 1 0 0 0-1 1z" />
    <path d="M3.5 13a1 1 0 0 0 1 1h8v-2" />
  </Svg>
)
export const IconSubmodule = (p: P) => (
  <Svg {...p}>
    <path d="M8 1.8 13.5 4.9v6.2L8 14.2 2.5 11.1V4.9z" />
    <path d="M2.7 5 8 8l5.3-3M8 8v6" />
  </Svg>
)

/** 파일 히스토리: 시계 */
export const IconHistory = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="5.5" />
    <path d="M8 4.8V8l2.2 1.4" />
  </Svg>
)
/** Blame: 줄 목록 + 왼쪽 작성자 표시 */
export const IconBlame = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 4h2M2.5 8h2M2.5 12h2M7 4h6.5M7 8h6.5M7 12h4.5" />
  </Svg>
)
/** 방향 바꾸기: 위·아래 두 화살표 */
export const IconSwap = (p: P) => (
  <Svg {...p}>
    <path d="M5 13V3M2.5 5.5 5 3l2.5 2.5M11 3v10M8.5 10.5 11 13l2.5-2.5" />
  </Svg>
)
/** Launchpad: 발사대(로켓) */
export const IconLaunchpad = (p: P) => (
  <Svg {...p}>
    <path d="M8 1.8c2.2 1.5 3.3 3.7 3.3 6.1v2.3H4.7V7.9C4.7 5.5 5.8 3.3 8 1.8z" />
    <circle cx="8" cy="6.4" r="1.1" />
    <path d="M4.7 8.8 2.8 10.9v1.9l1.9-1.1M11.3 8.8l1.9 2.1v1.9l-1.9-1.1M8 11.6v2.6" />
  </Svg>
)
export const IconPin = ({ filled, ...p }: P & { filled?: boolean }) => (
  <Svg {...p} {...(filled ? { fill: 'currentColor' } : {})}>
    <path d="M9.6 1.9l4.5 4.5-1.6.5-2.2 2.2.2 3-1 1-2.9-2.9-3.7 3.7-.7-.7 3.7-3.7L3 6.6l1-1 3 .2 2.2-2.2z" />
  </Svg>
)
export const IconSnooze = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8.5" r="5" />
    <path d="M8 5.8v2.9l1.9 1.2M3 3l2-1.3M13 3l-2-1.3" />
  </Svg>
)
export const IconPalette = (p: P) => (
  <Svg {...p}>
    <path d="M8 2a6 6 0 1 0 0 12c.9 0 1.4-.6 1.4-1.3 0-.4-.2-.7-.4-1-.2-.3-.4-.6-.4-1 0-.7.6-1.3 1.3-1.3h1.6A2.5 2.5 0 0 0 14 6.9C14 4.2 11.3 2 8 2z" />
    <circle cx="5" cy="7.5" r=".9" fill="currentColor" stroke="none" />
    <circle cx="7.2" cy="4.8" r=".9" fill="currentColor" stroke="none" />
    <circle cx="10.4" cy="5.3" r=".9" fill="currentColor" stroke="none" />
  </Svg>
)
export const IconCalendar = (p: P) => (
  <Svg {...p}>
    <rect x="2.5" y="3.5" width="11" height="10" rx="1.5" />
    <path d="M2.5 6.5h11M5.5 2v3M10.5 2v3" />
  </Svg>
)
