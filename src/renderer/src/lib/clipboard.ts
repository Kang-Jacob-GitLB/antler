// 클립보드 복사. 앱이 권한 요청을 모두 거부해 navigator.clipboard.writeText 가 막히므로 Rust 의 clipboard:writeText 채널을 거친다
import { api } from './api'

export function copyText(text: string): Promise<void> {
  return api.invoke('clipboard:writeText', text)
}
