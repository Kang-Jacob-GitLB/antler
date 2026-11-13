// drive.mjs 의 Electron 호스트: Playwright _electron 으로 띄운다.
// host 인터페이스: { kind, page, pid, startedAt, uptimeMs(), memory(), cpu(), close() }
import { createRequire } from 'node:module'
import { _electron as electron } from 'playwright-core'

export async function launchElectron({ appDir, exePath, electronArgs, userData, extraEnv }) {
  const startedAt = Date.now()
  const app = await electron.launch({
    // require('electron') 는 실행 파일 경로를 돌려주며, 바이너리가 없으면 그때 내려받는다(Electron 44+)
    executablePath: exePath ?? createRequire(import.meta.url)('electron'),
    // 패키지된 앱(--exe)은 앱 경로 인자 없이 실행한다(app.asar 를 스스로 읽는다)
    args: exePath ? electronArgs : [...electronArgs, appDir],
    env: { ...process.env, ANTLER_USER_DATA: userData, ...extraEnv },
    timeout: 30_000
  })
  // ANTLER_DRIVE_DEBUG=1 이면 main 프로세스 출력을 함께 보여 준다
  if (process.env['ANTLER_DRIVE_DEBUG']) {
    app.process().stdout?.on('data', (d) => process.stdout.write(`[main] ${d}`))
    app.process().stderr?.on('data', (d) => process.stdout.write(`[main!] ${d}`))
  }
  const page = await app.firstWindow()
  return {
    kind: 'electron',
    page,
    pid: app.process().pid,
    startedAt,
    // main 프로세스가 시작된 뒤 지금까지(ms)
    uptimeMs: async () => Math.round((await app.evaluate(() => process.uptime())) * 1000),
    // 프로세스별 { type, ws, priv } (KB)
    memory: () => app.evaluate(async ({ app }) => app.getAppMetrics().map((m) => ({ type: m.type, ws: m.memory.workingSetSize, priv: m.memory.privateBytes ?? 0 }))),
    // 렌더러(Tab) 프로세스별 CPU %
    cpu: () => app.evaluate(({ app }) => app.getAppMetrics().filter((m) => m.type === 'Tab').map((m) => m.cpu.percentCPUUsage)),
    close: () => app.close()
  }
}
