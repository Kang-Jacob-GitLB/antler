import { create } from 'zustand'
import { useAppStore } from './appStore'

/** 명령 팔레트(Ctrl/⌘+Shift+P)와 단축키 목록(Ctrl/⌘+/) 모달의 열림 상태 */
interface PaletteState {
  paletteOpen: boolean
  shortcutsOpen: boolean
  setPaletteOpen(open: boolean): void
  setShortcutsOpen(open: boolean): void
}

export const usePaletteStore = create<PaletteState>((set) => ({
  paletteOpen: false,
  shortcutsOpen: false,
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
  setShortcutsOpen: (shortcutsOpen) => set({ shortcutsOpen })
}))

/**
 * 가운데 뜨는 팝업(팔레트·단축키 목록·저장소 전환·저장소 추가)이 하나라도 열려 있는지.
 * 열려 있는 동안 전역 단축키(App 의 onKey)는 끼어들지 않고 팝업이 자기 키를 처리한다.
 */
export function isOverlayOpen(): boolean {
  const app = useAppStore.getState()
  const p = usePaletteStore.getState()
  return app.switcherOpen || app.addOpen || p.paletteOpen || p.shortcutsOpen
}
