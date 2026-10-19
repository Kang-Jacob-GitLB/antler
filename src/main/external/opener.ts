// 외부 도구로 열기의 실행부. electron(shell)·child_process(spawn)·fs 는 주입받아 테스트에서 가짜로 바꾼다.
// 저장소에는 아무것도 쓰지 않는다 — 폴더·파일을 가리켜 외부 프로그램을 띄울 뿐이다.
import { posix, win32 } from 'node:path'
import type { EditorKind, ExternalAction, UiPrefs } from '@shared/types'
import { uncFromWsl, wslFromUnc } from '../executors/wslPaths'
import {
  absoluteTarget,
  buildEditorLaunch,
  buildRevealPlan,
  buildTerminalLaunch,
  assertSafeName,
  clsidComponent,
  hasDotComponent,
  type TerminalTools,
  checkRealContainment,
  relSegments,
  type Launch,
  type Target
} from './plan'

interface StatLike {
  isDirectory(): boolean
  isFile(): boolean
}

export interface ChildLike {
  once(event: 'spawn' | 'error', cb: (err?: Error) => void): unknown
  unref(): void
}

export interface OpenerDeps {
  platform: NodeJS.Platform
  env: NodeJS.ProcessEnv
  fs: {
    realpathSync(p: string): string
    statSync(p: string): StatLike
    /** 링크 자체를 본다(Windows 앱 실행 별칭 wt.exe 는 statSync 가 EACCES 로 실패한다) */
    lstatSync(p: string): StatLike
  }
  spawn(file: string, args: string[], opts: { cwd?: string; detached: true; stdio: 'ignore'; shell: false; windowsHide: false; env: NodeJS.ProcessEnv }): ChildLike
  shell: { openPath(p: string): Promise<string>; showItemInFolder(p: string): void }
}

const isFileSync = (deps: OpenerDeps, p: string): boolean => {
  try {
    return deps.fs.statSync(p).isFile()
  } catch {
    return false
  }
}

/** PATH 의 폴더에서 실행 파일 이름을 찾아 절대 경로로 돌려준다(리눅스) */
function findInPath(deps: OpenerDeps, name: string): string | null {
  for (const dir of (deps.env['PATH'] ?? '').split(posix.delimiter)) {
    if (!dir || !posix.isAbsolute(dir)) continue
    const p = posix.join(dir, name)
    if (isFileSync(deps, p)) return p
  }
  return null
}

/**
 * VS Code·Cursor 의 실행 파일. 저장소가 아니라 앱이 아는 설치 위치에서만 찾는다.
 * Windows 는 .cmd 셰임이 아니라 실제 .exe 를 쓴다(shell:false 로는 .cmd 를 실행할 수 없다).
 */
export function findEditorExe(kind: 'vscode' | 'cursor', deps: OpenerDeps): string | null {
  const env = deps.env
  const candidates: string[] = []
  if (deps.platform === 'win32') {
    const local = env['LOCALAPPDATA']
    const pf = env['ProgramFiles']
    const pf86 = env['ProgramFiles(x86)']
    const sub = kind === 'vscode' ? ['Microsoft VS Code', 'Code.exe'] : ['cursor', 'Cursor.exe']
    if (local) candidates.push(win32.join(local, 'Programs', ...sub))
    if (kind === 'vscode') {
      if (pf) candidates.push(win32.join(pf, ...sub))
      if (pf86) candidates.push(win32.join(pf86, ...sub))
    } else if (pf) candidates.push(win32.join(pf, ...sub))
  } else if (deps.platform === 'darwin') {
    candidates.push(
      kind === 'vscode'
        ? '/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code'
        : '/Applications/Cursor.app/Contents/Resources/app/bin/cursor'
    )
  } else {
    const name = kind === 'vscode' ? 'code' : 'cursor'
    const found = findInPath(deps, name)
    if (found) candidates.push(found)
    candidates.push(`/usr/bin/${name}`, `/usr/local/bin/${name}`, `/snap/bin/${name}`)
  }
  return candidates.find((p) => isFileSync(deps, p)) ?? null
}

/** 사용자가 설정 화면에서 고른 실행 파일을 다시 확인한다(절대 경로·존재·Windows 는 .exe/.com 만) */
export function checkChosenExe(p: unknown, platform: NodeJS.Platform, deps?: OpenerDeps): string {
  if (typeof p !== 'string' || !p || p.length > 4096 || p.includes('\0')) throw new Error('잘못된 실행 파일 경로입니다')
  const m = platform === 'win32' ? win32 : posix
  if (!m.isAbsolute(p) || (platform === 'win32' && /^[\\/]{2}/.test(p))) throw new Error('로컬 폴더의 실행 파일만 고를 수 있습니다')
  if (platform === 'win32' && !/\.(exe|com)$/i.test(p)) throw new Error('.exe 파일만 고를 수 있습니다(.cmd·.bat 은 안전하게 실행할 수 없습니다)')
  if (deps && !isFileSync(deps, p)) throw new Error('실행 파일을 찾을 수 없습니다. 설정에서 다시 선택하세요')
  return p
}

export function resolveEditorExe(editor: EditorKind, ui: Pick<UiPrefs, 'editorExePath'>, deps: OpenerDeps): string {
  if (editor === 'vscode' || editor === 'cursor') {
    const exe = findEditorExe(editor, deps)
    if (!exe) throw new Error(`${editor === 'vscode' ? 'VS Code' : 'Cursor'} 를 찾을 수 없습니다. 설정에서 다른 에디터를 고르세요`)
    return exe
  }
  if (!ui.editorExePath) throw new Error('에디터 실행 파일을 먼저 고르세요(설정)')
  return checkChosenExe(ui.editorExePath, deps.platform, deps)
}

const WIN_DRIVE_DIR_RE = /^[A-Za-z]:\\[^\0"<>|?*;]*$/

/** 터미널로 띄울 프로그램의 절대 경로를 찾는다. 상대 이름은 쓰지 않는다(cwd 의 저장소 파일이 먼저 실행될 수 있다) */
export function terminalTools(deps: OpenerDeps): TerminalTools {
  if (deps.platform === 'win32') {
    const root = deps.env['SystemRoot']
    const systemRoot = root && WIN_DRIVE_DIR_RE.test(root) ? root : 'C:\\Windows'
    const local = deps.env['LOCALAPPDATA']
    let wt: string | null = null
    if (local && WIN_DRIVE_DIR_RE.test(local)) {
      const p = win32.join(local, 'Microsoft', 'WindowsApps', 'wt.exe')
      try {
        deps.fs.lstatSync(p)
        wt = p
      } catch {
        wt = null
      }
    }
    return { systemRoot, wt }
  }
  if (deps.platform === 'darwin') {
    // 번들 ID 조회(open -b)는 저장소의 가짜 Terminal.app 에 가로채일 수 있어 시스템 위치의 절대 경로로 고정한다
    const dirs = ['/System/Applications/Utilities/Terminal.app', '/Applications/Utilities/Terminal.app']
    return {
      macTerminal:
        dirs.find((d) => {
          try {
            return deps.fs.lstatSync(d).isDirectory()
          } catch {
            return false
          }
        }) ?? null
    }
  }
  const term = 'x-terminal-emulator'
  return { linuxTerminal: findInPath(deps, term) ?? (isFileSync(deps, `/usr/bin/${term}`) ? `/usr/bin/${term}` : null) }
}

/** 외부 프로그램을 분리된 프로세스로 띄우고, 실행에 실패(ENOENT 등)하면 그 오류를 돌려준다 */
function launch(deps: OpenerDeps, l: Launch): Promise<void> {
  // Electron 을 Node 로 돌리는 변수가 에디터(Electron 앱)에 새지 않게 한다
  const env = { ...deps.env }
  delete env['ELECTRON_RUN_AS_NODE']
  return new Promise((resolve, reject) => {
    const child = deps.spawn(l.file, l.args, {
      ...(l.cwd ? { cwd: l.cwd } : {}),
      detached: true,
      stdio: 'ignore',
      shell: false,
      windowsHide: false,
      env
    })
    child.once('error', (err) => reject(new Error(`실행하지 못했습니다: ${err?.message ?? l.file}`)))
    child.once('spawn', () => {
      child.unref()
      resolve()
    })
  })
}

export class ExternalOpener {
  constructor(private readonly deps: OpenerDeps) {}

  /** root 는 저장소 최상위, rel 은 그 안의 경로. 검증을 통과한 경우에만 외부 프로그램을 띄운다 */
  async open(target: Target, action: ExternalAction, rel: unknown, ui: Pick<UiPrefs, 'editor' | 'editorExePath'>): Promise<void> {
    const { deps } = this
    const segs = relSegments(rel, target, deps.platform)
    const abs = absoluteTarget(target, segs, deps.platform)
    const root = absoluteTarget(target, [], deps.platform)
    const state = this.inspect(target, root, abs)

    if (action === 'fileManager') {
      const plan = buildRevealPlan(target, { path: state.real, isDir: state.isDir, root: state.realRoot }, deps.platform)
      if (plan.type === 'showItem') deps.shell.showItemInFolder(plan.path)
      else {
        const err = await deps.shell.openPath(plan.path)
        if (err) throw new Error(`파일 관리자를 열지 못했습니다: ${err}`)
      }
      return
    }
    if (action === 'editor') {
      const editor = ui.editor ?? 'vscode'
      const exe = resolveEditorExe(editor, ui, deps)
      // 저장소 폴더를 열고, 파일이면 그 파일도 함께 연다(없는 파일은 폴더만)
      const paths = state.exists && abs !== root ? [root, abs] : [root]
      await launch(deps, buildEditorLaunch(target, paths, editor, exe, deps.platform))
      return
    }
    if (action === 'terminal') {
      const dm = deps.platform === 'win32' ? win32 : posix
      // 터미널은 realpath 가 폴더인 곳에서만 연다(파일·번들을 넘기면 open 이 실행해 버린다)
      let dir = state.exists && !state.isDir ? dm.dirname(state.real) : state.real
      if (deps.platform === 'darwin') {
        // 점이 든 폴더(번들일 수 있음)는 피해 점 없는 가장 가까운 상위로 올라간다. 저장소 루트까지 없으면 거부
        while (hasDotComponent(state.realRoot, dir)) {
          if (dir === state.realRoot || dir === dm.dirname(dir)) throw new Error('이 저장소 경로에는 번들일 수 있는 폴더(이름에 .)가 있어 터미널을 열 수 없습니다')
          dir = dm.dirname(dir)
        }
      }
      if (deps.platform === 'win32') {
        // 이름 확장 폴더(name.{CLSID})는 셸이 바로 가기로 다루므로 피해 가장 가까운 상위로 올라간다. 루트까지 없으면 거부
        while (clsidComponent(state.realRoot, dir)) {
          if (dir === state.realRoot || dir === dm.dirname(dir)) throw new Error('이 저장소 경로에는 폴더 바로 가기로 해석되는 이름(.{GUID})이 있어 터미널을 열 수 없습니다')
          dir = dm.dirname(dir)
        }
      }
      if (target.kind === 'wsl') dir = wslFromUnc(dir)?.path ?? root
      await launch(deps, buildTerminalLaunch(target, dir, deps.platform, terminalTools(deps)))
      return
    }
    throw new Error('알 수 없는 동작입니다')
  }

  /**
   * 대상이 있는지·폴더인지, 없으면 존재하는 가장 가까운 상위를 구한다. 심볼릭 링크를 풀어도 저장소 안인지 확인한다.
   * 로컬은 반드시 확인한다. WSL 은 UNC 경로로 확인하되 풀 수 없으면(배포판이 꺼져 있는 등) 어휘적 검사만 한다.
   * SSH 는 원격이라 확인할 수 없다(어휘적 검사만) — 원격 쪽 링크는 그 호스트의 에디터가 다룬다.
   */
  private inspect(target: Target, root: string, abs: string): Inspected {
    const { deps } = this
    if (target.kind === 'ssh') return { exists: true, isDir: abs === root, real: abs, realRoot: root }
    const wsl = target.kind === 'wsl'
    // 배포판 이름은 UNC 경로에 쓰기 전에 검증한다
    if (wsl) assertSafeName('distro', target.distro)
    if (wsl && deps.platform !== 'win32') throw new Error('WSL 저장소는 Windows 에서만 열 수 있습니다')
    const m = !wsl && deps.platform !== 'win32' ? posix : win32
    const nRoot = wsl ? uncFromWsl(target.distro, root) : root
    const nAbs = wsl ? uncFromWsl(target.distro, abs) : abs
    const io = { realpath: (p: string): string => deps.fs.realpathSync(p) }
    const checked = checkRealContainment(nRoot, nAbs, io, m)
    if (!checked) {
      if (!wsl) throw new Error('저장소 폴더를 찾을 수 없습니다')
      // 배포판이 꺼져 있는 등 풀 수 없으면 어휘 경로를 그대로 쓰되 폴더로 취급하지 않는다(openPath 안 함)
      return { exists: true, isDir: false, real: nAbs, realRoot: nRoot }
    }
    const isDirectory = (p: string): boolean => {
      try {
        return deps.fs.statSync(p).isDirectory()
      } catch {
        return false
      }
    }
    // 모든 판정은 링크를 푼 실제 경로(real) 기준이다. 대상이 없으면 존재하는 가장 가까운 상위이고,
    // 그 상위가 파일이면(evil.exe/x → evil.exe) 폴더가 될 때까지 한 단계씩 올라간다
    let real = checked.real
    if (checked.exists) return { exists: true, isDir: isDirectory(real), real, realRoot: checked.realRoot }
    while (real !== checked.realRoot && !isDirectory(real)) real = m.dirname(real)
    return { exists: false, isDir: true, real, realRoot: checked.realRoot }
  }
}

/** realpath 기준 검사 결과. real 은 대상(없으면 가장 가까운 상위 폴더)의 실제 경로, WSL 은 UNC 형태 */
interface Inspected {
  exists: boolean
  isDir: boolean
  real: string
  realRoot: string
}
