import { useSyncExternalStore } from 'react'
import type { Locale } from '@shared/types'
import { formatDate, RELATIVE_MAX_SEC } from '../lib/format'

/** "n분 전" 표시를 갱신하는 주기 */
const TICK_MS = 30_000

// 모든 DateText 가 시계 하나를 나눠 쓴다. 구독자가 없으면 타이머도 멈춘다
let now = Date.now()
let timer: ReturnType<typeof setInterval> | null = null
const listeners = new Set<() => void>()

function subscribe(cb: () => void): () => void {
  listeners.add(cb)
  if (!timer) {
    now = Date.now()
    timer = setInterval(() => {
      now = Date.now()
      listeners.forEach((l) => l())
    }, TICK_MS)
  }
  return () => {
    listeners.delete(cb)
    if (listeners.size === 0 && timer) {
      clearInterval(timer)
      timer = null
    }
  }
}

const noopSubscribe = (): (() => void) => () => {}
const getNow = (): number => now

/**
 * formatDate 결과를 보여 주되, 상대 시간 범위(3시간 이내)면 시계에 맞춰 다시 그린다.
 * memo 된 행 안에서도 "n분 전" 이 멈춰 있지 않게 한다.
 */
export function DateText({ epochSec, locale }: { epochSec: number; locale: Locale }): React.JSX.Element {
  // 범위 밖(날짜·시각 표기)은 바뀔 일이 없으니 구독하지 않는다. 경계 직전 행은 다음 틱에 날짜로 넘어간다
  const relative = Math.abs(Date.now() / 1000 - epochSec) < RELATIVE_MAX_SEC + TICK_MS / 1000
  // 틱마다 다시 그리게만 하고, 계산은 formatDate 가 그 순간의 Date.now() 로 한다
  useSyncExternalStore(relative ? subscribe : noopSubscribe, getNow)
  return <>{formatDate(epochSec, locale)}</>
}
