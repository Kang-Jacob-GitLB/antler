// @vitest-environment happy-dom
/// <reference lib="dom" />
import { describe, expect, it } from 'vitest'
import { findAnchor, highlightCodeBlocks, parseRepoUrl, renderMarkdownDoc, repoBase, resolveFenceLang, slugify, slugger } from '../src/renderer/src/lib/readme'

const html = (src: string, dir = ''): HTMLElement => {
  const el = document.createElement('div')
  el.innerHTML = renderMarkdownDoc(src, dir)
  return el
}

describe('renderMarkdownDoc', () => {
  it('코드펜스의 언어를 <pre lang> 으로 보존한다', () => {
    const el = html('```ts\nconst a = 1 < 2\n```\n\n```\nplain\n```')
    const pres = el.querySelectorAll('pre')
    expect(pres[0]!.getAttribute('lang')).toBe('ts')
    expect(pres[0]!.querySelector('code')!.textContent).toBe('const a = 1 < 2')
    expect(pres[1]!.hasAttribute('lang')).toBe(false)
  })

  it('한 줄 개행은 합친다(GitHub 문서는 breaks: false)', () => {
    expect(renderMarkdownDoc('a\nb', '')).not.toContain('<br')
  })

  it('상대 이미지는 가짜 접두사 경로로 풀어 src 를 보류한다', () => {
    const el = html('![logo](./img/a%20b.png) ![up](../top.png)', 'docs/guide')
    const [a, b] = [...el.querySelectorAll('img')]
    expect(a!.getAttribute('data-repo-src')).toBe('docs/guide/img/a b.png')
    expect(a!.hasAttribute('src')).toBe(false)
    expect(b!.getAttribute('data-repo-src')).toBe('docs/top.png')
  })

  it('외부 https 이미지는 그대로 둔다', () => {
    const el = html('![x](https://example.com/x.png)')
    expect(el.querySelector('img')!.getAttribute('src')).toBe('https://example.com/x.png')
    expect(el.querySelector('img')!.hasAttribute('data-repo-src')).toBe(false)
  })

  it('상대 링크는 가짜 접두사 data-href 로 풀리고 외부 링크는 그대로다', () => {
    const el = html('[a](sub/file.md#intro) [b](/root.md) [c](https://example.com/p) [d](#top) [e](한글 폴더/x.md)', 'docs')
    const hrefs = [...el.querySelectorAll('a[data-href]')].map((a) => a.getAttribute('data-href')!)
    expect(parseRepoUrl(hrefs[0]!)).toEqual({ path: 'docs/sub/file.md', hash: 'intro' })
    expect(parseRepoUrl(hrefs[1]!)).toEqual({ path: 'root.md', hash: '' })
    expect(hrefs[2]).toBe('https://example.com/p')
    expect(parseRepoUrl(hrefs[3]!)).toEqual({ path: 'docs', hash: 'top' })
  })

  it('스크립트·이벤트 속성·javascript: 를 제거한다', () => {
    const el = html('x <script>alert(1)</script> <img src="a.png" onerror="alert(1)"> [bad](javascript:alert(1))\n\n<div onclick="x()">d</div>')
    const out = el.innerHTML
    expect(out).not.toMatch(/<script|onerror|onclick|javascript:/)
    expect(el.querySelector('a')?.hasAttribute('href') ?? false).toBe(false)
  })

  it('제목에 slug id 와 앵커를 붙이고 중복은 -1, -2 를 붙인다', () => {
    const el = html('# Hello, World!\n\n## 시작하기\n\n## Hello, World!\n\n### Hello, World!\n\n## a  b')
    const ids = [...el.querySelectorAll('h1,h2,h3')].map((h) => h.id)
    expect(ids).toEqual(['user-content-hello-world', 'user-content-시작하기', 'user-content-hello-world-1', 'user-content-hello-world-2', 'user-content-a--b'])
    expect(el.querySelector('#user-content-hello-world > a.anchor')!.getAttribute('data-anchor')).toBe('hello-world')
  })

  it('findAnchor 는 접두사 유무·대소문자와 무관하게 제목을 찾는다', () => {
    const el = html('# Hello, World!\n\n## 시작하기')
    const h1 = el.querySelector('h1')
    expect(findAnchor(el, 'hello-world')).toBe(h1)
    expect(findAnchor(el, 'user-content-hello-world')).toBe(h1)
    expect(findAnchor(el, 'Hello-World')).toBe(h1)
    expect(findAnchor(el, '시작하기')).toBe(el.querySelector('h2'))
    expect(findAnchor(el, 'nope')).toBeNull()
  })

  it('외부 링크에는 실제 주소를 title 로 달고, 작성자 title 이 있으면 앞에 둔다', () => {
    const el = html('[a](https://example.com/p) [b](https://example.com/q "설명") [c](sub/x.md)')
    const [a, b, c] = [...el.querySelectorAll('a[data-href]')]
    expect(a!.getAttribute('title')).toBe('https://example.com/p')
    expect(b!.getAttribute('title')).toBe('설명\nhttps://example.com/q')
    expect(c!.hasAttribute('title')).toBe(false)
  })

  it('작업 목록에 표시 클래스를 단다', () => {
    const el = html('- [x] done\n- [ ] todo\n- plain')
    expect(el.querySelectorAll('li.task-list-item').length).toBe(2)
    expect(el.querySelector('ul.contains-task-list')).not.toBeNull()
    expect(el.querySelector('input[type=checkbox]')!.hasAttribute('disabled')).toBe(true)
  })
})

describe('slug', () => {
  it('GitHub 규칙: 소문자·특수문자 제거·공백은 하이픈', () => {
    expect(slugify('API (v2) / Notes')).toBe('api-v2--notes')
    expect(slugify('  Foo_Bar-baz  ')).toBe('foo_bar-baz')
    expect(slugify('한글 제목 123')).toBe('한글-제목-123')
    expect(slugify('🚀')).toBe('')
  })

  it('발급기는 중복에 -1, -2 를 붙이고 이미 있는 이름과도 겹치지 않는다', () => {
    const s = slugger()
    expect([s('A'), s('A'), s('A'), s('a-1'), s('A')]).toEqual(['a', 'a-1', 'a-2', 'a-1-1', 'a-3'])
  })
})

describe('repo url', () => {
  it('repoBase 는 디렉터리를 인코딩하고 parseRepoUrl 이 되돌린다', () => {
    expect(repoBase('')).toBe('https://repo.antler.invalid/')
    const u = new URL('x y.md#h%20z', repoBase('한글/dir')).href
    expect(parseRepoUrl(u)).toEqual({ path: '한글/dir/x y.md', hash: 'h z' })
    expect(parseRepoUrl('https://example.com/a')).toBeNull()
  })
})

describe('highlightCodeBlocks', () => {
  it('언어를 아는 블록은 강조 span 으로 바꾸고 텍스트는 보존한다', async () => {
    const el = html('```js\nconst x = "<b>"\n```\n\n```unknownlang\n<i>raw</i>\n```')
    await highlightCodeBlocks(el)
    const [a, b] = [...el.querySelectorAll('pre > code')]
    expect(a!.querySelector('.hljs-keyword')).not.toBeNull()
    expect(a!.textContent).toBe('const x = "<b>"')
    expect(a!.querySelector('b')).toBeNull()
    expect(b!.textContent).toBe('<i>raw</i>')
    expect(b!.querySelector('i')).toBeNull()
  })

  it('펜스 언어 별칭을 푼다', () => {
    expect(resolveFenceLang('ts')).toBe('typescript')
    expect(resolveFenceLang('shell')).toBe('bash')
    expect(resolveFenceLang('')).toBeNull()
  })
})
