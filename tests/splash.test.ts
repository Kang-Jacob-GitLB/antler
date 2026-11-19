import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const read = (p: string): string => readFileSync(p, 'utf8')

describe('시작 로딩 창(splash.html)', () => {
  const html = read('src/renderer/splash.html')

  it('스크립트·외부 리소스 없이 정적이다', () => {
    expect(html).not.toMatch(/<script/i)
    expect(html).not.toMatch(/https?:\/\//i)
    expect(html).not.toMatch(/<(link|img|iframe|object|embed)\b/i)
    expect(html).not.toMatch(/\burl\(/i)
    expect(html).not.toMatch(/@import/i)
    expect(html).not.toMatch(/\son\w+=/i)
  })

  it('로고·이름·로딩 표시가 있다', () => {
    expect(html).toContain('<svg')
    expect(html).toContain('<h1>Antler</h1>')
    expect(html).toContain('@keyframes')
  })

  it('바탕색이 tokens.css 의 --bg-base 와 같다', () => {
    const css = read('src/renderer/src/styles/tokens.css')
    const dark = /:root \{[\s\S]*?--bg-base: (#[0-9A-Fa-f]{6})/.exec(css)?.[1]
    const light = /:root\[data-theme='light'\] \{[\s\S]*?--bg-base: (#[0-9A-Fa-f]{6})/.exec(css)?.[1]
    expect(html.toLowerCase()).toContain(`background: ${dark!.toLowerCase()}`)
    expect(html.toLowerCase()).toContain(`background: ${light!.toLowerCase()}`)
  })
})

describe('빌드 설정', () => {
  it('vite 입력에 splash.html 이 들어 있다', () => {
    const cfg = read('vite.config.mts')
    expect(cfg).toMatch(/input:\s*\{[^}]*index\.html[^}]*splash\.html[^}]*\}/)
  })
})

describe('첫 화면 신호', () => {
  it('main.tsx 가 FirstPaint 를 그리고, 어댑터가 Rust command 로 전한다', () => {
    expect(read('src/renderer/src/main.tsx')).toContain('<FirstPaint />')
    expect(read('src/renderer/tauri/bridge.ts')).toMatch(/antler:first-paint[\s\S]*antler_window_ready/)
    expect(read('src/renderer/src/components/FirstPaint.tsx')).toContain("'antler:first-paint'")
  })
})
