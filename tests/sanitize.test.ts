// @vitest-environment happy-dom
/// <reference lib="dom" />
// (renderer 의 sanitize 는 DOM API 를 쓴다 — node 쪽 타입 검사에서도 DOM 타입이 보이도록 이 파일에서만 참조한다)
import { describe, expect, it } from 'vitest'
import { sanitizeHtml } from '../src/renderer/src/lib/sanitize'

describe('sanitizeHtml', () => {
  it('스크립트·스타일·iframe 은 내용까지 제거한다', () => {
    const out = sanitizeHtml('<p>a</p><script>alert(1)</script><style>*{}</style><iframe src="https://x"></iframe><b>b</b>')
    expect(out).toBe('<p>a</p><b>b</b>')
  })

  it('이벤트 핸들러·style·class 속성을 지운다', () => {
    const out = sanitizeHtml('<p onclick="x()" style="color:red" class="c" title="t">hi</p><img src="https://a/x.png" onerror="x()">')
    expect(out).not.toMatch(/onclick|onerror|style=|class=/)
    expect(out).toContain('<p>hi</p>')
  })

  it('javascript:·data: 링크는 버리고 https 링크는 data-href 로 옮긴다', () => {
    const out = sanitizeHtml('<a href="javascript:alert(1)">x</a><a href="data:text/html,hi">y</a><a href="https://github.com/o/r/pull/1">z</a>')
    // data-href 가 아닌 진짜 href 속성은 남지 않는다
    expect(out).not.toMatch(/javascript:|data:text|(^|[^-])href="/)
    expect(out).toContain('data-href="https://github.com/o/r/pull/1"')
  })

  it('상대 링크는 github.com 기준으로 바꾼다', () => {
    expect(sanitizeHtml('<a href="/o/r/issues/2">#2</a>')).toContain('data-href="https://github.com/o/r/issues/2"')
  })

  it('http 이미지는 제거하고 https 이미지는 지연 로드·리퍼러 없이', () => {
    const out = sanitizeHtml('<img src="http://evil/x.png"><img src="https://user-images.githubusercontent.com/1.png" alt="a">')
    expect(out).not.toContain('http://evil')
    expect(out).toContain('loading="lazy"')
    expect(out).toContain('referrerpolicy="no-referrer"')
  })

  it('모르는 태그는 벗기고 내용만 남긴다', () => {
    expect(sanitizeHtml('<custom-el><em>x</em></custom-el>')).toBe('<em>x</em>')
  })

  it('작업 목록 체크박스는 남기되 비활성화하고, 다른 input 은 지운다', () => {
    const out = sanitizeHtml('<ul><li><input type="checkbox" checked> done</li></ul><input type="text" value="x">')
    expect(out).toMatch(/<input type="checkbox" checked="" disabled="">/)
    expect(out).not.toContain('type="text"')
  })

  it('svg 안의 스크립트 우회도 막는다', () => {
    const out = sanitizeHtml('<svg><script>alert(1)</script><a href="javascript:x">y</a></svg>ok')
    expect(out).not.toMatch(/script|javascript:|<svg/)
  })
})
