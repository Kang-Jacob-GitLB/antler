// @vitest-environment happy-dom
/// <reference lib="dom" />
// (renderer 의 sanitize 는 DOM API 를 쓴다 — node 쪽 타입 검사에서도 DOM 타입이 보이도록 이 파일에서만 참조한다)
import { describe, expect, it } from 'vitest'
import { adfToHtml } from '../src/main/jira/adf'
import { sanitizeHtml } from '../src/renderer/src/lib/sanitize'
import { extractIssueKeys, parseProjectKeys } from '../src/renderer/src/lib/issueKeys'

const text = (t: string, marks?: unknown[]): unknown => ({ type: 'text', text: t, ...(marks ? { marks } : {}) })
const doc = (...content: unknown[]): unknown => ({ type: 'doc', version: 1, content })

describe('adfToHtml', () => {
  it('문단·제목·목록·코드·인용·표를 변환한다', () => {
    const html = adfToHtml(
      doc(
        { type: 'heading', attrs: { level: 2 }, content: [text('제목')] },
        { type: 'paragraph', content: [text('a'), { type: 'hardBreak' }, text('b', [{ type: 'em' }, { type: 'code' }])] },
        { type: 'bulletList', content: [{ type: 'listItem', content: [{ type: 'paragraph', content: [text('x')] }] }] },
        { type: 'orderedList', attrs: { order: 3 }, content: [{ type: 'listItem', content: [{ type: 'paragraph', content: [text('y')] }] }] },
        { type: 'codeBlock', content: [text('if (a < b) {}')] },
        { type: 'blockquote', content: [{ type: 'paragraph', content: [text('q')] }] },
        { type: 'table', content: [{ type: 'tableRow', content: [{ type: 'tableHeader', content: [{ type: 'paragraph', content: [text('h')] }] }, { type: 'tableCell', content: [{ type: 'paragraph', content: [text('c')] }] }] }] },
        { type: 'rule' }
      )
    )
    expect(html).toContain('<h2>제목</h2>')
    expect(html).toContain('<p>a<br><code><em>b</em></code></p>')
    expect(html).toContain('<ul><li><p>x</p></li></ul>')
    expect(html).toContain('<ol start="3"><li><p>y</p></li></ol>')
    expect(html).toContain('<pre><code>if (a &lt; b) {}</code></pre>')
    expect(html).toContain('<table><tbody><tr><th><p>h</p></th><td><p>c</p></td></tr></tbody></table>')
    expect(html).toContain('<hr>')
  })

  it('텍스트·속성은 이스케이프하고 http(s) 가 아닌 링크는 싣지 않는다', () => {
    const html = adfToHtml(
      doc({
        type: 'paragraph',
        content: [
          text('<script>alert(1)</script>'),
          text('js', [{ type: 'link', attrs: { href: 'javascript:alert(1)' } }]),
          text('data', [{ type: 'link', attrs: { href: 'data:text/html,<script>x</script>' } }]),
          text('ok', [{ type: 'link', attrs: { href: 'https://x.atlassian.net/a?b="1"&c=2' } }]),
          { type: 'mention', attrs: { text: '@<img src=x onerror=1>' } },
          { type: 'heading', attrs: { level: '1><script>' }, content: [text('h')] }
        ]
      })
    )
    expect(html).not.toContain('<script')
    expect(html).not.toContain('<img')
    expect(html).not.toMatch(/javascript:|data:text/)
    expect(html).toContain('<a href="https://x.atlassian.net/a?b=%221%22&amp;c=2">ok</a>')
  })

  it('정화 후에도 스크립트·javascript: 링크가 남지 않는다', () => {
    const html = sanitizeHtml(
      adfToHtml(
        doc({
          type: 'paragraph',
          content: [text('<script>alert(1)</script>안녕'), text('x', [{ type: 'link', attrs: { href: 'javascript:alert(1)' } }]), text('y', [{ type: 'link', attrs: { href: '/browse/ABC-2' } }])]
        })
      ),
      'https://x.atlassian.net/'
    )
    expect(html).not.toMatch(/<script|javascript:|onerror/)
    expect(html).toContain('안녕')
  })

  it('서버가 준 HTML(Data Center)도 정화하면 위험 요소가 사라지고 상대 링크는 Jira 사이트 기준이 된다', () => {
    const html = sanitizeHtml(
      '<p onclick="x()">a<script>1</script></p><a href="javascript:alert(1)">j</a><a href="/browse/ABC-2">k</a><img src="http://evil/x.png">',
      'https://jira.example.com/'
    )
    expect(html).not.toMatch(/<script|javascript:|onclick|evil/)
    expect(html).toContain('data-href="https://jira.example.com/browse/ABC-2"')
  })

  it('깊이·크기 상한: 아주 깊은 문서도 멈추고, 이상한 입력은 빈 문자열', () => {
    let deep: unknown = text('끝')
    for (let i = 0; i < 5000; i++) deep = { type: 'blockquote', content: [deep] }
    expect(() => adfToHtml(deep)).not.toThrow()
    expect(adfToHtml(deep)).not.toContain('끝')
    expect(adfToHtml(null)).toBe('')
    expect(adfToHtml('str')).toBe('')
    expect(adfToHtml({ type: 'doc' })).toBe('')
    expect(adfToHtml(doc({ type: 'unknownNode', content: [{ type: 'paragraph', content: [text('자식만')] }] }))).toBe('<p>자식만</p>')
  })
})

describe('extractIssueKeys', () => {
  it('일반 패턴: 제목·본문에서 등장 순서대로, 중복 제거', () => {
    expect(extractIssueKeys(['ABC-123: 로그인 수정 (see DEF-4)', 'Fixes ABC-123, refs XY_Z-5'])).toEqual(['ABC-123', 'DEF-4', 'XY_Z-5'])
    expect(extractIssueKeys(['[ABC-1] fix', undefined, 'feature/ABC-2-login'])).toEqual(['ABC-1', 'ABC-2'])
  })

  it('이슈 키가 아닌 흔한 토큰은 제외한다', () => {
    expect(extractIssueKeys(['UTF-8 인코딩, SHA-256 해시, CVE-2024-1234, ISO-8601, GH-12, foo-ABC-1, ABC-1x, abc-12, A-1'])).toEqual([])
  })

  it('프로젝트 키를 설정하면 그 키만(제외 목록도 풀린다)', () => {
    expect(extractIssueKeys(['ABC-1 DEF-2 GH-3'], ['ABC', 'GH'])).toEqual(['ABC-1', 'GH-3'])
  })

  it('최대 10개', () => {
    const many = Array.from({ length: 30 }, (_, i) => `ABC-${i + 1}`).join(' ')
    expect(extractIssueKeys([many])).toHaveLength(10)
  })

  it('프로젝트 키 입력 파싱', () => {
    expect(parseProjectKeys('abc, DEF;ghi  abc 1x bad-key')).toEqual(['ABC', 'DEF', 'GHI'])
    expect(parseProjectKeys('')).toEqual([])
  })
})
