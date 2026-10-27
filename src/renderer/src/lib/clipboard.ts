// 클립보드 복사. 앱이 권한 요청을 모두 거부해 navigator.clipboard.writeText 가 막히므로 main 의 Electron clipboard 를 거친다
import { api } from './api'

export function copyText(text: string): Promise<void> {
  return api.invoke('clipboard:writeText', text)
}
