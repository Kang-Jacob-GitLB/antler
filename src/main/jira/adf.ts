// Atlassian Document Format(ADF, Jira Cloud 의 설명·댓글 JSON) → HTML.
// 모든 텍스트·속성은 이스케이프하고, 모르는 노드는 자식만 렌더한다. 링크는 http(s) 만 싣는다.
// 결과는 renderer 가 sanitizeHtml 로 한 번 더 정화한 뒤에만 화면에 넣는다(이 변환기를 유일한 방어선으로 쓰지 않는다).

const MAX_DEPTH = 50
const MAX_OUTPUT = 1_000_000

interface AdfMark {
  type?: string
  attrs?: Record<string, unknown>
}

interface AdfNode {
  type?: string
  text?: string
  attrs?: Record<string, unknown>
  marks?: AdfMark[]
  content?: AdfNode[]
}

const esc = (s: string): string => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')

const str = (v: unknown): string => (typeof v === 'string' ? v : '')

/** http(s) 절대 주소만 통과시킨다 */
function safeHref(raw: unknown): string | null {
  if (typeof raw !== 'string') return null
  try {
    const u = new URL(raw)
    return u.protocol === 'https:' || u.protocol === 'http:' ? u.href : null
  } catch {
    return null
  }
}

class Budget {
  size = 0
}

function renderChildren(n: AdfNode, depth: number, b: Budget): string {
  if (!Array.isArray(n.content)) return ''
  let out = ''
  for (const c of n.content) {
    if (b.size > MAX_OUTPUT) break
    out += render(c, depth + 1, b)
  }
  return out
}

function applyMarks(html: string, marks: AdfMark[] | undefined): string {
  let out = html
  for (const m of Array.isArray(marks) ? marks : []) {
    if (!m || typeof m !== 'object') continue
    switch (m.type) {
      case 'strong':
        out = `<strong>${out}</strong>`
        break
      case 'em':
        out = `<em>${out}</em>`
        break
      case 'code':
        out = `<code>${out}</code>`
        break
      case 'strike':
        out = `<s>${out}</s>`
        break
      case 'underline':
        out = `<u>${out}</u>`
        break
      case 'subsup':
        out = m.attrs?.['type'] === 'sub' ? `<sub>${out}</sub>` : `<sup>${out}</sup>`
        break
      case 'link': {
        const href = safeHref(m.attrs?.['href'])
        if (href) out = `<a href="${esc(href)}">${out}</a>`
        break
      }
      default:
        break
    }
  }
  return out
}

function render(n: AdfNode, depth: number, b: Budget): string {
  if (!n || typeof n !== 'object' || depth > MAX_DEPTH) return ''
  let out: string
  switch (n.type) {
    case 'text':
      out = applyMarks(esc(str(n.text)), n.marks)
      break
    case 'hardBreak':
      out = '<br>'
      break
    case 'paragraph':
      out = `<p>${renderChildren(n, depth, b)}</p>`
      break
    case 'heading': {
      const lv = Number(n.attrs?.['level'])
      const h = Number.isInteger(lv) && lv >= 1 && lv <= 6 ? lv : 3
      out = `<h${h}>${renderChildren(n, depth, b)}</h${h}>`
      break
    }
    case 'bulletList':
      out = `<ul>${renderChildren(n, depth, b)}</ul>`
      break
    case 'orderedList': {
      const start = Number(n.attrs?.['order'])
      out = `<ol${Number.isInteger(start) && start > 1 && start < 1e6 ? ` start="${start}"` : ''}>${renderChildren(n, depth, b)}</ol>`
      break
    }
    case 'listItem':
      out = `<li>${renderChildren(n, depth, b)}</li>`
      break
    case 'taskList':
      out = `<ul>${renderChildren(n, depth, b)}</ul>`
      break
    case 'taskItem':
      out = `<li>${n.attrs?.['state'] === 'DONE' ? '☑' : '☐'} ${renderChildren(n, depth, b)}</li>`
      break
    case 'blockquote':
      out = `<blockquote>${renderChildren(n, depth, b)}</blockquote>`
      break
    case 'panel':
      out = `<blockquote>${renderChildren(n, depth, b)}</blockquote>`
      break
    case 'codeBlock':
      out = `<pre><code>${esc((Array.isArray(n.content) ? n.content : []).map((c) => str(c?.text)).join(''))}</code></pre>`
      break
    case 'rule':
      out = '<hr>'
      break
    case 'table':
      out = `<table><tbody>${renderChildren(n, depth, b)}</tbody></table>`
      break
    case 'tableRow':
      out = `<tr>${renderChildren(n, depth, b)}</tr>`
      break
    case 'tableHeader':
      out = `<th>${renderChildren(n, depth, b)}</th>`
      break
    case 'tableCell':
      out = `<td>${renderChildren(n, depth, b)}</td>`
      break
    case 'expand':
    case 'nestedExpand': {
      const title = str(n.attrs?.['title'])
      out = `<details>${title ? `<summary>${esc(title)}</summary>` : ''}${renderChildren(n, depth, b)}</details>`
      break
    }
    case 'mention':
      out = `<strong>${esc(str(n.attrs?.['text']) || '@user')}</strong>`
      break
    case 'emoji':
      out = esc(str(n.attrs?.['text']) || str(n.attrs?.['shortName']))
      break
    case 'status':
      out = `<mark>${esc(str(n.attrs?.['text']))}</mark>`
      break
    case 'date': {
      const ms = Number(n.attrs?.['timestamp'])
      // 범위를 벗어난 값은 Invalid Date 가 되어 toISOString 이 RangeError 를 던진다
      const day = new Date(ms)
      out = Number.isFinite(ms) && !Number.isNaN(day.getTime()) ? esc(day.toISOString().slice(0, 10)) : ''
      break
    }
    case 'inlineCard':
    case 'blockCard': {
      const href = safeHref(n.attrs?.['url'])
      out = href ? `<a href="${esc(href)}">${esc(href)}</a>` : ''
      break
    }
    case 'mediaSingle':
    case 'mediaGroup':
    case 'media':
      // 첨부 이미지는 인증이 필요하고 CSP 상 불러올 수도 없다 — 자리만 표시한다
      out = n.type === 'media' ? '<em>[첨부]</em> ' : renderChildren(n, depth, b)
      break
    default:
      out = renderChildren(n, depth, b)
  }
  b.size += out.length
  return out
}

/** 변환이 실패했을 때의 대체: 문서 안의 text 를 모아 문단 하나로(깊이·길이 상한 적용) */
function plainFallback(node: unknown, depth = 0, acc: string[] = []): string {
  if (depth > MAX_DEPTH || !node || typeof node !== 'object') return ''
  const n = node as AdfNode
  if (typeof n.text === 'string') acc.push(n.text)
  if (Array.isArray(n.content)) for (const c of n.content) plainFallback(c, depth + 1, acc)
  if (depth > 0) return ''
  const text = acc.join(' ').slice(0, 100_000)
  return text.trim() ? `<p>${esc(text)}</p>` : ''
}

/** ADF 문서(또는 노드)를 HTML 로. 해석할 수 없는 입력은 빈 문자열, 변환 중 예외가 나면 평문으로 대체 */
export function adfToHtml(doc: unknown): string {
  if (!doc || typeof doc !== 'object') return ''
  try {
    const html = render(doc as AdfNode, 0, new Budget())
    return html.length > MAX_OUTPUT ? html.slice(0, MAX_OUTPUT) : html
  } catch {
    try {
      return plainFallback(doc)
    } catch {
      return ''
    }
  }
}
