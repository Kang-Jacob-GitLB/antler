// 명령 레지스트리의 타입과, 상황(ctx)에 맞는 명령을 질의로 거르는 순수 함수. 스토어를 import 하지 않아 단위 테스트가 가능하다.
import type { Locale, ThemePref } from '@shared/types'
import { rankByQuery, type Ranked } from './fuzzy'
import type { Shortcut } from './shortcuts'

export type CommandCategory = 'general' | 'list' | 'diff' | 'panel'

/** 명령이 보이고 켜지는 조건·현재 상태를 판단하는 데 쓰는 앱 상태 스냅샷 */
export interface CommandCtx {
  hasRepo: boolean
  /** 활성 저장소가 GitHub 저장소이고 목록을 열 수 있는지 */
  github: boolean
  filtered: boolean
  theme: ThemePref
  locale: Locale
  diffMode: 'unified' | 'split'
  autoFetch: number
  minimapCollapsed: boolean
  initials: boolean
  sidebarShown: boolean
  detailShown: boolean
}

export interface CommandLike {
  id: string
  category: CommandCategory
  /** 팔레트·단축키 목록에 보일 이름 */
  label: string
  /** 이름에 없는 검색어(영문 별칭 등) */
  keywords?: string
  /** 상황에 맞지 않아 숨겨야 하면 false */
  available: boolean
  /** 팔레트에는 올리지 않고 단축키 목록·전역 단축키에만 쓰는 명령 */
  paletteHidden?: boolean
  shortcut?: Shortcut
  /** 지금 상태와 같은 선택지(테마·언어 등)라 체크 표시를 한다 */
  checked?: boolean
}

export function filterCommands<T extends CommandLike>(commands: readonly T[], query: string): Ranked<T>[] {
  const visible = commands.filter((c) => c.available && !c.paletteHidden)
  return rankByQuery(visible, query, (c) => ({ title: c.label, keywords: c.keywords }))
}
