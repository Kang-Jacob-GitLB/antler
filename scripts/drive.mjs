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
import { execFileSync } from 'node:child_process'

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
// 실험용: 이 CSS 를 페이지에 주입한 상태로 측정한다(예: --inject-css '* { box-shadow: none !important }')
const injectCss = opt('inject-css', '')
// 지정 시 diff 를 연 상태에서 첫 저장소에 빈 커밋을 만들어 자동 반영을 확인한다(임시 저장소에만 쓸 것)
const externalCommit = args.includes('--external-commit') && args.splice(args.indexOf('--external-commit'), 1) && true
// 지정 시 GitHub PR 목록·상세·파일 탭 시나리오를 돈다(첫 저장소의 origin 이 GitHub 공개 저장소일 때, 로그인 없이 읽기만)
const githubScenario = args.includes('--github') && args.splice(args.indexOf('--github'), 1) && true
// 실험용 Chromium·Electron 스위치(예: --electron-args "--disable-gpu-rasterization")
const electronArgs = opt('electron-args', '').split(' ').filter(Boolean)
// --idle-only: 첫 화면만 띄우고 조작 없이 N초 쉰 뒤 메모리를 잰다(유휴 메모리 측정)
const idleOnly = Number(opt('idle-only', '0'))
// 저장소 지정: 로컬 경로 또는 'wsl:<배포판>:<리눅스 경로>' / 'ssh:<호스트>:<경로>'
const repos = args.map((p) => {
  const m = /^(wsl|ssh):([^:]+):(.+)$/.exec(p)
  if (!m) return { kind: 'local', sourceId: 'local', path: resolve(p) }
  const [, kind, target, path] = m
  return { kind, sourceId: `${kind}:${target}`, target, path }
})
const sources = [{ id: 'local', kind: 'local', label: 'Local' }]
for (const r of repos) {
  if (r.kind === 'local' || sources.some((s) => s.id === r.sourceId)) continue
  sources.push({ id: r.sourceId, kind: r.kind, label: `${r.kind.toUpperCase()} · ${r.target}`, ...(r.kind === 'wsl' ? { distro: r.target } : { sshHost: r.target }) })
}
mkdirSync(outDir, { recursive: true })

const userData = mkdtempSync(join(tmpdir(), 'antler-userdata-'))
writeFileSync(
  join(userData, 'antler.config.json'),
  JSON.stringify({
    version: 1,
    sources,
    repos: repos.map((r, i) => ({ id: `r${i}`, sourceId: r.sourceId, path: r.path, name: basename(r.path) })),
    ui: { theme, locale, paneWidths: [248, 380], lastRepoId: repos.length ? 'r0' : undefined }
  })
)

const t0 = Date.now()
const app = await electron.launch({
  // require('electron') 는 실행 파일 경로를 돌려주며, 바이너리가 없으면 그때 내려받는다(Electron 44+)
  executablePath: createRequire(import.meta.url)('electron'),
  args: [...electronArgs, APP_DIR],
  env: { ...process.env, ANTLER_USER_DATA: userData },
  timeout: 30_000
})
// ANTLER_DRIVE_DEBUG=1 이면 main 프로세스 출력을 함께 보여 준다
if (process.env['ANTLER_DRIVE_DEBUG']) {
  app.process().stdout?.on('data', (d) => process.stdout.write(`[main] ${d}`))
  app.process().stderr?.on('data', (d) => process.stdout.write(`[main!] ${d}`))
}
const page = await app.firstWindow()
const shot = async (name) => {
  const f = join(outDir, `${name}.png`)
  await page.screenshot({ path: f })
  console.log('screenshot:', f)
}

try {
  await page.setViewportSize({ width: 1440, height: 860 }).catch(() => {})
  if (injectCss) await page.addStyleTag({ content: injectCss })
  if (!repos.length) {
    await page.waitForSelector('.empty-state', { timeout: 15_000 })
    console.log(`cold start → empty state: ${Date.now() - t0} ms`)
    await shot(`empty-${theme}`)
  } else {
    await page.waitForSelector('.commit-subject', { timeout: 15_000 })
    // 앱 기준 콜드 스타트: main 프로세스가 시작된 뒤 첫 커밋 행이 보일 때까지(Playwright 가 붙는 시간 제외)
    const appUptime = Math.round((await app.evaluate(() => process.uptime())) * 1000)
    console.log(`cold start → first commit row: ${Date.now() - t0} ms (app process uptime ${appUptime} ms)`)
    await page.waitForTimeout(300)
    await shot(`list-${theme}`)
  }
  if (repos.length && idleOnly > 0) {
    // 유휴 측정: 첫 화면 그대로 두고 기다린 뒤 아래 메모리 측정으로 넘어간다
    await page.waitForTimeout(idleOnly * 1000)
  } else if (repos.length) {

    // 번들 폰트가 실제로 로드·적용됐는지
    const fonts = await page.evaluate(async () => {
      await document.fonts.ready
      const loaded = [...document.fonts].filter((f) => f.status === 'loaded').map((f) => `${f.family} ${f.weight}`)
      return { loaded, ui: getComputedStyle(document.body).fontFamily.split(',')[0] }
    })
    console.log(`fonts loaded: ${fonts.loaded.join(', ') || '(none)'} — body: ${fonts.ui}`)

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

      // --external-commit: diff 를 연 채로 첫 저장소(로컬)에 외부 커밋을 만들고, 자동 반영 후에도 diff·선택이 유지되는지
      if (externalCommit && repos[0]?.kind === 'local') {
        const before = await page.textContent('.detail__sha')
        const file = await page.textContent('.diff-head__base')
        execFileSync('git', ['-C', repos[0].path, '-c', 'user.name=drive', '-c', 'user.email=d@d', 'commit', '-q', '--allow-empty', '-m', 'external commit'])
        // diff 를 보는 동안 그래프는 숨겨져(가상 스크롤이 행을 그리지 않음) 있으므로, 반영될 시간을 준 뒤
        // diff·선택 유지 여부를 보고, diff 를 닫아 목록 맨 위에 새 커밋이 왔는지 확인한다
        await page.waitForTimeout(4_000)
        const diffStill = await page.isVisible('.diff-view')
        const after = await page.textContent('.detail__sha')
        console.log(`external commit → diff still open: ${diffStill} (${await page.textContent('.diff-head__base').catch(() => '-')} vs ${file}); selection kept: ${before === after}`)
        await page.keyboard.press('Escape')
        await page.waitForSelector('.diff-view', { state: 'detached' })
        await page.waitForTimeout(300)
        console.log(`external commit → top row after closing diff: "${await page.textContent('.commit-row .commit-subject')}"`)
        await page.click('.file-row').catch(() => {})
        await page.waitForSelector('.diff-view', { timeout: 5_000 }).catch(() => {})
      }
      await page.keyboard.press('Escape')
      await page.waitForSelector('.diff-view', { state: 'detached' })
      console.log('Esc → diff closed, graph visible:', await page.isVisible('.commit-list'))
    }

    // --github: 첫 저장소의 origin 이 GitHub 공개 저장소면 PR 목록·상세·파일 탭을 확인한다(로그인 없이 읽기만)
    if (githubScenario) {
      const t0gh = Date.now()
      const hasSection = await page
        .waitForFunction(() => [...document.querySelectorAll('.side-section__title')].some((e) => /풀 리퀘스트|Pull requests/.test(e.textContent ?? '')), null, { timeout: 15_000 })
        .then(() => true)
        .catch(() => false)
      console.log(`github sections: ${hasSection} (+${Date.now() - t0gh} ms)`)
      if (hasSection) {
        // PR 섹션의 '전체 보기' → 목록, 상태 '전체'
        await page.click('.side-section:has(.side-section__title:text-matches("풀 리퀘스트|Pull requests")) .side-section__extra .icon-btn')
        await page.waitForSelector('.gh-view')
        await page.click('.gh-filters .seg__btn:nth-child(3)')
        await page.waitForSelector('.gh-row', { timeout: 15_000 }).catch(() => {})
        const rows = await page.$$eval('.gh-row', (els) => els.map((e) => e.querySelector('.gh-row__text')?.textContent))
        console.log(`github pulls (all): ${rows.length} — ${rows.slice(0, 3).join(' | ')}`)
        await shot(`gh-list-${theme}`)
        if (rows.length) {
          await page.click('.gh-row >> nth=0')
          await page.waitForSelector('.gh-comment', { timeout: 15_000 }).catch(() => {})
          console.log(`github pull detail: "${(await page.textContent('.gh-head__title').catch(() => ''))?.trim()}"`)
          await shot(`gh-pull-${theme}`)
          await page.click('.gh-tab:nth-child(3)')
          await page.waitForSelector('.gh-files .file-row', { timeout: 15_000 }).catch(() => {})
          await page.waitForTimeout(800)
          console.log(`github pull files: ${await page.$$eval('.gh-files .file-row', (e) => e.length)}, diff rows: ${await page.$$eval('.gh-files .diff-row', (e) => e.length)}`)
          await shot(`gh-files-${theme}`)
        }
        await page.keyboard.press('Escape')
        await page.keyboard.press('Escape')
        await page.keyboard.press('Escape')
        await page.waitForSelector('.gh-view', { state: 'detached', timeout: 5_000 }).catch(() => {})
      }
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

    // 저장소 추가 대화상자: WSL 탭이 있으면 연결 테스트까지(원격 SSH 서버에는 접속하지 않는다)
    await page.click('.side-section__extra .icon-btn')
    await page.waitForSelector('.add-dialog')
    const wslTab = await page.$('.add-dialog__tabs .seg__btn:nth-child(2)')
    if (wslTab && (await wslTab.textContent())?.includes('WSL')) {
      await wslTab.click()
      await page.waitForTimeout(800)
      const t = Date.now()
      await page.click('.add-dialog__foot .btn:first-child')
      await page.waitForSelector('.add-dialog__msg', { timeout: 30_000 }).catch(() => {})
      console.log(`add dialog WSL test: "${(await page.textContent('.add-dialog__msg').catch(() => ''))?.trim()}" (+${Date.now() - t} ms)`)
    }
    await shot(`add-dialog-${theme}`)
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
