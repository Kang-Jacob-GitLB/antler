import { describe, expect, it } from 'vitest'
import { posix, win32 } from 'node:path'
import {
  absoluteTarget,
  buildEditorLaunch,
  buildTerminalLaunch,
  checkRealContainment,
  relSegments,
  type Target
} from '../src/main/external/plan'

const local: Target = { kind: 'local', root: '/home/u/repo' }
const winLocal: Target = { kind: 'local', root: 'C:\\work\\repo' }
const wsl: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'Ubuntu-22.04' }
const ssh: Target = { kind: 'ssh', root: '/srv/repo', host: 'devbox' }

describe('relSegments: 저장소 안의 상대 경로 검증', () => {
  it('비어 있으면 루트, 중복 슬래시와 . 은 정리한다', () => {
    expect(relSegments(undefined, local, 'linux')).toEqual([])
    expect(relSegments('', local, 'linux')).toEqual([])
    expect(relSegments('a//b/./c.txt', local, 'linux')).toEqual(['a', 'b', 'c.txt'])
  })

  it('.. 와 절대 경로는 거부한다', () => {
    for (const bad of ['..', '../x', 'a/../../x', 'a/..', '/etc/passwd', 'C:/Windows', 'c:x']) {
      expect(() => relSegments(bad, local, 'linux'), bad).toThrow()
    }
  })

  it('NUL·비문자열·너무 긴 경로는 거부한다', () => {
    expect(() => relSegments('a\0b', local, 'linux')).toThrow()
    expect(() => relSegments(42, local, 'linux')).toThrow()
    expect(() => relSegments('a/'.repeat(3000), local, 'linux')).toThrow()
  })

  it('Windows 로컬에서는 역슬래시·콜론(대체 데이터 스트림)을 거부한다', () => {
    expect(() => relSegments('a\\..\\b', winLocal, 'win32')).toThrow()
    expect(() => relSegments('a.txt:stream', winLocal, 'win32')).toThrow()
    // 리눅스 파일 이름에는 역슬래시·콜론이 있을 수 있다
    expect(relSegments('a\\b:c', local, 'linux')).toEqual(['a\\b:c'])
    expect(relSegments('a\\b', wsl, 'win32')).toEqual(['a\\b'])
  })

  it('옵션처럼 보이는 파일 이름도 그냥 이름이다', () => {
    expect(relSegments('-rf', local, 'linux')).toEqual(['-rf'])
    expect(relSegments('--remote', local, 'linux')).toEqual(['--remote'])
  })
})

describe('absoluteTarget', () => {
  it('플랫폼에 맞게 합친다', () => {
    expect(absoluteTarget(local, ['a', 'b'], 'linux')).toBe('/home/u/repo/a/b')
    expect(absoluteTarget(winLocal, ['a', 'b'], 'win32')).toBe('C:\\work\\repo\\a\\b')
    expect(absoluteTarget(wsl, ['x'], 'win32')).toBe('/home/u/repo/x')
    expect(absoluteTarget(ssh, [], 'linux')).toBe('/srv/repo')
  })

  it('옵션 이름 파일도 절대 경로가 되어 옵션으로 해석될 수 없다', () => {
    expect(absoluteTarget(local, ['--remote'], 'linux')).toBe('/home/u/repo/--remote')
    expect(absoluteTarget(winLocal, ['-x'], 'win32')).toBe('C:\\work\\repo\\-x')
  })

  it('절대 경로가 아닌 루트와 네트워크 공유 루트는 거부한다', () => {
    expect(() => absoluteTarget({ kind: 'local', root: 'repo' }, [], 'linux')).toThrow()
    expect(() => absoluteTarget({ kind: 'ssh', root: '~/repo', host: 'h' }, [], 'linux')).toThrow()
    expect(() => absoluteTarget({ kind: 'local', root: '\\\\srv\\share\\repo' }, [], 'win32')).toThrow()
  })
})

describe('checkRealContainment: 심볼릭 링크로 저장소 밖에 나가지 않는다', () => {
  // links: 경로 → 푼 결과. 목록에 없는 경로는 존재하지 않는다
  const io = (links: Record<string, string>) => ({
    realpath: (p: string): string => {
      const v = links[p]
      if (v === undefined) throw new Error('ENOENT')
      return v
    }
  })

  it('저장소 안의 파일은 통과한다', () => {
    const r = checkRealContainment('/r', '/r/a.txt', io({ '/r': '/r', '/r/a.txt': '/r/a.txt' }), posix)
    expect(r).toMatchObject({ exists: true, existing: '/r/a.txt', real: '/r/a.txt', realRoot: '/r' })
  })

  it('밖을 가리키는 링크는 거부한다', () => {
    expect(() => checkRealContainment('/r', '/r/link', io({ '/r': '/r', '/r/link': '/etc' }), posix)).toThrow(/심볼릭/)
    // 이름이 비슷한 형제 폴더(/r2)도 밖이다
    expect(() => checkRealContainment('/r', '/r/link', io({ '/r': '/r', '/r/link': '/r2/x' }), posix)).toThrow()
  })

  it('폴더 링크를 거쳐 밖으로 나가는 하위 파일도 거부한다', () => {
    expect(() => checkRealContainment('/r', '/r/d/x', io({ '/r': '/r', '/r/d': '/tmp/out', '/r/d/x': '/tmp/out/x' }), posix)).toThrow()
  })

  it('루트 자체가 링크여도 푼 루트 기준으로 비교한다', () => {
    const r = checkRealContainment('/link', '/link/a', io({ '/link': '/real', '/link/a': '/real/a' }), posix)
    expect(r?.exists).toBe(true)
  })

  it('없는 파일은 존재하는 가장 가까운 상위로 확인한다', () => {
    const r = checkRealContainment('/r', '/r/d/gone.txt', io({ '/r': '/r', '/r/d': '/r/d' }), posix)
    expect(r).toMatchObject({ exists: false, existing: '/r/d', real: '/r/d' })
    // 그 상위가 밖을 가리키면 거부
    expect(() => checkRealContainment('/r', '/r/d/gone.txt', io({ '/r': '/r', '/r/d': '/etc' }), posix)).toThrow()
  })

  it('루트를 풀 수 없으면 null', () => {
    expect(checkRealContainment('/r', '/r/a', io({}), posix)).toBeNull()
  })

  it('Windows 경로는 대소문자를 구분하지 않고 비교한다', () => {
    const r = checkRealContainment('C:\\Repo', 'C:\\Repo\\a', io({ 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\a': 'c:\\repo\\a' }), win32)
    expect(r?.exists).toBe(true)
    expect(() => checkRealContainment('C:\\Repo', 'C:\\Repo\\a', io({ 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\a': 'D:\\x' }), win32)).toThrow()
  })
})

describe('buildEditorLaunch', () => {
  it('VS Code 로컬: 경로만', () => {
    expect(buildEditorLaunch(local, ['/home/u/repo', '/home/u/repo/-x'], 'vscode', '/usr/bin/code', 'linux')).toEqual({
      file: '/usr/bin/code',
      args: ['/home/u/repo', '/home/u/repo/-x']
    })
  })

  it('VS Code WSL: --remote wsl+<distro>', () => {
    expect(buildEditorLaunch(wsl, ['/home/u/repo', '/home/u/repo/--help'], 'vscode', 'Code.exe', 'win32').args).toEqual([
      '--remote',
      'wsl+Ubuntu-22.04',
      '/home/u/repo',
      '/home/u/repo/--help'
    ])
  })

  it('VS Code·Cursor SSH: --remote ssh-remote+<host>', () => {
    expect(buildEditorLaunch(ssh, ['/srv/repo'], 'vscode', 'code', 'linux').args).toEqual(['--remote', 'ssh-remote+devbox', '/srv/repo'])
    expect(buildEditorLaunch({ ...ssh, host: 'me@dev-box.example.com' }, ['/srv/repo'], 'cursor', 'cursor', 'linux').args).toEqual([
      '--remote',
      'ssh-remote+me@dev-box.example.com',
      '/srv/repo'
    ])
  })

  it('JetBrains·사용자 지정: 로컬은 경로, WSL 은 UNC, SSH 는 거부', () => {
    expect(buildEditorLaunch(winLocal, ['C:\\work\\repo'], 'jetbrains', 'idea64.exe', 'win32').args).toEqual(['C:\\work\\repo'])
    expect(buildEditorLaunch(wsl, ['/home/u/repo', '/home/u/repo/a b.txt'], 'custom', 'x.exe', 'win32').args).toEqual([
      '\\\\wsl.localhost\\Ubuntu-22.04\\home\\u\\repo',
      '\\\\wsl.localhost\\Ubuntu-22.04\\home\\u\\repo\\a b.txt'
    ])
    expect(() => buildEditorLaunch(ssh, ['/srv/repo'], 'jetbrains', 'x', 'linux')).toThrow()
    expect(() => buildEditorLaunch(wsl, ['/home/u/repo'], 'custom', 'x', 'linux')).toThrow()
  })

  it('옵션처럼 보이는 경로·이름은 거부한다', () => {
    expect(() => buildEditorLaunch(local, ['--remote'], 'vscode', 'code', 'linux')).toThrow()
    expect(() => buildEditorLaunch({ ...wsl, distro: '--exec' }, ['/r'], 'vscode', 'code', 'win32')).toThrow()
    expect(() => buildEditorLaunch({ ...wsl, distro: 'a b' }, ['/r'], 'vscode', 'code', 'win32')).toThrow()
    expect(() => buildEditorLaunch({ ...ssh, host: '-oProxyCommand=x' }, ['/r'], 'vscode', 'code', 'linux')).toThrow()
    expect(() => buildEditorLaunch({ ...ssh, host: 'h;rm' }, ['/r'], 'vscode', 'code', 'linux')).toThrow()
  })
})

const WT = 'C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\wt.exe'
const SYS32 = 'C:\\Windows\\System32'
const withWt = { systemRoot: 'C:\\Windows', wt: WT }
const noWt = { systemRoot: 'C:\\Windows', wt: null }

describe('buildTerminalLaunch', () => {
  it('Windows 로컬: wt 가 있으면 wt -d, 없으면 절대 경로 cmd start 에 cwd 로 넘긴다', () => {
    expect(buildTerminalLaunch(winLocal, 'C:\\work\\repo', 'win32', withWt)).toEqual({ file: WT, args: ['-d', 'C:\\work\\repo'] })
    const fb = buildTerminalLaunch(winLocal, 'C:\\work\\a&b', 'win32', noWt)
    expect(fb.file).toBe(`${SYS32}\\cmd.exe`)
    expect(fb.args).toEqual(['/d', '/c', 'start', '', `${SYS32}\\cmd.exe`])
    expect(fb.cwd).toBe('C:\\work\\a&b')
    // 경로는 명령줄에 들어가지 않는다
    expect(fb.args.join(' ')).not.toContain('a&b')
  })

  it("wt 는 ';'·'\"'·제어 문자가 든 경로에 쓰지 않는다", () => {
    for (const d of ['C:\\a;b', 'C:\\a"b', 'C:\\a\nb']) {
      expect(buildTerminalLaunch(winLocal, d, 'win32', withWt).file).toBe(`${SYS32}\\cmd.exe`)
    }
  })

  it('WSL: wsl.exe(절대 경로) -d <distro> --cd <path>', () => {
    expect(buildTerminalLaunch(wsl, '/home/u/repo', 'win32', withWt)).toEqual({
      file: WT,
      args: [`${SYS32}\\wsl.exe`, '-d', 'Ubuntu-22.04', '--cd', '/home/u/repo']
    })
    expect(buildTerminalLaunch(wsl, '/home/u/repo', 'win32', noWt)).toEqual({
      file: `${SYS32}\\cmd.exe`,
      args: ['/d', '/c', 'start', '', `${SYS32}\\wsl.exe`, '-d', 'Ubuntu-22.04', '--cd', '/home/u/repo']
    })
  })

  it('WSL 에서 wt 를 쓸 수 없을 때 셸 메타문자·따옴표가 든 경로는 거부한다', () => {
    for (const p of ['/r/a&calc', '/r/%PATH%', '/r/a b', '/r/"x', '/r/a|b', '/r/^x', '/r/a;b']) {
      expect(() => buildTerminalLaunch(wsl, p, 'win32', noWt), p).toThrow()
    }
    // wt 가 있어도 따옴표·';' 가 든 경로는 wt 로 넘기지 않으므로 거부된다(wt 의 인자 재조합으로 --cd 를 끊을 수 없다)
    expect(() => buildTerminalLaunch(wsl, '/r/a" --exec x', 'win32', withWt)).toThrow()
    expect(() => buildTerminalLaunch(wsl, '/r/a;b', 'win32', withWt)).toThrow()
    // 공백만 있는 경로는 인자 배열로 안전하게 넘어간다
    expect(buildTerminalLaunch(wsl, '/r/a b', 'win32', withWt).args).toContain('/r/a b')
  })

  it('SSH 는 지원하지 않고, mac·리눅스는 절대 경로 터미널', () => {
    expect(() => buildTerminalLaunch(ssh, '/srv/repo', 'linux', {})).toThrow()
    expect(buildTerminalLaunch(local, '/home/u/repo', 'darwin', { macTerminal: '/System/Applications/Utilities/Terminal.app' })).toEqual({ file: '/usr/bin/open', args: ['-a', '/System/Applications/Utilities/Terminal.app', '/home/u/repo'] })
    expect(() => buildTerminalLaunch(local, '/home/u/repo', 'darwin', {})).toThrow(/Terminal/)
    expect(buildTerminalLaunch(local, '/home/u/repo', 'linux', { linuxTerminal: '/usr/bin/x-terminal-emulator' })).toEqual({
      file: '/usr/bin/x-terminal-emulator',
      args: [],
      cwd: '/home/u/repo'
    })
    expect(() => buildTerminalLaunch(local, '/home/u/repo', 'linux', {})).toThrow(/찾을 수 없/)
    expect(() => buildTerminalLaunch(wsl, '/r', 'linux', {})).toThrow()
  })

  it('실행하는 모든 프로그램(file)과 start 대상은 절대 경로다', () => {
    const isAbs = (p: string): boolean => /^([A-Za-z]:\\|\/)/.test(p)
    const cases = [
      buildTerminalLaunch(winLocal, 'C:\\r', 'win32', withWt),
      buildTerminalLaunch(winLocal, 'C:\\r', 'win32', noWt),
      buildTerminalLaunch(winLocal, 'C:\\r', 'win32', {}),
      buildTerminalLaunch(wsl, '/r', 'win32', withWt),
      buildTerminalLaunch(wsl, '/r', 'win32', noWt),
      buildTerminalLaunch(local, '/r', 'darwin', { macTerminal: '/System/Applications/Utilities/Terminal.app' }),
      buildTerminalLaunch(local, '/r', 'linux', { linuxTerminal: '/usr/bin/x-terminal-emulator' })
    ]
    for (const l of cases) {
      expect(isAbs(l.file), l.file).toBe(true)
      const i = l.args.indexOf('start')
      if (i >= 0) expect(isAbs(l.args[i + 2]!), l.args.join(' ')).toBe(true)
      if (l.file === WT && l.args[0] !== '-d') expect(isAbs(l.args[0]!)).toBe(true)
    }
  })
})

