import { create } from 'zustand'
import type { UpdateState } from '@shared/types'
import { api, errorMessage } from '../lib/api'

interface UpdateStore {
  state: UpdateState | null
  /** 사용자가 [×] 로 닫은 알림("상태|버전"). 상태가 바뀌면 다시 보인다 */
  hiddenKey: string | null
  /** 메뉴에서 직접 확인한 경우에만 '확인 중'·'최신 버전' 안내를 보여 준다 */
  manual: boolean
  init(): () => void
  check(): Promise<void>
  download(): Promise<void>
  cancel(): void
  install(): void
  dismiss(action: 'later' | 'skip'): void
  hide(): void
}

const keyOf = (s: UpdateState): string => `${s.status}|${s.version ?? ''}`

export const useUpdateStore = create<UpdateStore>((set, get) => {
  const apply = (state: UpdateState): void => set({ state, hiddenKey: get().hiddenKey === keyOf(state) ? get().hiddenKey : null })
  const fail = (e: unknown): void => {
    const cur = get().state
    if (cur) apply({ ...cur, status: 'error', error: errorMessage(e) })
  }
  return {
    state: null,
    hiddenKey: null,
    manual: false,
    init() {
      void api.invoke('update:get').then(apply).catch(() => {})
      return api.on('update:state', apply)
    },
    async check() {
      set({ manual: true, hiddenKey: null })
      try {
        apply(await api.invoke('update:check'))
      } catch (e) {
        fail(e)
      }
    },
    async download() {
      try {
        apply(await api.invoke('update:download'))
      } catch (e) {
        fail(e)
      }
    },
    cancel() {
      void api.invoke('update:cancel').catch(() => {})
    },
    install() {
      void api.invoke('update:install').catch(fail)
    },
    dismiss(action) {
      const v = get().state?.version
      if (!v) return
      set({ manual: false })
      void api.invoke('update:dismiss', action, v).then(apply).catch(() => {})
    },
    hide() {
      const s = get().state
      if (s) set({ hiddenKey: keyOf(s), manual: false })
    }
  }
})
