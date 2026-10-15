#!/usr/bin/env node
// tokens.css 의 테마별 색 토큰으로 WCAG 대비를 검사한다.
// 사용: node scripts/check-contrast.mjs   (기준 미달이 있으면 exit 1)
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const css = readFileSync(resolve(import.meta.dirname, '../src/renderer/src/styles/tokens.css'), 'utf8')

/** 선택자 블록 안의 `--name: #hex` 선언을 모은다 */
function block(selector) {
  const start = css.indexOf(`${selector} {`)
  if (start < 0) throw new Error(`selector not found: ${selector}`)
  const body = css.slice(start, css.indexOf('\n}', start))
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*(#[0-9a-fA-F]{6})\b/g)].map((m) => [m[1], m[2]]))
}

const lum = (hex) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}
const ratio = (a, b) => {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

// [전경 토큰, 최소 대비] — 텍스트는 AA 4.5, 큰 글씨·아이콘·비텍스트 UI 는 3.0
const CHECKS = [
  ['--text-primary', 4.5],
  ['--text-secondary', 4.5],
  ['--text-muted', 4.5],
  ['--accent-text', 4.5],
  ['--added-text', 4.5],
  ['--removed-text', 4.5],
  ['--modified-text', 4.5],
  ['--renamed-text', 4.5],
  ['--velvet-text', 4.5],
  ['--syn-keyword', 4.5],
  ['--syn-string', 4.5],
  ['--syn-number', 4.5],
  ['--syn-comment', 4.5],
  ['--syn-function', 4.5],
  ['--syn-type', 4.5],
  ['--syn-attr', 4.5],
  ['--syn-meta', 4.5],
  ['--accent', 3.0],
  ['--added', 3.0],
  ['--removed', 3.0],
  ['--renamed', 3.0]
]
const BACKGROUNDS = ['--bg-base', '--bg-surface', '--bg-elevated']

const root = block(':root')
const themes = { dark: root, light: { ...root, ...block(":root[data-theme='light']") } }
let failed = 0
for (const [theme, t] of Object.entries(themes)) {
  console.log(`\n[${theme}]`)
  for (const [fg, min] of CHECKS) {
    if (!t[fg]) {
      console.log(`  ${fg.padEnd(18)} (정의 없음)`)
      failed++
      continue
    }
    const cells = BACKGROUNDS.map((bg) => {
      const r = ratio(t[fg], t[bg])
      if (r < min) failed++
      return `${bg.slice(5)} ${r.toFixed(2)}${r < min ? ' ✗' : ''}`
    })
    console.log(`  ${fg.padEnd(18)} ${t[fg]}  ≥${min}  ${cells.join('  ')}`)
  }

  // 그래프 노드 이니셜: 레인 색 위 글자(어두운 글자·흰 글자 중 큰 쪽)가 AA 4.5 를 넘는지.
  // 둘 다 모자라면 renderer(graph/nodeStyle.ts nodeColors)가 채움색을 밀어 맞추므로 보정 후 값을 함께 검사한다
  const rgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16))
  const ratioRgb = (a, b) => {
    const l = (c) => lum('#' + c.map((v) => Math.round(v).toString(16).padStart(2, '0')).join(''))
    const [hi, lo] = [l(a), l(b)].sort((x, y) => y - x)
    return (hi + 0.05) / (lo + 0.05)
  }
  const DARK = [0x12, 0x0f, 0x0d]
  const WHITE = [255, 255, 255]
  const notes = []
  for (let i = 0; i < 8; i++) {
    const lane = t[`--lane-${i}`]
    if (!lane) continue
    const c = rgb(lane)
    const [rd, rw] = [ratioRgb(c, DARK), ratioRgb(c, WHITE)]
    const text = rd >= rw ? DARK : WHITE
    let best = Math.max(rd, rw)
    let tag = rd >= rw ? '어두운 글자' : '흰 글자'
    if (best < 4.5) {
      const target = text === DARK ? WHITE : DARK
      for (let k = 1; k <= 20 && best < 4.5; k++) {
        const m = c.map((v, j) => v + (target[j] - v) * (k / 20))
        best = ratioRgb(m, text)
        tag += ` (채움 ${k * 5}% 보정)`
      }
    }
    if (best < 4.5) failed++
    notes.push(`lane-${i} ${best.toFixed(2)} ${tag}${best < 4.5 ? ' ✗' : ''}`)
  }
  console.log(`  그래프 노드 글자: ${notes.join(' | ')}`)
}
console.log(failed ?`\n${failed}건 기준 미달` : '\n모든 대비 기준 통과')
process.exit(failed ? 1 : 0)
