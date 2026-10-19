import { EventEmitter } from 'node:events'
import { describe, expect, it } from 'vitest'
import { ExternalOpener, checkChosenExe, findEditorExe, resolveEditorExe, type OpenerDeps } from '../src/main/external/opener'
import type { Target } from '../src/main/external/plan'

interface Call {
  file: string
  args: string[]
  cwd?: string
  shell: false
  detached: true
}

/** 가짜 파일 시스템(files: 파일, dirs: 폴더, links: 심볼릭 링크 → 대상)과 가짜 spawn·shell */
function fake(platform: NodeJS.Platform, o: { files?: string[]; dirs?: string[]; links?: Record<string, string>; env?: NodeJS.ProcessEnv; spawnError?: string } = {}) {
  const files = new Set(o.files ?? [])
  const dirs = new Set(o.dirs ?? [])
  const links = o.links ?? {}
  const spawned: Call[] = []
  const shown: { type: string; path: string }[] = []
  const real = (p: string): string => {
    const r = links[p] ?? p
    if (!files.has(r) && !dirs.has(r)) throw new Error('ENOENT')
    return r
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
      spawned.push({ file, args, ...(opts.cwd ? { cwd: opts.cwd } : {}), shell: opts.shell, detached: opts.detached })
      const ee = new EventEmitter() as EventEmitter & { unref(): void }
      ee.unref = () => {}
      setImmediate(() => (o.spawnError ? ee.emit('error', new Error(o.spawnError)) : ee.emit('spawn')))
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

const local: Target = { kind: 'local', root: '/home/u/repo' }
const LINUX = { files: ['/home/u/repo/a.txt', '/home/u/repo/-rf', '/usr/bin/code', '/usr/bin/x-terminal-emulator'], dirs: ['/home/u/repo', '/home/u/repo/src', '/etc'] }

describe('ExternalOpener: 로컬', () => {
  it('에디터: 저장소 폴더와 파일을 절대 경로로, shell 없이 분리 실행', async () => {
    const f = fake('linux', { ...LINUX, env: { PATH: '/usr/bin' } })
    await new ExternalOpener(f.deps).open(local, 'editor', '-rf', {})
    expect(f.spawned).toEqual([{ file: '/usr/bin/code', args: ['/home/u/repo', '/home/u/repo/-rf'], shell: false, detached: true }])
  })

  it('에디터: 저장소 루트만 열 수도 있다', async () => {
    const f = fake('linux', { ...LINUX, env: { PATH: '/usr/bin' } })
    await new ExternalOpener(f.deps).open(local, 'editor', undefined, { editor: 'vscode' })
    expect(f.spawned[0]?.args).toEqual(['/home/u/repo'])
  })

  it('없는 파일(삭제된 파일의 diff)은 저장소 폴더만 연다', async () => {
    const f = fake('linux', { ...LINUX, env: { PATH: '/usr/bin' } })
    await new ExternalOpener(f.deps).open(local, 'editor', 'src/gone.txt', {})
    expect(f.spawned[0]?.args).toEqual(['/home/u/repo'])
  })

  it('파일 관리자: 폴더는 openPath, 파일은 showItemInFolder, 없는 파일은 가까운 폴더', async () => {
    const f = fake('linux', LINUX)
    const o = new ExternalOpener(f.deps)
    await o.open(local, 'fileManager', undefined, {})
    await o.open(local, 'fileManager', 'a.txt', {})
    await o.open(local, 'fileManager', 'src/gone/x.txt', {})
    expect(f.shown).toEqual([
      { type: 'openPath', path: '/home/u/repo' },
      { type: 'showItem', path: '/home/u/repo/a.txt' },
      { type: 'openPath', path: '/home/u/repo/src' }
    ])
    expect(f.spawned).toEqual([])
  })

  it('존재하는 실행 파일이 상위인 없는 경로는 openPath 로 실행하지 않는다', async () => {
    const f = fake('linux', { ...LINUX, files: [...LINUX.files, '/home/u/repo/evil.exe'] })
    const o = new ExternalOpener(f.deps)
    await o.open(local, 'fileManager', 'evil.exe/x', {})
    // evil.exe(파일)는 건너뛰고 폴더인 저장소 루트를 연다
    expect(f.shown).toEqual([{ type: 'openPath', path: '/home/u/repo' }])
    await o.open(local, 'fileManager', 'evil.exe', {})
    expect(f.shown[1]).toEqual({ type: 'showItem', path: '/home/u/repo/evil.exe' })
    await o.open(local, 'terminal', 'evil.exe/x', {})
    expect(f.spawned[0]?.cwd).toBe('/home/u/repo')
  })

  it('macOS: 번들 디렉터리(.app)는 openPath 대신 보여 주기만 하고 터미널은 점 없는 상위 폴더에서 연다', async () => {
    const f = fake('darwin', { dirs: ['/System/Applications/Utilities/Terminal.app', '/home/u/repo', '/home/u/repo/X.app', '/home/u/repo/X.app/Contents'] })
    const o = new ExternalOpener(f.deps)
    await o.open(local, 'fileManager', 'X.app', {})
    await o.open(local, 'fileManager', 'X.app/gone', {})
    expect(f.shown).toEqual([
      { type: 'showItem', path: '/home/u/repo/X.app' },
      { type: 'showItem', path: '/home/u/repo/X.app' }
    ])
    await o.open(local, 'terminal', 'X.app/Contents', {})
    await o.open(local, 'terminal', 'X.app', {})
    expect(f.spawned.map((c) => [c.file, c.args.at(-1)])).toEqual([
      ['/usr/bin/open', '/home/u/repo'],
      ['/usr/bin/open', '/home/u/repo']
    ])
  })

  it('터미널: 파일이면 그 폴더, 폴더면 그 폴더(cwd 로 전달)', async () => {
    const f = fake('linux', LINUX)
    const o = new ExternalOpener(f.deps)
    await o.open(local, 'terminal', 'a.txt', {})
    await o.open(local, 'terminal', 'src', {})
    expect(f.spawned.map((c) => c.cwd)).toEqual(['/home/u/repo', '/home/u/repo/src'])
    expect(f.spawned[0]?.file).toBe('/usr/bin/x-terminal-emulator')
  })

  it('.. 와 절대 경로는 실행 전에 거부한다', async () => {
    const f = fake('linux', LINUX)
    const o = new ExternalOpener(f.deps)
    for (const rel of ['../x', 'src/../../etc', '/etc/passwd', 'a\0b']) {
      await expect(o.open(local, 'terminal', rel, {}), rel).rejects.toThrow()
    }
    expect(f.spawned).toEqual([])
    expect(f.shown).toEqual([])
  })

  it('저장소 밖을 가리키는 심볼릭 링크는 실행 전에 거부한다', async () => {
    const f = fake('linux', { ...LINUX, links: { '/home/u/repo/evil': '/etc' } })
    const o = new ExternalOpener(f.deps)
    await expect(o.open(local, 'fileManager', 'evil', {})).rejects.toThrow(/심볼릭/)
    await expect(o.open(local, 'terminal', 'evil', {})).rejects.toThrow(/심볼릭/)
    await expect(o.open(local, 'editor', 'evil', { editor: 'custom', editorExePath: '/opt/ide' })).rejects.toThrow()
    expect(f.spawned).toEqual([])
    expect(f.shown).toEqual([])
  })

  it('저장소 루트를 찾을 수 없으면 거부한다', async () => {
    const f = fake('linux', {})
    await expect(new ExternalOpener(f.deps).open(local, 'fileManager', undefined, {})).rejects.toThrow()
  })

  it('실행 실패(ENOENT 등)를 오류로 돌려준다', async () => {
    const f = fake('linux', { ...LINUX, spawnError: 'spawn ENOENT' })
    await expect(new ExternalOpener(f.deps).open(local, 'terminal', undefined, {})).rejects.toThrow(/실행하지 못했습니다/)
  })

  it('ELECTRON_RUN_AS_NODE 는 자식에게 넘기지 않는다', async () => {
    const f = fake('linux', LINUX)
    let seen: NodeJS.ProcessEnv | undefined
    const orig = f.deps.spawn
    f.deps.spawn = (file, args, opts) => {
      seen = opts.env
      return orig(file, args, opts)
    }
    f.deps.env = { ELECTRON_RUN_AS_NODE: '1', HOME: '/h' }
    await new ExternalOpener(f.deps).open(local, 'terminal', undefined, {})
    expect(seen).toEqual({ HOME: '/h' })
  })
})

describe('ExternalOpener: Windows 로컬', () => {
  const root = 'C:\\work\\repo'
  const t: Target = { kind: 'local', root }
  const env = { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' }
  const code = 'C:\\Users\\u\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe'
  const wt = 'C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\wt.exe'

  it('VS Code 는 .cmd 셰임이 아니라 Code.exe 를 쓴다', async () => {
    const f = fake('win32', { env, files: [code, 'C:\\work\\repo\\-x.txt'], dirs: [root] })
    await new ExternalOpener(f.deps).open(t, 'editor', '-x.txt', {})
    expect(f.spawned[0]).toMatchObject({ file: code, args: [root, 'C:\\work\\repo\\-x.txt'], shell: false })
  })

  it('터미널: wt 가 있으면 wt -d, 없으면 cmd start + cwd', async () => {
    const withWt = fake('win32', { env, files: [wt], dirs: [root] })
    await new ExternalOpener(withWt.deps).open(t, 'terminal', undefined, {})
    expect(withWt.spawned[0]).toMatchObject({ file: wt, args: ['-d', root] })
    const noWt = fake('win32', { env, dirs: [root] })
    await new ExternalOpener(noWt.deps).open(t, 'terminal', undefined, {})
    expect(noWt.spawned[0]).toMatchObject({ file: 'C:\\Windows\\System32\\cmd.exe', cwd: root })
  })

  it('경로에 역슬래시·드라이브가 든 상대 경로는 거부한다', async () => {
    const f = fake('win32', { env, dirs: [root] })
    const o = new ExternalOpener(f.deps)
    await expect(o.open(t, 'terminal', '..\\x', {})).rejects.toThrow()
    await expect(o.open(t, 'terminal', 'C:/x', {})).rejects.toThrow()
  })
})

describe('ExternalOpener: WSL / SSH', () => {
  const wslT: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'Ubuntu' }
  const unc = (p: string): string => '\\\\wsl.localhost\\Ubuntu' + p.replace(/\//g, '\\')
  const env = { LOCALAPPDATA: 'C:\\L' }
  const code = 'C:\\L\\Programs\\Microsoft VS Code\\Code.exe'
  const wslFs = { files: [code, unc('/home/u/repo/a.txt')], dirs: [unc('/home/u/repo')], env }

  it('WSL VS Code: code --remote wsl+<distro> <리눅스 경로>', async () => {
    const f = fake('win32', wslFs)
    await new ExternalOpener(f.deps).open(wslT, 'editor', 'a.txt', {})
    expect(f.spawned[0]).toMatchObject({ file: code, args: ['--remote', 'wsl+Ubuntu', '/home/u/repo', '/home/u/repo/a.txt'] })
  })

  it('WSL 파일 관리자: UNC 경로', async () => {
    const f = fake('win32', wslFs)
    await new ExternalOpener(f.deps).open(wslT, 'fileManager', 'a.txt', {})
    expect(f.shown).toEqual([{ type: 'showItem', path: unc('/home/u/repo/a.txt') }])
  })

  it('WSL 터미널: wsl.exe -d <distro> --cd <path>', async () => {
    const f = fake('win32', wslFs)
    await new ExternalOpener(f.deps).open(wslT, 'terminal', 'a.txt', {})
    expect(f.spawned[0]).toMatchObject({
      file: 'C:\\Windows\\System32\\cmd.exe',
      args: ['/d', '/c', 'start', '', 'C:\\Windows\\System32\\wsl.exe', '-d', 'Ubuntu', '--cd', '/home/u/repo']
    })
  })

  it('WSL 배포판 이름이 이상하면 UNC 를 만들기 전에 거부한다', async () => {
    const f = fake('win32', wslFs)
    await expect(new ExternalOpener(f.deps).open({ ...wslT, distro: '..\\evil' } as Target, 'fileManager', undefined, {})).rejects.toThrow(/배포판/)
    expect(f.shown).toEqual([])
  })

  it('WSL 링크가 저장소 밖을 가리키면 거부한다', async () => {
    const f = fake('win32', { ...wslFs, dirs: [...wslFs.dirs, unc('/etc')], links: { [unc('/home/u/repo/evil')]: unc('/etc') } })
    await expect(new ExternalOpener(f.deps).open(wslT, 'editor', 'evil', {})).rejects.toThrow(/심볼릭/)
    expect(f.spawned).toEqual([])
  })

  it('배포판이 꺼져 있어 UNC 를 풀 수 없으면 어휘 검사만으로 진행한다', async () => {
    const f = fake('win32', { env, files: [code] })
    await new ExternalOpener(f.deps).open(wslT, 'editor', 'a.txt', {})
    expect(f.spawned[0]?.args.slice(0, 2)).toEqual(['--remote', 'wsl+Ubuntu'])
    await expect(new ExternalOpener(f.deps).open(wslT, 'editor', '../x', {})).rejects.toThrow()
  })

  const sshT: Target = { kind: 'ssh', root: '/srv/repo', host: 'devbox' }

  it('SSH: VS Code 만. 파일 관리자·터미널·다른 에디터는 거부', async () => {
    const f = fake('linux', { files: ['/usr/bin/code'], env: { PATH: '/usr/bin' } })
    const o = new ExternalOpener(f.deps)
    await o.open(sshT, 'editor', 'a/b.txt', {})
    expect(f.spawned[0]?.args).toEqual(['--remote', 'ssh-remote+devbox', '/srv/repo', '/srv/repo/a/b.txt'])
    await expect(o.open(sshT, 'fileManager', undefined, {})).rejects.toThrow()
    await expect(o.open(sshT, 'terminal', undefined, {})).rejects.toThrow()
    await expect(o.open(sshT, 'editor', undefined, { editor: 'custom', editorExePath: '/opt/ide' })).rejects.toThrow()
    await expect(o.open(sshT, 'editor', '../x', {})).rejects.toThrow()
    expect(f.spawned).toHaveLength(1)
  })
})

describe('에디터 실행 파일', () => {
  it('Windows: 알려진 설치 위치의 .exe 를 찾는다', () => {
    const env = { LOCALAPPDATA: 'C:\\L', ProgramFiles: 'C:\\PF' }
    const f = fake('win32', { env, files: ['C:\\PF\\Microsoft VS Code\\Code.exe', 'C:\\L\\Programs\\cursor\\Cursor.exe'] })
    expect(findEditorExe('vscode', f.deps)).toBe('C:\\PF\\Microsoft VS Code\\Code.exe')
    expect(findEditorExe('cursor', f.deps)).toBe('C:\\L\\Programs\\cursor\\Cursor.exe')
    expect(findEditorExe('vscode', fake('win32', { env }).deps)).toBeNull()
  })

  it('저장소 폴더(상대 PATH 항목)는 PATH 탐색에 쓰지 않는다', () => {
    const f = fake('linux', { env: { PATH: '.:bin:/usr/bin' }, files: ['bin/code', './code', '/usr/bin/code'] })
    expect(findEditorExe('vscode', f.deps)).toBe('/usr/bin/code')
  })

  it('JetBrains·사용자 지정은 사용자가 고른 파일만 쓴다', () => {
    const f = fake('linux', { files: ['/opt/idea/bin/idea.sh'] })
    expect(() => resolveEditorExe('jetbrains', {}, f.deps)).toThrow(/먼저 고르세요/)
    expect(resolveEditorExe('custom', { editorExePath: '/opt/idea/bin/idea.sh' }, f.deps)).toBe('/opt/idea/bin/idea.sh')
    expect(() => resolveEditorExe('custom', { editorExePath: '/opt/gone' }, f.deps)).toThrow()
  })

  it('checkChosenExe: 절대 경로만, Windows 는 .exe/.com 만 그리고 네트워크 경로 불가', () => {
    expect(checkChosenExe('/opt/x', 'linux')).toBe('/opt/x')
    expect(() => checkChosenExe('x', 'linux')).toThrow()
    expect(() => checkChosenExe('C:\\a\\run.cmd', 'win32')).toThrow()
    expect(() => checkChosenExe('C:\\a\\run.bat', 'win32')).toThrow()
    expect(() => checkChosenExe('\\\\srv\\s\\a.exe', 'win32')).toThrow()
    expect(checkChosenExe('C:\\a\\idea64.exe', 'win32')).toBe('C:\\a\\idea64.exe')
    expect(() => checkChosenExe('/a\0b', 'linux')).toThrow()
  })
})
