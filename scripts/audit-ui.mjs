#!/usr/bin/env node
// UI 컨트롤 크기 감사: 별도 빌드(out/audit-build)를 임시 저장소·임시 userData 로 띄워
// 화면별 상호작용 요소의 높이·폰트·반경을 재고 스크린샷을 남긴다. dev 서버의 out/ 을 건드리지 않는다.
//
// 빌드: npx electron-vite build --outDir out/audit-build
// 사용: node scripts/audit-ui.mjs --out out/design-before [--theme dark|light] [--build out/audit-build] [--only update]
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { _electron as electron } from 'playwright-core'
import { createRequire } from 'node:module'

const ROOT = resolve(import.meta.dirname, '..')
const args = process.argv.slice(2)
const opt = (n, d) => {
  const i = args.indexOf(`--${n}`)
  return i < 0 ? d : args[i + 1]
}
const outDir = resolve(opt('out', join(ROOT, 'out', 'design-audit')))
const theme = opt('theme', 'dark')
const only = opt('only', '')
const build = resolve(opt('build', join(ROOT, 'out', 'audit-build')))
mkdirSync(outDir, { recursive: true })

// ── 임시 저장소(커밋·브랜치·태그·더러운 작업 트리) ──
const repo = mkdtempSync(join(tmpdir(), 'antler-audit-repo-'))
const env = { ...process.env, GIT_AUTHOR_NAME: 'Audit', GIT_AUTHOR_EMAIL: 'a@a', GIT_COMMITTER_NAME: 'Audit', GIT_COMMITTER_EMAIL: 'a@a' }
const git = (...a) => execFileSync('git', ['-C', repo, ...a], { env, stdio: 'pipe' })
git('init', '-q', '-b', 'main')
for (let i = 1; i <= 12; i++) {
  writeFileSync(join(repo, `file${i % 4}.txt`), `line ${i}\n`.repeat(i))
  git('add', '.')
  git('commit', '-q', '-m', `커밋 ${i}: 긴 제목을 가진 커밋 메시지 예시 to check truncation behaviour`)
  if (i === 4) git('branch', 'feature/ui-audit')
  if (i === 6) git('tag', 'v0.1.0')
}
writeFileSync(join(repo, 'file1.txt'), 'const label = "abcdefgh 한글 섞인 줄 ijkl"; // 한국어 주석 Hello\nconst n = 1234567890; // 숫자 0123456789\n')
writeFileSync(join(repo, 'untracked.txt'), 'new\n')

const userData = mkdtempSync(join(tmpdir(), 'antler-audit-ud-'))
writeFileSync(
  join(userData, 'antler.config.json'),
  JSON.stringify({
    version: 1,
    sources: [{ id: 'local', kind: 'local', label: 'Local' }],
    repos: [{ id: 'r0', sourceId: 'local', path: repo, name: 'audit-repo' }],
    ui: { theme, locale: 'ko', paneWidths: [248, 380], lastRepoId: 'r0' }
  })
)

const launch = (extraEnv = {}) =>
  electron.launch({
    executablePath: createRequire(import.meta.url)('electron'),
    args: [join(build, 'main', 'index.js')],
    env: { ...process.env, ANTLER_USER_DATA: userData, ...extraEnv },
    timeout: 30_000
  })

const rows = []
const measure = async (page, surface, scope = 'body') => {
  const list = await page.evaluate((scope) => {
    const root = document.querySelector(scope)
    if (!root) return []
    const out = []
    for (const el of root.querySelectorAll('button, input, select, summary, [role=tab], .env-badge, .filter-chip, .pr-chip, .badge, .side-item, .side-section__head, .menu-btn__item')) {
      const r = el.getBoundingClientRect()
      if (r.width === 0 || r.height === 0) continue
      const cs = getComputedStyle(el)
      const label = (el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 24)
      out.push({
        sel: `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}`,
        label,
        x: Math.round(r.x),
        y: Math.round(r.y),
        w: +r.width.toFixed(1),
        h: +r.height.toFixed(1),
        fs: cs.fontSize,
        lh: cs.lineHeight,
        br: cs.borderRadius,
        pad: cs.padding
      })
    }
    return out
  }, scope)
  for (const l of list) rows.push({ surface, ...l })
}
const shot = async (page, name) => page.screenshot({ path: join(outDir, `${name}-${theme}.png`) })
const settle = (page, ms = 250) => page.waitForTimeout(ms)

/** 업데이트 스낵바: 위치(오른쪽 아래, 상태 표시줄 위)와 문구·버튼·닫기 아이콘의 세로 중심 어긋남을 잰다 */
const snackGeometry = (page) =>
  page.evaluate(() => {
    const box = (el) => {
      const r = el.getBoundingClientRect()
      return { x: r.x, y: r.y, w: r.width, h: r.height, cy: r.y + r.height / 2 }
    }
    const snack = document.querySelector('.update-snack')
    const msg = document.querySelector('.update-snack__msg')
    const x = document.querySelector('.update-snack__x')
    const actions = document.querySelector('.update-snack__actions')
    const bar = document.querySelector('.statusbar')
    // 글자 한 줄의 실제 잉크 중심: Range 로 첫 줄 박스를 잰다
    const range = document.createRange()
    range.selectNodeContents(msg)
    const line = range.getClientRects()[0]
    const s = box(snack)
    const o = {
      vw: innerWidth,
      vh: innerHeight,
      snack: s,
      right: +(innerWidth - (s.x + s.w)).toFixed(1),
      gapAboveStatusbar: bar ? +(bar.getBoundingClientRect().y - (s.y + s.h)).toFixed(1) : null,
      msgLines: Math.round(msg.getBoundingClientRect().height / parseFloat(getComputedStyle(msg).lineHeight)),
      // 문구 첫 줄 중심 - 닫기 아이콘 중심
      msgFirstLineVsX: +((line.y + line.height / 2) - box(x).cy).toFixed(2),
      // 닫기 중심 - 카드 중심 / 문구 블록 중심 - 닫기 중심
      msgBlockVsX: +(box(msg).cy - box(x).cy).toFixed(2),
      xVsCard: +(box(x).cy - s.cy).toFixed(2)
    }
    if (actions) {
      const btns = [...actions.children].map(box)
      o.actionsVsRow = btns.map((b) => +(b.cy - box(actions).cy).toFixed(2))
    }
    return o
  })

async function main() {
  if (only === 'update') return updateSnack()
  const app = await launch()
  const page = await app.firstWindow()
  try {
    await page.setViewportSize({ width: 1440, height: 860 }).catch(() => {})
    await page.waitForSelector('.commit-subject', { timeout: 20_000 })
    await settle(page, 600)
    await shot(page, 'main')
    await measure(page, 'toolbar', '.toolbar')
    await measure(page, 'sidebar', '.sidebar, aside')
    await measure(page, 'commit-list', '.commit-list')

    // 하단 상태 표시줄: 높이·폭·위치, 드래그 영역 여부
    const sb = await page.evaluate(() => {
      const el = document.querySelector('.statusbar')
      if (!el) return null
      const r = el.getBoundingClientRect()
      const cs = getComputedStyle(el)
      return { x: r.x, y: r.y, w: r.width, h: r.height, vw: innerWidth, vh: innerHeight, borderTop: cs.borderTopWidth, region: cs.getPropertyValue('-webkit-app-region'), text: el.textContent }
    })
    console.log('statusbar', JSON.stringify(sb))
    await measure(page, 'statusbar', '.statusbar')
    // 저장소 카드(사이드바)
    const card = await page.evaluate(() => {
      const el = document.querySelector('.side-section--card')
      if (!el) return null
      const r = el.getBoundingClientRect()
      const cs = getComputedStyle(el)
      return { x: r.x, w: r.width, border: cs.borderTopWidth, radius: cs.borderTopLeftRadius, bg: cs.backgroundColor }
    })
    console.log('repo card', JSON.stringify(card))

    // 테마·언어 메뉴(상태 표시줄에서 위로 열린다)
    for (const lbl of ['테마', '언어']) {
      const b = await page.$(`.statusbar .menu-btn > button[aria-label="${lbl}"]`)
      if (b) {
        await b.click()
        await settle(page)
        await shot(page, `menu-${lbl === '테마' ? 'theme' : 'lang'}`)
        await measure(page, `menu-${lbl}`, '.menu-btn__pop')
        const pop = await page.evaluate(() => {
          const p = document.querySelector('.menu-btn__pop').getBoundingClientRect()
          const bt = document.querySelector('.menu-btn__pop').parentElement.querySelector('button').getBoundingClientRect()
          return { popBottom: p.bottom, btnTop: bt.top, popTop: p.top, opensUp: p.bottom <= bt.top, inViewport: p.top >= 0 && p.right <= innerWidth }
        })
        console.log(`menu ${lbl}`, JSON.stringify(pop))
        await page.keyboard.press('Escape')
      }
    }
    const chev = await page.$('.refresh-group .menu-btn > button')
    if (chev) {
      await chev.click()
      await settle(page)
      await shot(page, 'menu-fetch')
      await measure(page, 'menu-fetch', '.menu-btn__pop')
      await page.keyboard.press('Escape')
    }

    // GitHub 팝오버(로그아웃 상태)
    const gh = await page.$('.gh-auth__btn')
    if (gh) {
      await gh.click()
      await settle(page)
      await shot(page, 'gh-popover')
      await measure(page, 'gh-popover', '.gh-auth__pop')
      const adv = await page.$('.gh-auth__adv summary')
      if (adv) {
        await adv.click()
        await settle(page)
        await shot(page, 'gh-popover-adv')
        await measure(page, 'gh-popover-adv', '.gh-auth__pop')
      }
      await page.keyboard.press('Escape')
      if (await page.$('.gh-auth__pop')) await page.click('.gh-auth__btn')
    }

    // 필터 칩(브랜치 더블클릭)
    const side = '.side-section:nth-of-type(2) .side-item:not(.side-item--folder)'
    if (await page.$(side)) {
      await page.dblclick(`${side} >> nth=-1`)
      await page.waitForSelector('.filter-chip', { timeout: 4000 }).catch(() => {})
      await settle(page, 400)
      await shot(page, 'filter-chip')
      await measure(page, 'toolbar+chip', '.toolbar')
      await page.click('.filter-chip').catch(() => {})
      await settle(page)
    }

    // 검색 바
    await page.keyboard.press('Control+F')
    await page.waitForSelector('.search-bar__input')
    await page.fill('.search-bar__input', '커밋')
    await settle(page, 700)
    await shot(page, 'search')
    await measure(page, 'search-bar', '.search-bar')
    await page.keyboard.press('Escape')

    // 저장소 전환기
    await page.keyboard.press('Control+P')
    await page.waitForSelector('.switcher')
    await settle(page)
    await shot(page, 'switcher')
    await measure(page, 'switcher', '.switcher')
    const ndrag = await page.evaluate(() => getComputedStyle(document.querySelector('.switcher-backdrop')).getPropertyValue('-webkit-app-region'))
    console.log(`switcher-backdrop app-region: "${ndrag}"`)
    await page.keyboard.press('Escape')

    // 저장소 추가 대화상자(로컬·WSL·SSH 탭)
    await page.click('.side-section__extra .icon-btn')
    await page.waitForSelector('.add-dialog')
    await settle(page)
    const mdrag = await page.evaluate(() => getComputedStyle(document.querySelector('.modal-backdrop')).getPropertyValue('-webkit-app-region'))
    console.log(`modal-backdrop app-region: "${mdrag}"`)
    await shot(page, 'add-local')
    await measure(page, 'add-local', '.add-dialog')
    const tabs = await page.$$('.add-dialog__tabs .seg__btn')
    console.log(`add dialog tabs: ${tabs.length}`)
    if (tabs.length >= 2) {
      await tabs[1].click()
      await settle(page, 900)
      await shot(page, 'add-wsl')
      await measure(page, 'add-wsl', '.add-dialog')
    }
    if (tabs.length >= 3) {
      await tabs[2].click()
      await settle(page)
      await shot(page, 'add-ssh')
      await measure(page, 'add-ssh', '.add-dialog')
      await page.fill('.add-dialog input[placeholder]', 'invalid.invalid').catch(() => {})
      const browse = await page.$('.field__row .btn')
      if (browse) {
        await browse.click()
        await page.waitForSelector('.rfp', { timeout: 5000 }).catch(() => {})
        await settle(page, 1500)
        await shot(page, 'add-ssh-rfp')
        await measure(page, 'rfp', '.rfp')
        await page.keyboard.press('Escape')
      }
    }
    await page.keyboard.press('Escape')
    await page.waitForSelector('.add-dialog', { state: 'detached', timeout: 2000 }).catch(() => {})

    // WIP 패널 → diff
    await page.click('.commit-row--wip').catch(() => {})
    await page.waitForSelector('.wip-head', { timeout: 5000 }).catch(() => {})
    await settle(page, 400)
    await shot(page, 'wip')
    await measure(page, 'wip-panel', '.detail')
    await page.click('.detail .file-row >> nth=0').catch(() => {})
    await page.waitForSelector('.diff-row', { timeout: 5000 }).catch(() => {})
    await settle(page, 400)
    await shot(page, 'wip-diff')
    // 코드 글꼴: 한글/라틴이 각각 어떤 글꼴로 그려지는지(CDP) + 글자 폭
    {
      const cdp = await page.context().newCDPSession(page)
      await cdp.send('DOM.enable')
      await cdp.send('CSS.enable')
      const { root } = await cdp.send('DOM.getDocument', { depth: -1 })
      const { nodeIds } = await cdp.send('DOM.querySelectorAll', { nodeId: root.nodeId, selector: '.diff-code' })
      const seen = new Map()
      for (const id of nodeIds.slice(0, 12)) {
        const { fonts } = await cdp.send('CSS.getPlatformFontsForNode', { nodeId: id })
        for (const f of fonts) seen.set(f.familyName, (seen.get(f.familyName) ?? 0) + f.glyphCount)
      }
      console.log('code fonts (glyph counts):', JSON.stringify([...seen]))
      const w = await page.evaluate(() => {
        const mk = (t) => {
          const e = document.createElement('span')
          e.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;font:13px var(--font-mono)'
          e.textContent = t
          document.body.appendChild(e)
          const r = e.getBoundingClientRect().width
          e.remove()
          return r
        }
        return { ascii10: mk('abcdefghij'), hangul5: mk('한글한글한'), loaded: [...document.fonts].filter((f) => f.status === 'loaded').map((f) => `${f.family} ${f.weight}`) }
      })
      console.log('code widths @13px', JSON.stringify(w), 'hangul cell / ascii cell:', (w.hangul5 / 5 / (w.ascii10 / 10)).toFixed(2))
    }
    await measure(page, 'diff-head', '.diff-view')
    await page.keyboard.press('Escape')

    // 커밋 상세 + diff
    await page.click('.commit-row:not(.commit-row--wip) >> nth=1').catch(() => {})
    await page.waitForSelector('.file-row', { timeout: 5000 }).catch(() => {})
    await settle(page, 400)
    await shot(page, 'detail')
    await measure(page, 'detail', '.detail')
    if (await page.$('.minimap')) await measure(page, 'minimap', '.minimap')
    await page.click('.file-row >> nth=0').catch(() => {})
    await page.waitForSelector('.diff-row', { timeout: 5000 }).catch(() => {})
    await settle(page, 400)
    await shot(page, 'diff')
    await measure(page, 'diff-commit', '.diff-view')
    await page.keyboard.press('Escape')
  } finally {
    await app.close()
  }

  await updateSnack()
}

async function updateSnack() {
  // 업데이트 스낵바
  for (const state of ['available', 'downloaded', 'error']) {
    const a2 = await launch({ ANTLER_UPDATE_FAKE: state })
    const p2 = await a2.firstWindow()
    try {
      await p2.setViewportSize({ width: 1440, height: 860 }).catch(() => {})
      await p2.waitForSelector('.update-snack', { timeout: 15_000 })
      await settle(p2, 500)
      await shot(p2, `update-${state}`)
      await measure(p2, `update-${state}`, '.update-snack')
      console.log(`snack ${state}`, JSON.stringify(await snackGeometry(p2)))
      if (state === 'available') {
        // 오류 토스트를 억지로 띄워 스낵바와 겹치지 않고 쌓이는지 본다
        // (스토어에 접근할 수 없어 같은 클래스의 DOM 을 주입한다 — 쌓임은 CSS 만으로 정해진다)
        await p2.evaluate(() => {
          const d = document.createElement('div')
          d.className = 'toast'
          d.setAttribute('role', 'alert')
          d.textContent = '테스트 오류: 토스트와 스낵바 쌓임 확인'
          document.querySelector('.app').appendChild(d)
        })
        await settle(p2, 200)
        const st = await p2.evaluate(() => {
          const a = document.querySelector('.update-snack').getBoundingClientRect()
          const t = document.querySelector('.toast')?.getBoundingClientRect()
          return t ? { snackTop: a.top, toastBottom: t.bottom, gap: +(a.top - t.bottom).toFixed(1), overlap: t.bottom > a.top } : null
        })
        console.log('toast stack', JSON.stringify(st))
        await shot(p2, 'update-toast-stack')
      }
    } catch (e) {
      console.log(`update ${state}: ${e.message}`)
    } finally {
      await a2.close()
    }
  }
}

try {
  await main()
} finally {
  rmSync(userData, { recursive: true, force: true })
  rmSync(repo, { recursive: true, force: true })
}
writeFileSync(join(outDir, `measure-${theme}.json`), JSON.stringify(rows, null, 1))
const fmt = (r) => `${r.surface.padEnd(14)} ${r.sel.slice(0, 44).padEnd(44)} ${String(r.h).padStart(5)}h ${String(r.w).padStart(6)}w fs ${r.fs.padEnd(5)} br ${r.br.padEnd(5)} ${r.label}`
console.log(rows.map(fmt).join('\n'))
