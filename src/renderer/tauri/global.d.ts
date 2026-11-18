// window.antler 의 타입(런타임 구현은 bridge.ts)
import type { AntlerApi } from '@shared/ipc'

declare global {
  interface Window {
    antler: AntlerApi
  }
}

export {}
