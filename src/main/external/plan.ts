// 외부 도구로 열기의 순수 계산부: 경로 검증과 실행 인자 구성. electron·child_process 를 import 하지 않아 단위 테스트가 가능하다.
// 실제 실행(spawn·shell)은 opener.ts 가 맡는다.
//
// 원칙
// - 저장소 안의 파일 이름·경로는 "데이터"다. 항상 절대 경로로 만들어 넘기므로 옵션(-x, --remote …)으로 해석될 수 없다.
// - 실행할 프로그램은 앱이 찾은 설치 위치나 사용자가 설정 화면에서 고른 파일뿐이다(저장소 내용이 정하지 못한다).
// - 모든 실행은 인자 배열(shell:false)이다. cmd.exe 를 쓸 때도 사용자 경로는 명령줄이 아니라 cwd 로 넘긴다.
import { posix, win32 } from 'node:path'
import type { EditorKind } from '@shared/types'
import { uncFromWsl } from '../executors/wslPaths'

export type Target =
  | { kind: 'local'; root: string }
  | { kind: 'wsl'; root: string; distro: string }
  | { kind: 'ssh'; root: string; host: string }

export interface Launch {
  file: string
  args: string[]
  cwd?: string
}

const MAX_PATH_LEN = 4096
// 배포판·호스트 이름은 옵션으로 해석되거나 인자를 가르는 문자를 갖지 못한다
const DISTRO_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$/
const HOST_RE = /^[A-Za-z0-9][A-Za-z0-9._@:-]{0,199}$/

type PathMod = typeof posix | typeof win32
const pathMod = (platform: NodeJS.Platform): PathMod => (platform === 'win32' ? win32 : posix)

/**
 * 저장소 루트 기준 상대 경로('/' 구분)를 구성 요소로 나눈다. 없으면 루트(빈 배열).
 * 절대 경로·'..'·NUL·(Windows 로컬) 역슬래시·콜론(드라이브·대체 스트림)은 거부한다.
 */
export function relSegments(rel: unknown, target: Target, platform: NodeJS.Platform): string[] {
  if (rel === undefined || rel === '') return []
  if (typeof rel !== 'string' || rel.length > MAX_PATH_LEN || rel.includes('\0')) throw new Error('잘못된 경로입니다')
  if (rel.startsWith('/') || /^[A-Za-z]:/.test(rel)) throw new Error('저장소 밖의 경로입니다')
  const winLocal = target.kind === 'local' && platform === 'win32'
  if (winLocal && /[\\:]/.test(rel)) throw new Error('잘못된 경로입니다')
  const segs: string[] = []
  for (const s of rel.split('/')) {
    if (s === '' || s === '.') continue
    if (s === '..') throw new Error('저장소 밖의 경로입니다')
    segs.push(s)
  }
  return segs
}

/** 대상 환경의 절대 경로(로컬은 OS 경로, WSL·SSH 는 그 환경의 POSIX 경로). 루트가 절대 경로가 아니면 거부 */
export function absoluteTarget(target: Target, segs: string[], platform: NodeJS.Platform): string {
  const m = target.kind === 'local' ? pathMod(platform) : posix
  if (typeof target.root !== 'string' || !target.root || target.root.includes('\0') || !m.isAbsolute(target.root)) {
    throw new Error('저장소 경로가 절대 경로가 아닙니다')
  }
  if (target.kind === 'local' && platform === 'win32' && /^[\\/]{2}/.test(target.root)) throw new Error('네트워크 공유 경로는 지원하지 않습니다')
  return segs.length ? m.join(target.root, ...segs) : m.normalize(target.root)
}

export interface RealIo {
  /** 심볼릭 링크를 모두 푼 경로. 없는 경로면 throw */
  realpath(p: string): string
}

/**
 * 심볼릭 링크를 풀어도 저장소 루트 안에 있는지 확인한다. 대상이 없으면(삭제된 파일) 존재하는 가장 가까운 상위로 검사하고,
 * 그 상위 경로(`existing`)와 대상 자체가 존재하는지(`exists`)를 돌려준다.
 * 루트를 풀 수 없으면 null(호출자가 어휘적 검사만으로 갈지 정한다).
 */
export function checkRealContainment(
  root: string,
  abs: string,
  io: RealIo,
  m: PathMod
): { exists: boolean; existing: string; real: string; realRoot: string } | null {
  let realRoot: string
  try {
    realRoot = io.realpath(root)
  } catch {
    return null
  }
  const outside = (rel: string): boolean => rel === '..' || rel.startsWith('..' + m.sep) || m.isAbsolute(rel)
  let cur = abs
  let exists = true
  for (;;) {
    let real: string | null = null
    try {
      real = io.realpath(cur)
    } catch {
      real = null
    }
    if (real !== null) {
      if (outside(m.relative(realRoot, real))) throw new Error('저장소 밖을 가리키는 경로입니다(심볼릭 링크)')
      return { exists, existing: cur, real, realRoot }
    }
    const parent = m.dirname(cur)
    // 루트까지 올라가도 없으면 루트가 사라진 것
    if (parent === cur || outside(m.relative(root, parent))) throw new Error('경로를 확인할 수 없습니다')
    cur = parent
    exists = false
  }
}

export function assertSafeName(kind: 'distro' | 'host', v: unknown): string {
  const re = kind === 'distro' ? DISTRO_RE : HOST_RE
  if (typeof v !== 'string' || !re.test(v)) throw new Error(kind === 'distro' ? '잘못된 WSL 배포판 이름입니다' : '잘못된 SSH 호스트입니다')
  return v
}

/** 옵션으로 해석될 수 있는(-로 시작하는) 경로는 넘기지 않는다. 절대 경로면 일어나지 않는 방어선 */
function assertNotOption(p: string): string {
  if (p.startsWith('-') || p.includes('\0')) throw new Error('잘못된 경로입니다')
  return p
}

/**
 * 에디터 실행 인자. paths 는 모두 절대 경로(대상 환경 기준)이고 첫 항목이 저장소 루트다.
 * - VS Code·Cursor: 로컬은 경로만, WSL 은 `--remote wsl+<distro>`, SSH 는 `--remote ssh-remote+<host>`
 * - JetBrains·사용자 지정: 경로만 넘긴다. WSL 은 UNC 로 바꿔 넘기고, SSH 는 지원하지 않는다
 */
export function buildEditorLaunch(target: Target, paths: string[], editor: EditorKind, exe: string, platform: NodeJS.Platform): Launch {
  const list = [...new Set(paths)].map(assertNotOption)
  if (editor === 'vscode' || editor === 'cursor') {
    if (target.kind === 'wsl') return { file: exe, args: ['--remote', `wsl+${assertSafeName('distro', target.distro)}`, ...list] }
    if (target.kind === 'ssh') return { file: exe, args: ['--remote', `ssh-remote+${assertSafeName('host', target.host)}`, ...list] }
    return { file: exe, args: list }
  }
  if (target.kind === 'ssh') throw new Error('SSH 저장소는 VS Code·Cursor 로만 열 수 있습니다')
  if (target.kind === 'wsl') {
    if (platform !== 'win32') throw new Error('WSL 저장소는 Windows 에서만 열 수 있습니다')
    const distro = assertSafeName('distro', target.distro)
    return { file: exe, args: list.map((p) => assertNotOption(uncFromWsl(distro, p))) }
  }
  return { file: exe, args: list }
}

/** cmd.exe 를 거치는 WSL 터미널 대체 경로에서 허용하는 문자(셸 메타문자·% 확장을 원천 차단) */
const SAFE_CMD_PATH_RE = /^\/[A-Za-z0-9_./+=,:@-]*$/

/**
 * 터미널을 띄울 때 쓰는 외부 프로그램의 절대 경로.
 * 상대 이름(cmd.exe 등)은 cwd(=저장소 폴더)에서 먼저 찾아져 저장소가 심은 파일이 실행될 수 있으므로 모든 file 은 절대 경로여야 한다.
 */
export interface TerminalTools {
  /** Windows: 검증된 %SystemRoot% (없으면 C:\Windows) */
  systemRoot?: string
  /** Windows Terminal 의 절대 경로(없으면 null) */
  wt?: string | null
  /** 리눅스: 찾은 x-terminal-emulator 의 절대 경로 */
  linuxTerminal?: string | null
  /** macOS: Terminal.app 의 절대 경로(번들 ID 조회는 저장소의 가짜 앱에 가로채일 수 있어 경로로 고정한다) */
  macTerminal?: string | null
}

/** wt 는 인자를 다시 조합하며 ';' 를 구분자로, '"' 를 따옴표로 쓰므로 그런 문자나 제어 문자가 든 경로에는 쓰지 않는다 */
const wtSafe = (dir: string): boolean => !/[;"\u0000-\u001f\u007f]/.test(dir)

/**
 * 터미널 실행. dir 은 대상 환경의 절대 경로(디렉터리). 모든 file 은 절대 경로다.
 * Windows: Windows Terminal(wt)이 있으면 wt, 없으면 cmd.exe 의 start 로 새 콘솔(경로는 cwd 로만 넘긴다).
 * WSL 은 `wsl.exe -d <distro> --cd <path>`. SSH 는 지원하지 않는다.
 */
export function buildTerminalLaunch(target: Target, dir: string, platform: NodeJS.Platform, tools: TerminalTools): Launch {
  if (target.kind === 'ssh') throw new Error('SSH 저장소는 터미널로 열 수 없습니다')
  assertNotOption(dir)
  if (platform === 'win32') {
    const sys32 = win32.join(tools.systemRoot ?? 'C:\\Windows', 'System32')
    const cmd = win32.join(sys32, 'cmd.exe')
    const wt = tools.wt && wtSafe(dir) ? tools.wt : null
    if (target.kind === 'wsl') {
      const distro = assertSafeName('distro', target.distro)
      const wslArgs = [win32.join(sys32, 'wsl.exe'), '-d', distro, '--cd', dir]
      if (wt) return { file: wt, args: wslArgs }
      if (!SAFE_CMD_PATH_RE.test(dir)) throw new Error('이 경로는 Windows Terminal 없이 열 수 없습니다(영문·숫자·._-/ 만 가능)')
      return { file: cmd, args: ['/d', '/c', 'start', '', ...wslArgs] }
    }
    if (wt) return { file: wt, args: ['-d', dir] }
    return { file: cmd, args: ['/d', '/c', 'start', '', cmd], cwd: dir }
  }
  if (target.kind === 'wsl') throw new Error('WSL 저장소는 Windows 에서만 열 수 있습니다')
  if (platform === 'darwin') {
    if (!tools.macTerminal) throw new Error('Terminal.app 을 찾을 수 없습니다')
    return { file: '/usr/bin/open', args: ['-a', tools.macTerminal, dir] }
  }
  if (!tools.linuxTerminal) throw new Error('터미널 프로그램(x-terminal-emulator)을 찾을 수 없습니다')
  return { file: tools.linuxTerminal, args: [], cwd: dir }
}

/**
 * macOS 에서 openPath·open 이 "열기"가 아니라 "실행"할 수 있는 디렉터리(번들: .app .xcodeproj .service …)를 이름 목록으로 막을 수 없어
 * 허용 방식으로 판정한다: 저장소 루트부터 대상까지의 **실제 경로(realpath)** 의 모든 구성 요소 이름에 '.' 이 없어야 일반 폴더다.
 * 점이 든 폴더는 번들일 수 있으므로 true(= 폴더로 열지 않고 보여 주기만 한다). root·p 는 둘 다 realpath 여야 한다.
 */
export function hasDotComponent(realRoot: string, p: string): boolean {
  const rel = posix.relative(realRoot, p)
  return [posix.basename(realRoot), ...rel.split('/').filter(Boolean)].some((c) => c.includes('.'))
}

/** Windows 셸이 폴더 바로 가기(특수 폴더)로 해석하는 이름 확장: `name.{GUID}` */
const CLSID_RE = /\.\{[0-9a-f-]{36}\}$/i

/**
 * realRoot 에서 p 까지의 구성 요소(루트 포함) 중 이름이 `.{GUID}` 로 끝나는 첫 경로(그 구성 요소 자신의 경로). 없으면 null.
 * 이런 폴더는 일반 폴더로 보이지만 셸이 바로 가기로 다뤄 저장소 밖을 열 수 있다. Windows(win32 경로)용.
 */
export function clsidComponent(realRoot: string, p: string): string | null {
  if (CLSID_RE.test(win32.basename(realRoot))) return realRoot
  let cur = realRoot
  for (const n of win32.relative(realRoot, p).split(/[\\/]/).filter(Boolean)) {
    cur = win32.join(cur, n)
    if (CLSID_RE.test(n)) return cur
  }
  return null
}

/**
 * 파일 관리자 동작: openPath 는 **realpath 가 폴더**이고(macOS 는 점 없는 일반 폴더)일 때만 쓴다 —
 * 파일·번들은 ShellExecute·open 이 실행해 버린다. 그 외에는 폴더에서 선택해 보여 준다(showItemInFolder).
 * state.path·root 는 realpath(WSL 은 UNC)이고, 대상이 없으면 존재하는 가장 가까운 상위 폴더다.
 */
export interface RevealPlan {
  type: 'openPath' | 'showItem'
  path: string
}

export function buildRevealPlan(target: Target, state: { path: string; isDir: boolean; root: string }, platform: NodeJS.Platform): RevealPlan {
  if (target.kind === 'ssh') throw new Error('SSH 저장소는 파일 관리자로 열 수 없습니다')
  if (target.kind === 'wsl') {
    if (platform !== 'win32') throw new Error('WSL 저장소는 Windows 에서만 열 수 있습니다')
    assertSafeName('distro', target.distro)
  }
  if (platform === 'win32') {
    // 이름 확장 폴더(name.{CLSID})는 셸이 바로 가기로 다루므로 열지 않고 그 폴더를 부모에서 보여 준다
    const clsid = clsidComponent(state.root, state.path)
    if (clsid) return { type: 'showItem', path: clsid }
  }
  const plain = platform !== 'darwin' || !hasDotComponent(state.root, state.path)
  return { type: state.isDir && plain ? 'openPath' : 'showItem', path: state.path }
}
