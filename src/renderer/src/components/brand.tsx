// 브랜드 심볼과 빈 화면 일러스트. 갈라진 사슴뿔 가지 = git 레인, 가지 끝 점 = 앰버 노드.

/** 앱 로고 마크(툴바용). resources/icon.svg 와 같은 형태를 단순화했다. */
export function AntlerMark({ size = 20 }: { size?: number }): React.JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <g stroke="var(--text-primary)" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
        <path d="M12 22V13" />
        <path d="M12 13C12 9.5 8.5 8.5 7 5" />
        <path d="M12 13C12 9.5 15.5 8.5 17 5" />
        <path d="M8.4 7.6L4.6 7" />
        <path d="M15.6 7.6L19.4 7" />
      </g>
      <g fill="var(--accent)">
        <circle cx="7" cy="4.2" r="1.9" />
        <circle cx="17" cy="4.2" r="1.9" />
        <circle cx="3.8" cy="6.9" r="1.6" />
        <circle cx="20.2" cy="6.9" r="1.6" />
      </g>
    </svg>
  )
}

/** 빈 화면용 단색 라인아트: 땅에서 솟아 여러 갈래로 뻗는 가지 */
export function AntlerIllustration(): React.JSX.Element {
  return (
    <svg width="220" height="170" viewBox="0 0 220 170" fill="none" aria-hidden="true">
      <g stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
        <path d="M30 158h160" opacity={0.35} />
        <path d="M110 158V104" />
        {/* 왼쪽 뿔 */}
        <path d="M110 104C110 80 86 74 74 52" />
        <path d="M74 52C70 42 70 32 74 20" />
        <path d="M83 66C70 64 58 58 48 44" />
        <path d="M48 44C44 38 40 34 32 32" />
        <path d="M76 56C66 50 62 40 58 26" />
        {/* 오른쪽 뿔 */}
        <path d="M110 104C110 80 134 74 146 52" />
        <path d="M146 52C150 42 150 32 146 20" />
        <path d="M137 66C150 64 162 58 172 44" />
        <path d="M172 44C176 38 180 34 188 32" />
        <path d="M144 56C154 50 158 40 162 26" />
      </g>
      <g fill="var(--bg-base)" stroke="currentColor" strokeWidth={2}>
        <circle cx="74" cy="18" r="4" />
        <circle cx="58" cy="24" r="4" />
        <circle cx="30" cy="31" r="4" />
        <circle cx="146" cy="18" r="4" />
        <circle cx="162" cy="24" r="4" />
        <circle cx="190" cy="31" r="4" />
        <circle cx="110" cy="104" r="4" />
      </g>
    </svg>
  )
}
