import { EventEmitter } from 'node:events'
import { describe, expect, it } from 'vitest'
import { ExternalOpener, terminalTools, type OpenerDeps } from '../src/main/external/opener'
import { buildRevealPlan, hasDotComponent, type Target } from '../src/main/external/plan'

const local: Target = { kind: 'local', root: '/r' }
const ssh: Target = { kind: 'ssh', root: '/srv/repo', host: 'devbox' }
const wsl: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'U' }

describe('buildRevealPlan', () => {
  it('폴더는 열고 파일은 폴더에서 선택해 보여 준다(파일을 직접 열지 않는다)', () => {
    expect(buildRevealPlan(local, { path: '/r', isDir: true, root: '/r' }, 'linux')).toEqual({ type: 'openPath', path: '/r' })
    expect(buildRevealPlan(local, { path: '/r/a.sh', isDir: false, root: '/r' }, 'linux')).toEqual({ type: 'showItem', path: '/r/a.sh' })
  })

  it('SSH 는 거부, WSL 은 Windows 에서만', () => {
    expect(() => buildRevealPlan(ssh, { path: '/srv/repo', isDir: true, root: '/srv/repo' }, 'linux')).toThrow()
    expect(() => buildRevealPlan(wsl, { path: '/r', isDir: true, root: '/r' }, 'linux')).toThrow()
    const unc = '\\\\wsl.localhost\\U\\r'
    expect(buildRevealPlan(wsl, { path: unc, isDir: true, root: unc }, 'win32').type).toBe('openPath')
  })

  it('macOS: 이름에 .이 든 폴더(번들일 수 있음)는 열지 않고 보여 주기만 한다. 점 없는 일반 폴더만 연다', () => {
    const root = '/Users/u/repo'
    expect(buildRevealPlan(local, { path: `${root}/src/lib`, isDir: true, root }, 'darwin').type).toBe('openPath')
    for (const d of ['X.app', 'App.xcodeproj', 'a/b.service', 'x/.hidden', 'Pad.playground', 'W.wdgt']) {
      expect(buildRevealPlan(local, { path: `${root}/${d}`, isDir: true, root }, 'darwin').type, d).toBe('showItem')
    }
    // 저장소 루트 자체가 tool.app 이면 그 안의 모든 폴더가 막힌다
    expect(buildRevealPlan(local, { path: '/Users/u/tool.app/src', isDir: true, root: '/Users/u/tool.app' }, 'darwin').type).toBe('showItem')
    // 루트 위쪽 이름의 점은 보지 않는다
    expect(buildRevealPlan(local, { path: '/Users/u.name/repo/src', isDir: true, root: '/Users/u.name/repo' }, 'darwin').type).toBe('openPath')
    // 리눅스는 이름 규칙을 쓰지 않는다
    expect(buildRevealPlan(local, { path: '/r/X.app', isDir: true, root: '/r' }, 'linux').type).toBe('openPath')
  })
})

describe('hasDotComponent', () => {
  it('루트 이름과 루트 아래 구성 요소만 본다', () => {
    expect(hasDotComponent('/a/repo', '/a/repo/x/y')).toBe(false)
    expect(hasDotComponent('/a/repo', '/a/repo')).toBe(false)
    expect(hasDotComponent('/a/repo.git', '/a/repo.git')).toBe(true)
    expect(hasDotComponent('/a.b/repo', '/a.b/repo/x')).toBe(false)
    expect(hasDotComponent('/a/repo', '/a/repo/x.y/z')).toBe(true)
  })
})

/** 가짜 fs(links: 심볼릭 링크 → 대상). 목록에 없는 경로는 존재하지 않는다 */
function fake(platform: NodeJS.Platform, o: { files?: string[]; dirs?: string[]; links?: Record<string, string>; env?: NodeJS.ProcessEnv }) {
  const files = new Set(o.files ?? [])
  const dirs = new Set(o.dirs ?? [])
  const links = o.links ?? {}
  const spawned: { file: string; args: string[]; cwd?: string }[] = []
  const shown: { type: string; path: string }[] = []
  const real = (p: string): string => {
    // 경로의 각 구성 요소까지 링크를 푼다
    let cur = ''
    for (const part of p.split('/').filter(Boolean)) {
      cur = links[`${cur}/${part}`] ?? `${cur}/${part}`
    }
    cur = cur || '/'
    if (!files.has(cur) && !dirs.has(cur)) throw new Error('ENOENT')
    return cur
  }
  const stat = (p: string) => {
    const r = real(p)
    return { isFile: () => files.has(r), isDirectory: () => dirs.has(r) }
  }
  const deps: OpenerDeps = {
    platform,
    env: o.env ?? {},
    fs: { realpathSync: real, statSync: stat, lstatSync: stat },
    spawn: (file, args, opts) => {
      spawned.push({ file, args, ...(opts.cwd ? { cwd: opts.cwd } : {}) })
      const ee = new EventEmitter() as EventEmitter & { unref(): void }
      ee.unref = () => {}
      setImmediate(() => ee.emit('spawn'))
      return ee as never
    },
    shell: {
      openPath: async (p) => {
        shown.push({ type: 'openPath', path: p })
        return ''
      },
      showItemInFolder: (p) => shown.push({ type: 'showItem', path: p })
    }
  }
  return { deps, spawned, shown }
}

describe('ExternalOpener(macOS): realpath 기준 번들 판정', () => {
  const root = '/Users/u/repo'
  const t: Target = { kind: 'local', root }
  const fs = {
    dirs: ['/System/Applications/Utilities/Terminal.app', root, `${root}/src`, `${root}/Evil.app`, `${root}/Evil.app/Contents`, `${root}/P.xcodeproj`, `${root}/plain`],
    files: [`${root}/a.txt`],
    // innocent 는 번들을 가리키는 심볼릭 링크, plainlink 는 일반 폴더를 가리키는 링크
    links: { [`${root}/innocent`]: `${root}/Evil.app`, [`${root}/plainlink`]: `${root}/plain` }
  }

  it('번들을 가리키는 심볼릭 링크는 파일 관리자·터미널 모두 번들로 취급한다', async () => {
    const f = fake('darwin', fs)
    const o = new ExternalOpener(f.deps)
    await o.open(t, 'fileManager', 'innocent', {})
    expect(f.shown).toEqual([{ type: 'showItem', path: `${root}/Evil.app` }])
    await o.open(t, 'terminal', 'innocent', {})
    // 번들이 아닌 가장 가까운 상위(저장소 루트)에서 연다
    expect(f.spawned).toEqual([{ file: '/usr/bin/open', args: ['-a', '/System/Applications/Utilities/Terminal.app', root] }])
    await o.open(t, 'terminal', 'innocent/Contents', {})
    expect(f.spawned[1]?.args.at(-1)).toBe(root)
  })

  it('점이 든 폴더(.xcodeproj)는 showItem, 정상 폴더(링크 포함)는 openPath', async () => {
    const f = fake('darwin', fs)
    const o = new ExternalOpener(f.deps)
    await o.open(t, 'fileManager', 'P.xcodeproj', {})
    await o.open(t, 'fileManager', 'plain', {})
    await o.open(t, 'fileManager', 'plainlink', {})
    await o.open(t, 'fileManager', 'src/gone/x', {})
    await o.open(t, 'fileManager', 'Evil.app/gone', {})
    await o.open(t, 'fileManager', 'a.txt', {})
    expect(f.shown).toEqual([
      { type: 'showItem', path: `${root}/P.xcodeproj` },
      { type: 'openPath', path: `${root}/plain` },
      { type: 'openPath', path: `${root}/plain` },
      { type: 'openPath', path: `${root}/src` },
      { type: 'showItem', path: `${root}/Evil.app` },
      { type: 'showItem', path: `${root}/a.txt` }
    ])
  })

  it('저장소 루트가 tool.app 이면 열 수 있는 폴더가 없고 터미널은 거부한다', async () => {
    const r = '/Users/u/tool.app'
    const f = fake('darwin', { dirs: ['/System/Applications/Utilities/Terminal.app', r, `${r}/src`], files: [] })
    const o = new ExternalOpener(f.deps)
    await o.open({ kind: 'local', root: r }, 'fileManager', 'src', {})
    expect(f.shown).toEqual([{ type: 'showItem', path: `${r}/src` }])
    await expect(o.open({ kind: 'local', root: r }, 'terminal', 'src', {})).rejects.toThrow(/번들/)
    expect(f.spawned).toEqual([])
  })

  it('터미널은 Terminal.app 을 절대 경로로 고정해 연다(번들 ID·앱 이름 조회에 기대지 않는다)', async () => {
    const f = fake('darwin', fs)
    await new ExternalOpener(f.deps).open(t, 'terminal', 'src', {})
    expect(f.spawned[0]).toEqual({ file: '/usr/bin/open', args: ['-a', '/System/Applications/Utilities/Terminal.app', `${root}/src`] })
  })
})

describe('ExternalOpener(Windows): 정션·링크도 realpath 가 폴더일 때만 openPath', () => {
  it('폴더를 가리키는 링크는 openPath, 파일을 가리키는 링크는 showItem', async () => {
    // 가짜 fs 는 POSIX 경로만 다루므로 Linux 로 같은 규칙(실제 경로가 폴더인가)을 확인한다
    const f = fake('linux', { dirs: ['/r', '/r/d'], files: ['/r/evil.exe'], links: { '/r/junction': '/r/d', '/r/lnk': '/r/evil.exe' } })
    const o = new ExternalOpener(f.deps)
    await o.open(local, 'fileManager', 'junction', {})
    await o.open(local, 'fileManager', 'lnk', {})
    expect(f.shown).toEqual([
      { type: 'openPath', path: '/r/d' },
      { type: 'showItem', path: '/r/evil.exe' }
    ])
  })
})

describe('terminalTools: macOS Terminal.app 절대 경로', () => {
  it('시스템 위치 → /Applications/Utilities 순으로 찾고, 둘 다 없으면 null(비활성)', () => {
    const sys = '/System/Applications/Utilities/Terminal.app'
    const apps = '/Applications/Utilities/Terminal.app'
    expect(terminalTools(fake('darwin', { dirs: [sys, apps] }).deps).macTerminal).toBe(sys)
    expect(terminalTools(fake('darwin', { dirs: [apps] }).deps).macTerminal).toBe(apps)
    expect(terminalTools(fake('darwin', { dirs: [] }).deps).macTerminal).toBeNull()
    // 디렉터리가 아니면(파일) 쓰지 않는다
    expect(terminalTools(fake('darwin', { files: [sys] }).deps).macTerminal).toBeNull()
  })

  it('Terminal.app 이 없으면 터미널 열기를 거부한다', async () => {
    const f = fake('darwin', { dirs: ['/Users/u/repo'] })
    await expect(new ExternalOpener(f.deps).open({ kind: 'local', root: '/Users/u/repo' }, 'terminal', undefined, {})).rejects.toThrow(/Terminal/)
    expect(f.spawned).toEqual([])
  })
})

describe('terminalTools', () => {
  const win = (env: NodeJS.ProcessEnv, files: string[] = []) => fake('win32', { env, files }).deps

  it('SystemRoot 가 드라이브 절대 경로일 때만 쓰고, 이상하면 C:\\Windows', () => {
    expect(terminalTools(win({ SystemRoot: 'D:\\WINNT' })).systemRoot).toBe('D:\\WINNT')
    for (const bad of ['', 'Windows', '\\\\evil\\share', 'C:\\a;b', 'C:\\a"b', '..\\x', '/etc']) {
      expect(terminalTools(win({ SystemRoot: bad })).systemRoot, bad).toBe('C:\\Windows')
    }
    expect(terminalTools(win({})).systemRoot).toBe('C:\\Windows')
  })

  it('LOCALAPPDATA 가 정상이고 wt.exe 가 있을 때만 wt 절대 경로를 돌려준다', () => {
    const d = fake('win32', { env: { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' }, files: [] }).deps
    expect(terminalTools(d).wt).toBeNull()
    const f = fake('win32', { env: { LOCALAPPDATA: 'C:\\L' }, files: [] })
    f.deps.fs.lstatSync = () => ({ isFile: () => true, isDirectory: () => false })
    expect(terminalTools(f.deps).wt).toBe('C:\\L\\Microsoft\\WindowsApps\\wt.exe')
    for (const bad of ['relative', '\\\\evil\\share', 'C:\\a;b']) {
      const g = fake('win32', { env: { LOCALAPPDATA: bad }, files: [] })
      g.deps.fs.lstatSync = () => ({ isFile: () => true, isDirectory: () => false })
      expect(terminalTools(g.deps).wt, bad).toBeNull()
    }
  })

  it('리눅스: PATH 의 절대 항목에서만 터미널을 찾는다', () => {
    const f = fake('linux', { env: { PATH: '.:rel:/opt/bin' }, files: ['/opt/bin/x-terminal-emulator', 'rel/x-terminal-emulator'] })
    expect(terminalTools(f.deps).linuxTerminal).toBe('/opt/bin/x-terminal-emulator')
    expect(terminalTools(fake('linux', { env: { PATH: '.:rel' }, files: [] }).deps).linuxTerminal).toBeNull()
  })
})
