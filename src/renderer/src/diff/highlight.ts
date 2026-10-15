// diff 를 열 때만 highlight.js core 와 필요한 언어를 지연 로드한다.
// 언어 모듈은 정적 import() 경로로 적어 두어 Vite 가 언어별 청크로 나눈다(첫 화면 번들에 들어가지 않는다).
import type { HLJSApi, LanguageFn } from 'highlight.js'

type Loader = () => Promise<{ default: LanguageFn }>

const LOADERS: Record<string, Loader> = {
  typescript: () => import('highlight.js/lib/languages/typescript'),
  javascript: () => import('highlight.js/lib/languages/javascript'),
  json: () => import('highlight.js/lib/languages/json'),
  css: () => import('highlight.js/lib/languages/css'),
  scss: () => import('highlight.js/lib/languages/scss'),
  xml: () => import('highlight.js/lib/languages/xml'),
  markdown: () => import('highlight.js/lib/languages/markdown'),
  python: () => import('highlight.js/lib/languages/python'),
  c: () => import('highlight.js/lib/languages/c'),
  cpp: () => import('highlight.js/lib/languages/cpp'),
  csharp: () => import('highlight.js/lib/languages/csharp'),
  java: () => import('highlight.js/lib/languages/java'),
  kotlin: () => import('highlight.js/lib/languages/kotlin'),
  go: () => import('highlight.js/lib/languages/go'),
  rust: () => import('highlight.js/lib/languages/rust'),
  ruby: () => import('highlight.js/lib/languages/ruby'),
  php: () => import('highlight.js/lib/languages/php'),
  swift: () => import('highlight.js/lib/languages/swift'),
  bash: () => import('highlight.js/lib/languages/bash'),
  powershell: () => import('highlight.js/lib/languages/powershell'),
  yaml: () => import('highlight.js/lib/languages/yaml'),
  ini: () => import('highlight.js/lib/languages/ini'),
  sql: () => import('highlight.js/lib/languages/sql'),
  dockerfile: () => import('highlight.js/lib/languages/dockerfile'),
  makefile: () => import('highlight.js/lib/languages/makefile'),
  cmake: () => import('highlight.js/lib/languages/cmake'),
  lua: () => import('highlight.js/lib/languages/lua'),
  dart: () => import('highlight.js/lib/languages/dart')
}

const BY_EXT: Record<string, string> = {
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  json: 'json', jsonc: 'json',
  css: 'css', scss: 'scss', less: 'scss',
  html: 'xml', htm: 'xml', xml: 'xml', svg: 'xml', vue: 'xml', xaml: 'xml', csproj: 'xml', props: 'xml',
  md: 'markdown', markdown: 'markdown',
  py: 'python', pyi: 'python',
  c: 'c', h: 'c',
  cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp', ino: 'cpp',
  cs: 'csharp',
  java: 'java', kt: 'kotlin', kts: 'kotlin',
  go: 'go', rs: 'rust', rb: 'ruby', php: 'php', swift: 'swift',
  sh: 'bash', bash: 'bash', zsh: 'bash',
  ps1: 'powershell', psm1: 'powershell',
  yml: 'yaml', yaml: 'yaml',
  ini: 'ini', toml: 'ini', cfg: 'ini', conf: 'ini',
  sql: 'sql', lua: 'lua', dart: 'dart', cmake: 'cmake'
}

const BY_NAME: Record<string, string> = {
  dockerfile: 'dockerfile',
  makefile: 'makefile',
  'cmakelists.txt': 'cmake',
  '.bashrc': 'bash',
  '.zshrc': 'bash'
}

/** 경로로 언어를 추정한다. 모르면 null(강조 없이 표시) */
export function languageFor(path: string): string | null {
  const base = path.split('/').pop()!.toLowerCase()
  if (BY_NAME[base]) return BY_NAME[base]
  const dot = base.lastIndexOf('.')
  if (dot < 0) return null
  return BY_EXT[base.slice(dot + 1)] ?? null
}

let core: Promise<HLJSApi> | null = null
const loaded = new Map<string, Promise<boolean>>()

/** 언어를 등록해 둔 highlight.js 를 돌려준다. 실패하면 null */
export async function loadHighlighter(lang: string): Promise<HLJSApi | null> {
  const loader = LOADERS[lang]
  if (!loader) return null
  core ??= import('highlight.js/lib/core').then((m) => m.default)
  const hljs = await core
  let ready = loaded.get(lang)
  if (!ready) {
    ready = loader()
      .then((m) => {
        hljs.registerLanguage(lang, m.default)
        return true
      })
      .catch(() => false)
    loaded.set(lang, ready)
  }
  return (await ready) ? hljs : null
}

/** 한 줄씩 강조한 HTML 캐시(같은 줄을 스크롤로 다시 그릴 때 재계산하지 않게) */
const cache = new Map<string, string>()
const CACHE_MAX = 20_000

/**
 * 한 줄을 강조한 HTML. highlight.js 는 텍스트를 이스케이프해 돌려주므로 innerHTML 로 넣어도 안전하다.
 * 줄 단위라 여러 줄에 걸친 문법(블록 주석 등)은 정확하지 않을 수 있다 — 속도와 맞바꾼 선택이다.
 */
export function highlightLine(hljs: HLJSApi, lang: string, text: string): string {
  const key = `${lang}\0${text}`
  let html = cache.get(key)
  if (html === undefined) {
    html = hljs.highlight(text, { language: lang, ignoreIllegals: true }).value
    if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value!)
    cache.set(key, html)
  }
  return html
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
}
