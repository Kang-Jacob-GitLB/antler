import type { AntlerApi } from '../shared/ipc'

declare global {
  interface Window {
    antler: AntlerApi
  }
}

export {}
