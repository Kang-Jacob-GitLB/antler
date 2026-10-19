import { create } from 'zustand'

/** 외부 도구로 열기 메뉴(파일 행 우클릭·diff 머리줄·툴바 저장소 이름)의 위치와 대상 */
export interface ExternalMenuTarget {
  x: number
  y: number
  repoId: string
  /** 저장소 루트 기준 경로. 없으면 저장소 폴더 */
  path?: string
}

interface ExternalMenuState {
  menu: ExternalMenuTarget | null
  show(menu: ExternalMenuTarget): void
  close(): void
}

export const useExternalMenuStore = create<ExternalMenuState>((set) => ({
  menu: null,
  show: (menu) => set({ menu }),
  close: () => set({ menu: null })
}))

/** 버튼 바로 아래에 메뉴를 연다 */
export function showExternalMenuAt(el: HTMLElement, repoId: string, path?: string): void {
  const r = el.getBoundingClientRect()
  useExternalMenuStore.getState().show({ x: r.left, y: r.bottom + 4, repoId, ...(path ? { path } : {}) })
}
