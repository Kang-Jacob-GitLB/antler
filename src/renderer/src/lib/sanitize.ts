// GitHub 가 렌더링한 PR·이슈 본문(body_html)을 화면에 넣기 전에 한 번 더 정화한다.
// GitHub 가 이미 정화한 HTML 이지만, 앱 쪽에서도 허용 목록만 남겨 이중으로 막는다(CSP 와 별개로).
//  - 허용한 태그·속성만 남기고 나머지 태그는 내용만 남긴다(script·style·iframe 등은 내용까지 제거)
//  - 링크는 https 만, 이미지는 https 만(실제 로드 도메인은 CSP 가 다시 제한)
//  - 링크는 클릭 시 앱이 기본 브라우저로 열도록 data-href 로 옮긴다(앱 안 탐색 금지)

const ALLOWED_TAGS = new Set([
  'a', 'abbr', 'b', 'blockquote', 'br', 'code', 'dd', 'del', 'details', 'div', 'dl', 'dt', 'em',
  'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i', 'img', 'ins', 'kbd', 'li', 'mark', 'ol', 'p', 'pre',
  'q', 's', 'samp', 'span', 'strike', 'strong', 'sub', 'summary', 'sup', 'table', 'tbody', 'td',
  'tfoot', 'th', 'thead', 'tr', 'tt', 'u', 'ul', 'input'
])

/** 내용까지 통째로 버릴 태그 */
const DROP_WITH_CONTENT = new Set(['script', 'style', 'iframe', 'object', 'embed', 'noscript', 'template', 'svg', 'math', 'form', 'textarea', 'select', 'button', 'link', 'meta', 'base'])

const ALLOWED_ATTRS: Record<string, Set<string>> = {
  a: new Set(['href', 'title']),
  img: new Set(['src', 'alt', 'title', 'width', 'height']),
  td: new Set(['align', 'colspan', 'rowspan']),
  th: new Set(['align', 'colspan', 'rowspan']),
  ol: new Set(['start']),
  details: new Set(['open']),
  input: new Set(['type', 'checked', 'disabled']),
  '*': new Set(['lang', 'dir'])
}

function safeHttpsUrl(raw: string | null): string | null {
  if (!raw) return null
  try {
    const u = new URL(raw, 'https://github.com/')
    return u.protocol === 'https:' ? u.href : null
  } catch {
    return null
  }
}

function clean(node: Node, doc: Document): void {
  for (const child of [...node.childNodes]) {
    if (child.nodeType === 3) continue // 텍스트
    if (child.nodeType !== 1) {
      child.parentNode?.removeChild(child) // 주석 등
      continue
    }
    const el = child as Element
    const tag = el.tagName.toLowerCase()
    if (DROP_WITH_CONTENT.has(tag)) {
      el.remove()
      continue
    }
    if (!ALLOWED_TAGS.has(tag)) {
      // 모르는 태그는 벗기고 내용만 남긴다
      clean(el, doc)
      el.replaceWith(...[...el.childNodes])
      continue
    }
    // 작업 목록 체크박스 외의 input 은 버린다
    if (tag === 'input' && el.getAttribute('type') !== 'checkbox') {
      el.remove()
      continue
    }
    const allowed = ALLOWED_ATTRS[tag]
    for (const attr of [...el.attributes]) {
      const name = attr.name.toLowerCase()
      if (!(allowed?.has(name) || ALLOWED_ATTRS['*']!.has(name))) el.removeAttribute(attr.name)
    }
    if (tag === 'a') {
      const href = safeHttpsUrl(el.getAttribute('href'))
      el.removeAttribute('href')
      if (href) {
        el.setAttribute('data-href', href)
        el.setAttribute('role', 'link')
        el.setAttribute('tabindex', '0')
      }
    }
    if (tag === 'img') {
      const src = safeHttpsUrl(el.getAttribute('src'))
      if (src) {
        el.setAttribute('src', src)
        el.setAttribute('loading', 'lazy')
        el.setAttribute('referrerpolicy', 'no-referrer')
      } else el.remove()
    }
    if (tag === 'input') el.setAttribute('disabled', '')
    clean(el, doc)
  }
}

/** 허용 목록으로 정화한 HTML 문자열 */
export function sanitizeHtml(html: string): string {
  const doc = new DOMParser().parseFromString(`<!doctype html><body>${html}`, 'text/html')
  clean(doc.body, doc)
  return doc.body.innerHTML
}
