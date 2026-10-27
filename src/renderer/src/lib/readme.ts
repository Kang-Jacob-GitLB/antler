// 코드 탐색기의 저장소 문서(README·*.md)를 GitHub 처럼 렌더링한다.
//  - 커밋 메시지용 lib/markdown.ts 와 별개의 marked 인스턴스(breaks: false — GitHub 문서는 한 줄 개행을 합친다)
//  - sanitizer 는 넓히지 않고 우회한다: 코드펜스는 <pre lang> 으로 내보내고(lang 은 전역 허용 속성),
//    상대 경로는 가짜 https 접두사(REPO_ORIGIN)로 풀어 정화를 통과시킨 뒤 앱이 저장소 경로로 되돌린다
//  - 정화 뒤 한 번의 DOM 후처리: 제목 id·앵커, 작업 목록 표시, 저장소 이미지 src 보류(data-repo-src)
//  결과 HTML 은 정화를 거쳤으므로 dangerouslySetInnerHTML 로 넣어도 안전하다. 코드 강조·이미지 로드는 넣은 뒤 비동기로 한다.
import { Marked } from 'marked'
import { escapeHtml, languageFor, loadHighlighter } from '../diff/highlight'
import { sanitizeHtml } from './sanitize'

/** 상대 경로를 풀 때 쓰는 가짜 origin(.invalid 는 실제로 존재할 수 없는 최상위 도메인) */
export const REPO_ORIGIN = 'https://repo.antler.invalid'

const md = new Marked({
  gfm: true,
  breaks: false,
  async: false,
  renderer: {
    // 코드펜스를 <pre lang="언어"><code>…</code></pre> 로 — sanitizeHtml 이 class 를 지우므로 lang 으로 언어를 전한다
    code({ text, lang }) {
      const l = (lang ?? '').trim().split(/\s+/)[0] ?? ''
      return `<pre${l ? ` lang="${escapeHtml(l)}"` : ''}><code>${escapeHtml(text)}</code></pre>\n`
    }
  }
})

/** 이 디렉터리('' 이면 최상위) 안의 상대 경로를 풀 기준 주소 */
export function repoBase(dir: string): string {
  const segs = dir.split('/').filter(Boolean).map(encodeURIComponent)
  return segs.length ? `${REPO_ORIGIN}/${segs.join('/')}/` : `${REPO_ORIGIN}/`
}

export interface RepoUrl {
  /** 저장소 최상위 기준 경로('' = 최상위, 앞뒤 슬래시 없음) */
  path: string
  /** '#' 뒤(디코드). 없으면 '' */
  hash: string
}

const safeDecode = (s: string): string => {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

/** 가짜 접두사 주소를 저장소 경로로 되돌린다. 가짜 접두사가 아니면 null */
export function parseRepoUrl(href: string): RepoUrl | null {
  let u: URL
  try {
    u = new URL(href)
  } catch {
    return null
  }
  if (u.origin !== REPO_ORIGIN) return null
  const path = u.pathname.split('/').filter(Boolean).map(safeDecode).join('/')
  return { path, hash: safeDecode(u.hash.replace(/^#/, '')) }
}

/** GitHub 식 제목 slug: 소문자, 글자·숫자·`_`·`-`·공백 외 제거, 공백은 `-` */
export function slugify(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\p{M}_\- ]/gu, '')
    .replace(/ /g, '-')
}

/** 중복 slug 에 -1, -2 … 를 붙이는 발급기(한 문서에 하나) */
export function slugger(): (text: string) => string {
  const used = new Set<string>()
  return (text) => {
    const base = slugify(text)
    if (!base) return ''
    let slug = base
    for (let n = 1; used.has(slug); n++) slug = `${base}-${n}`
    used.add(slug)
    return slug
  }
}

const ANCHOR_SVG =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6.8 9.2a2.5 2.5 0 0 0 3.5 0l2-2a2.5 2.5 0 0 0-3.5-3.5l-.6.6M9.2 6.8a2.5 2.5 0 0 0-3.5 0l-2 2a2.5 2.5 0 0 0 3.5 3.5l.6-.6"/></svg>'

/** 정화된 HTML 에 제목 id·앵커, 작업 목록 표시, 저장소 이미지 src 보류를 더한다(앱이 만든 속성만 추가) */
function decorate(safeHtml: string): string {
  const doc = new DOMParser().parseFromString(`<!doctype html><body>${safeHtml}`, 'text/html')
  const root = doc.body
  const slug = slugger()
  for (const h of root.querySelectorAll('h1,h2,h3,h4,h5,h6')) {
    const id = slug(h.textContent ?? '')
    if (!id) continue
    h.setAttribute('id', id)
    const a = doc.createElement('a')
    a.className = 'anchor'
    a.setAttribute('data-anchor', id)
    a.setAttribute('aria-hidden', 'true')
    a.innerHTML = ANCHOR_SVG
    h.prepend(a)
  }
  for (const input of root.querySelectorAll('input[type="checkbox"]')) {
    const li = input.closest('li')
    if (!li) continue
    li.classList.add('task-list-item')
    li.parentElement?.classList.add('contains-task-list')
  }
  for (const img of root.querySelectorAll('img')) {
    const repo = parseRepoUrl(img.getAttribute('src') ?? '')
    if (!repo) continue
    // 가짜 주소로 네트워크 요청이 나가지 않게 src 를 걷어 두고, 경로만 남긴다
    img.setAttribute('data-repo-src', repo.path)
    img.removeAttribute('src')
    img.removeAttribute('loading')
  }
  return root.innerHTML
}

/** 마크다운 → 정화·후처리된 HTML. dir 은 이 문서가 있는 디렉터리('' = 최상위) — 상대 이미지·링크의 기준 */
export function renderMarkdownDoc(src: string, dir: string): string {
  return decorate(sanitizeHtml(md.parse(src) as string, repoBase(dir)))
}

// ── 넣은 뒤 DOM 후처리 ──────────────────────────────────

const LANG_ALIAS: Record<string, string> = {
  shell: 'bash', sh: 'bash', zsh: 'bash', console: 'bash', terminal: 'bash',
  'c++': 'cpp', 'c#': 'csharp', golang: 'go', html: 'xml', jsonc: 'json', json5: 'json',
  js: 'javascript', ts: 'typescript', py: 'python', rb: 'ruby', rs: 'rust', yml: 'yaml', ps: 'powershell', pwsh: 'powershell'
}

/** 펜스 언어 이름 → highlight.js 가 아는 언어(모르면 null) */
export function resolveFenceLang(raw: string | null): string | null {
  const l = (raw ?? '').trim().toLowerCase().split(/[\s{,]/)[0] ?? ''
  if (!l) return null
  return LANG_ALIAS[l] ?? languageFor(`x.${l}`) ?? l
}

/** 이보다 긴 코드블록은 강조하지 않는다(동기 강조가 UI 를 멈추지 않게) */
const HIGHLIGHT_BLOCK_MAX = 100_000

/** `pre[lang] > code` 를 블록 전체로 강조한다. 언어를 모르거나 불러오지 못하면 그대로 둔다. hljs 결과는 이미 이스케이프돼 있다 */
export async function highlightCodeBlocks(root: ParentNode, alive: () => boolean = () => true): Promise<void> {
  for (const code of [...root.querySelectorAll('pre[lang] > code')]) {
    const lang = resolveFenceLang(code.parentElement?.getAttribute('lang') ?? null)
    if (!lang) continue
    const hljs = await loadHighlighter(lang)
    if (!alive()) return
    const text = code.textContent ?? ''
    if (!hljs || text.length > HIGHLIGHT_BLOCK_MAX) continue
    try {
      code.innerHTML = hljs.highlight(text, { language: lang, ignoreIllegals: true }).value
    } catch {
      /* 강조에 실패하면 원문 그대로 */
    }
  }
}

/** 문서 안에서 id 로 제목을 찾는다(GitHub 의 user-content- 접두사·대소문자 차이를 허용) */
export function findAnchor(root: ParentNode, frag: string): Element | null {
  const want = frag.replace(/^user-content-/, '')
  const lower = want.toLowerCase()
  for (const el of root.querySelectorAll('[id]')) {
    if (el.id === want || el.id === lower) return el
  }
  return null
}
