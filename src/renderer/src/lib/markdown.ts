// 커밋 메시지 본문(마크다운)을 HTML 로 바꾼다.
// 결과는 정화 전 HTML 이다 — 원문의 raw HTML 도 그대로 통과하므로 반드시 sanitizeHtml 을 거쳐야 한다
// (HtmlBody 가 넣기 직전에 정화한다. sanitizeHtml 은 멱등이 아니라 여기서 미리 정화하지 않는다).
import { Marked } from 'marked'

// 커밋 메시지는 줄바꿈을 그대로 의도한 경우가 많아 breaks 를 켠다
const md = new Marked({ gfm: true, breaks: true, async: false })

export function markdownToHtml(src: string): string {
  return md.parse(src) as string
}
