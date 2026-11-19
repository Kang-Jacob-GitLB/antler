import { useEffect } from 'react'

/** 마운트(React 루트의 첫 커밋)된 뒤 두 프레임이 지나면 첫 화면이 그려진 것이다. 어댑터(tauri/bridge.ts)가 이 알림을 Rust 에 전해 창을 보이게 한다 */
export function FirstPaint(): null {
  useEffect(() => {
    const id = requestAnimationFrame(() => requestAnimationFrame(() => window.dispatchEvent(new Event('antler:first-paint'))))
    return () => cancelAnimationFrame(id)
  }, [])
  return null
}
