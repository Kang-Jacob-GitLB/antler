#!/usr/bin/env node
// 빌드된 앱(out/)을 Playwright 로 띄워 시나리오를 돌리고 스크린샷·측정치를 남긴다.
// 실사용 설정을 건드리지 않도록 임시 userData 를 쓴다.
//
// 사용: node scripts/drive.mjs <저장소 경로...> [--out <스크린샷 디렉터리>] [--theme dark|light] [--locale ko|en]
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { _electron as electron } from 'playwright-core'
import { createRequire } from 'node:module'

const APP_DIR = resolve(import.meta.dirname, '..')
const args = process.argv.slice(2)
const opt = (name, def) => {
  const i = args.indexOf(`--${name}`)
  if (i < 0) return def
  const v = args[i + 1]
  args.splice(i, 2)
  return v
}
const outDir = resolve(opt('out', join(tmpdir(), 'antler-shots')))
const theme = opt('theme', 'dark')
const locale = opt('locale', 'ko')
const searchQuery = opt('search', 'Merge branch')
const repos = args.map((p) => resolve(p))
mkdirSync(outDir, { recursive: true })

const userData = mkdtempSync(join(tmpdir(), 'antler-userdata-'))
writeFileSync(
  join(userData, 'antler.config.json'),
  JSON.stringify({
    version: 1,
    sources: [{ id: 'local', kind: 'local', label: 'Local' }],
    repos: repos.map((p, i) => ({ id: `r${i}`, sourceId: 'local', path: p, name: basename(p) })),
    ui: { theme, locale, paneWidths: [248, 380], lastRepoId: repos.length ? 'r0' : undefined }
  })
)

const t0 = Date.now()
const app = await electron.launch({
  // require('electron') 는 실행 파일 경로를 돌려주며, 바이너리가 없으면 그때 내려받는다(Electron 44+)
  executablePath: createRequire(import.meta.url)('electron'),
  args: [APP_DIR],
  env: { ...process.env, ANTLER_USER_DATA: userData },
  timeout: 30_000
})
const page = await app.firstWindow()
const shot = async (name) => {
  const f = join(outDir, `${name}.png`)
  await page.screenshot({ path: f })
  console.log('screenshot:', f)
}

try {
  await page.setViewportSize({ width: 1440, height: 860 }).catch(() => {})
  if (!repos.length) {
    await page.waitForSelector('.empty-state', { timeout: 15_000 })
    console.log(`cold start → empty state: ${Date.now() - t0} ms`)
    await shot(`empty-${theme}`)
  } else {
    await page.waitForSelector('.commit-subject', { timeout: 15_000 })
    console.log(`cold start → first commit row: ${Date.now() - t0} ms`)
    await page.waitForTimeout(300)
    await shot(`list-${theme}`)

    // 사이드바 ref 섹션이 채워질 때까지
    const tRefs = Date.now()
    await page.waitForSelector('.side-section:nth-of-type(2)', { timeout: 20_000 }).catch(() => {})
    const sections = await page.$$eval('.side-section__head', (els) => els.map((e) => e.textContent?.trim()))
    console.log(`sidebar sections (+${Date.now() - tRefs} ms): ${sections.join(' | ')}`)
    await page.waitForTimeout(300)
    await shot(`graph-${theme}`)

    // 브랜치 클릭 → 그래프에서 그 커밋으로 이동
    const branch = await page.$$eval('.side-section:nth-of-type(2) .side-item:not(.side-item--folder)', (els) =>
      els.map((e) => e.textContent?.trim()).filter(Boolean)
    )
    const target = branch.at(-1)
    if (target) {
      await page.click(`.side-section:nth-of-type(2) .side-item:not(.side-item--folder) >> nth=-1`)
      await page.waitForTimeout(600)
      const chip = await page.textContent('.commit-row.is-selected .cell--refs').catch(() => null)
      console.log(`reveal branch "${target}": selected row refs = "${chip}"`)
      await shot(`reveal-${theme}`)

      // 더블클릭 → 해당 ref 만 보기
      await page.dblclick(`.side-section:nth-of-type(2) .side-item:not(.side-item--folder) >> nth=-1`)
      await page.waitForSelector('.filter-chip', { timeout: 5_000 })
      await page.waitForTimeout(500)
      console.log(`filter chip: "${await page.textContent('.filter-chip')}", rows: ${await page.textContent('.commit-list__foot')}`)
      await shot(`filter-${theme}`)
      await page.click('.filter-chip')
      await page.waitForSelector('.filter-chip', { state: 'detached' })
      await page.waitForSelector('.commit-subject')
    }

    // 커밋 상세 → 변경 파일 → diff (M3)
    await page.click('.commit-list__scroll')
    await page.keyboard.press('Home')
    const tDetail = Date.now()
    await page.waitForSelector('.file-row', { timeout: 10_000 }).catch(() => {})
    const files = await page.$$eval('.file-row', (els) => els.length)
    console.log(`detail files: ${files} (+${Date.now() - tDetail} ms) — ${await page.textContent('.file-list__head').catch(() => '')}`)
    if (files) {
      // 가장 많이 바뀐 파일을 연다(강조·줄 안 변경을 보기 좋게)
      await page.$$eval('.file-row', (els) => {
        const score = (e) => [...e.querySelectorAll('.file-row__stat span')].reduce((n, s) => n + Number(s.textContent.replace(/[^0-9]/g, '')), 0)
        els.sort((a, b) => score(b) - score(a))[0].click()
      })
      const tDiff = Date.now()
      await page.waitForSelector('.diff-row, .diff-notice', { timeout: 10_000 })
      await page.waitForTimeout(500)
      const rows = await page.$$eval('.diff-row', (els) => els.length)
      const hl = await page.$$eval('.diff-code [class^="hljs-"]', (els) => els.length)
      console.log(`diff "${await page.textContent('.diff-head__base')}": ${rows} rows rendered, ${hl} highlighted tokens (+${Date.now() - tDiff} ms)`)
      await shot(`diff-unified-${theme}`)
      await page.click('.seg__btn:nth-child(2)')
      await page.waitForSelector('.diff-row--split')
      await page.waitForTimeout(300)
      await shot(`diff-split-${theme}`)
      await page.click('.seg__btn:nth-child(1)')
      await page.keyboard.press('Alt+ArrowDown')
      await page.waitForTimeout(400)
      console.log(`Alt+↓ → "${await page.textContent('.diff-head__base')}" (${await page.textContent('.diff-head__pos')})`)
      await page.keyboard.press('Escape')
      await page.waitForSelector('.diff-view', { state: 'detached' })
      console.log('Esc → diff closed, graph visible:', await page.isVisible('.commit-list'))
    }

    // Ctrl+F 검색
    await page.keyboard.press('Control+F')
    await page.waitForSelector('.search-bar__input')
    await page.fill('.search-bar__input', searchQuery)
    await page.waitForTimeout(1200)
    console.log(`search "${searchQuery}": ${await page.textContent('.search-bar__status')}`)
    await page.keyboard.press('Enter')
    await page.waitForTimeout(300)
    await shot(`search-${theme}`)
    await page.keyboard.press('Escape')

    // 키보드 ↓ 이동과 상세 패널 반영 확인
    await page.click('.commit-list__scroll')
    await page.keyboard.press('Home')
    for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowDown')
    const subject = await page.textContent('.detail__subject')
    const selected = await page.textContent('.commit-row.is-selected .commit-subject')
    console.log(`↓×3 selected: "${selected}" / detail: "${subject}" → ${selected === subject ? 'OK' : 'MISMATCH'}`)

    // 스크롤 프레임 측정: 일정 속도로 끝까지 스크롤하며 rAF 간격을 잰다
    const perf = await page.evaluate(async () => {
      const el = document.querySelector('.commit-list__scroll')
      const frames = []
      let last = performance.now()
      const until = performance.now() + 4000
      await new Promise((done) => {
        const step = (now) => {
          frames.push(now - last)
          last = now
          el.scrollTop += 600
          if (now < until) requestAnimationFrame(step)
          else done()
        }
        requestAnimationFrame(step)
      })
      frames.shift()
      frames.sort((a, b) => a - b)
      const avg = frames.reduce((s, x) => s + x, 0) / frames.length
      return {
        frames: frames.length,
        avgMs: +avg.toFixed(2),
        p95Ms: +frames[Math.floor(frames.length * 0.95)].toFixed(2),
        maxMs: +frames.at(-1).toFixed(2),
        rows: document.querySelector('.commit-list__foot')?.textContent
      }
    })
    console.log('scroll perf:', JSON.stringify(perf))
    await shot(`scrolled-${theme}`)

    // Ctrl+P 저장소 전환기
    await page.keyboard.press('Control+P')
    await page.waitForSelector('.switcher')
    await shot(`switcher-${theme}`)
    await page.keyboard.press('Escape')
  }
  // 유휴 상태 메모리: 잠시 쉰 뒤 프로세스별 working set / private bytes(KB → MB)
  await page.waitForTimeout(1500)
  const mem = await app.evaluate(async ({ app }) =>
    app.getAppMetrics().map((m) => ({ type: m.type, ws: m.memory.workingSetSize, priv: m.memory.privateBytes ?? 0 }))
  )
  const mb = (kb) => Math.round(kb / 1024)
  console.log(
    `memory: working set ${mb(mem.reduce((s, m) => s + m.ws, 0))} MB, private ${mb(mem.reduce((s, m) => s + m.priv, 0))} MB — ` +
      mem.map((m) => `${m.type} ${mb(m.ws)}/${mb(m.priv)}`).join(', ')
  )
} finally {
  await app.close()
  rmSync(userData, { recursive: true, force: true })
}
