// drive.mjs 의 Tauri 호스트: 빌드된 exe(cdp 기능 빌드)를 띄우고 WebView2 에 CDP 로 붙는다.
// host 인터페이스는 electronHost.mjs 와 같다. 창을 전면으로 가져오거나 OS 입력을 합성하지 않는다.
import { spawn, execFileSync } from 'node:child_process'
import { createServer } from 'node:net'
import { chromium } from 'playwright-core'
import { listProcesses, webviewProcsOf, measureTree } from './procTree.mjs'

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer()
    srv.once('error', reject)
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address()
      srv.close(() => resolve(port))
    })
  })
}

/** 최대화 버튼에 머물면 620ms 뒤 Rust 가 실제 Win+Z 를 보낸다 — 창 컨트롤 위로 마우스가 가지 못하게 막는다 */
async function guardWindowControls(page) {
  // 1) 컨트롤을 히트 테스트에서 뺀다(mouseenter 자체가 일어나지 않는다). 측정 전용 페이지 수정이며 앱 코드는 건드리지 않는다
  // bridge 가 창 컨트롤을 올릴 때까지 기다린다(페이지가 막 열린 직후에는 아직 없다)
  await page.waitForSelector('.tb-controls .tb-btn--max', { state: 'attached', timeout: 30_000 })
  await page.addStyleTag({ content: '.tb-controls, .tb-controls * { pointer-events: none !important }' })
  const ok = await page.evaluate(() => {
    const max = document.querySelector('.tb-controls .tb-btn--max')
    return !!max && getComputedStyle(max).pointerEvents === 'none'
  })
  if (!ok) throw new Error('창 컨트롤 보호 실패: .tb-controls 를 찾지 못했거나 pointer-events 가 꺼지지 않았다')
  // 2) page.mouse 로 직접 좌표를 쓰는 곳은 .tb-controls 상자와 겹치면 throw
  const assertClear = async (x, y) => {
    const hit = await page.evaluate(
      ([px, py]) => {
        const r = document.querySelector('.tb-controls')?.getBoundingClientRect()
        return !!r && px >= r.left && px <= r.right && py >= r.top && py <= r.bottom
      },
      [x, y]
    )
    if (hit) throw new Error(`마우스 좌표 (${x}, ${y}) 가 창 컨트롤(.tb-controls) 위다 — Win+Z 가 실제로 나간다`)
  }
  for (const name of ['move', 'click', 'dblclick', 'down', 'up']) {
    const orig = page.mouse[name].bind(page.mouse)
    // down·up 은 좌표가 없다(직전 move 로 검증됨)
    page.mouse[name] = async (...a) => {
      if (typeof a[0] === 'number' && typeof a[1] === 'number') await assertClear(a[0], a[1])
      return orig(...a)
    }
  }
}

export async function launchTauri({ exePath, userData, gpu }) {
  const port = await freePort()
  const debug = !!process.env['ANTLER_DRIVE_DEBUG']
  const startedAt = Date.now()
  const proc = spawn(exePath, [], {
    env: { ...process.env, ANTLER_USER_DATA: userData, ANTLER_CDP_PORT: String(port), ANTLER_MEASURE: '1', ...(gpu ? { ANTLER_GPU: '1' } : {}) },
    stdio: debug ? ['ignore', 'pipe', 'pipe'] : 'ignore'
  })
  if (debug) {
    proc.stdout?.on('data', (d) => process.stdout.write(`[main] ${d}`))
    proc.stderr?.on('data', (d) => process.stdout.write(`[main!] ${d}`))
  }
  let exited = false
  proc.once('exit', () => (exited = true))

  const killAll = () => {
    // 이름이 아니라 PID 로만 죽인다(설치판 Antler.exe 를 건드리지 않는다)
    try {
      execFileSync('taskkill', ['/PID', String(proc.pid), '/T', '/F'], { stdio: 'ignore' })
    } catch {
      // 이미 끝났다
    }
    // 트리 밖으로 빠진 이 userData 의 WebView2 잔여 프로세스
    try {
      for (const p of webviewProcsOf(listProcesses(), userData)) {
        try {
          execFileSync('taskkill', ['/PID', String(p.ProcessId), '/T', '/F'], { stdio: 'ignore' })
        } catch {
          // 이미 끝났다
        }
      }
    } catch {
      // 목록을 못 읽어도 정리는 계속한다
    }
  }

  try {
    // 디버깅 포트가 열릴 때까지 25ms 폴링
    const deadline = Date.now() + 30_000
    for (;;) {
      if (exited) throw new Error('Tauri 앱이 CDP 연결 전에 종료됐다')
      if (Date.now() > deadline) throw new Error('CDP 포트가 30초 안에 열리지 않았다')
      const up = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) })
        .then((r) => r.ok)
        .catch(() => false)
      if (up) break
      await sleep(25)
    }
    const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`)
    let page
    while (!page) {
      page = browser.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes('tauri.localhost'))
      if (!page) {
        if (Date.now() > deadline) throw new Error('tauri.localhost 페이지를 찾지 못했다')
        await sleep(25)
      }
    }
    await guardWindowControls(page)
    return {
      kind: 'tauri',
      page,
      pid: proc.pid,
      startedAt,
      // bridge 가 기록한 첫 커밋 행 시각(performance.now)을 시계 시각으로 바꿔 spawn 부터의 ms 로 돌려준다
      uptimeMs: async () => {
        const at = await page.evaluate(() => (window.__ANTLER_FIRST_ROW__ === undefined ? null : performance.timeOrigin + window.__ANTLER_FIRST_ROW__))
        return at === null ? null : Math.round(at - startedAt)
      },
      // Tauri 에는 getAppMetrics 가 없어 프로세스 트리(procTree)로 잰다. 값은 KB(Electron 과 같은 단위)
      memory: async () => {
        const t = measureTree({ rootPid: proc.pid, userData })
        return Object.entries(t.groups)
          .filter(([, s]) => s.count)
          .map(([type, s]) => ({ type, ws: s.ws * 1024, priv: s.priv * 1024 }))
      },
      cpu: undefined,
      close: async () => {
        await browser.close().catch(() => {})
        killAll()
        // 핸들이 풀릴 때까지 잠깐 기다린다(userData 삭제용)
        for (let i = 0; i < 40 && !exited; i++) await sleep(50)
      }
    }
  } catch (e) {
    killAll()
    throw e
  }
}
