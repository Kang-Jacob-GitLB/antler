// 제목 표시줄 오버레이(Windows·Linux 의 OS 창 컨트롤) 색 동기화.
// 창 컨트롤은 웹 화면 위에 OS 가 따로 그려 CSS 배경막(dim)이 덮지 못한다. 그래서 테마와 함께 "모달이 떠 있는가"를
// main 에 알려 오버레이 색을 배경막과 같은 정도로 어둡게 바꾼다.
import { api } from './api'

/** 화면 전체를 덮는 배경막. 새 모달을 만들면 여기에 클래스를 더한다 */
const BACKDROP_SELECTOR = '.modal-backdrop, .switcher-backdrop'

let dark = true
let dim = false
let sent = ''

function send(): void {
  const key = `${dark}:${dim}`
  if (key === sent) return
  sent = key
  void api.invoke('win:setTitleBarTheme', { dark, dim }).catch(() => {})
}

export function setTitleBarDark(next: boolean): void {
  dark = next
  send()
}

/** 앱 시작 때 한 번 부른다. 배경막이 생기고 없어지는 것을 지켜본다(모달마다 따로 알릴 필요가 없다) */
export function watchBackdrops(root: HTMLElement = document.body): () => void {
  const check = (): void => {
    const next = root.querySelector(BACKDROP_SELECTOR) !== null
    if (next === dim) return
    dim = next
    send()
  }
  const mo = new MutationObserver(check)
  // 배경막은 항상 요소 추가·제거로 나타난다(속성 변화는 볼 필요 없다)
  mo.observe(root, { childList: true, subtree: true })
  check()
  return () => mo.disconnect()
}
