// @vitest-environment happy-dom
/// <reference lib="dom" />
import { describe, expect, it } from 'vitest'
import { markdownToHtml } from '../src/renderer/src/lib/markdown'
import { sanitizeHtml } from '../src/renderer/src/lib/sanitize'

const render = (s: string): string => sanitizeHtml(markdownToHtml(s))

describe('markdownToHtml + sanitizeHtml', () => {
  it('제목·목록·코드를 렌더링한다', () => {
    const out = render('## 요약\n\n- 하나\n- **둘**\n\n`code` 와\n\n```\nblock\n```')
    expect(out).toContain('<h2>요약</h2>')
    expect(out).toContain('<li>하나</li>')
    expect(out).toContain('<strong>둘</strong>')
    expect(out).toContain('<code>code</code>')
    expect(out).toContain('<pre><code>block')
  })

  it('한 줄 개행은 <br> 로 유지한다', () => {
    expect(render('a\nb')).toContain('a<br>b')
  })

  it('<script> 와 javascript: 링크를 제거한다', () => {
    const out = render('x <script>alert(1)</script>\n\n[bad](javascript:alert(1)) [ok](https://github.com/a/b)')
    expect(out).not.toMatch(/script|javascript:|alert/)
    expect(out).toContain('data-href="https://github.com/a/b"')
    expect(out).not.toMatch(/ href=/)
  })
})
