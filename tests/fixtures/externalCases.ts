// 외부 도구로 열기(src/main/external)의 입력 표. tests/goldenExternal.test.ts 가 TS 구현으로 골든(tests/golden/external/*.json)을 만들고,
// Rust 이식(src-tauri/src/external, 검사: src-tauri/tests/external.rs)이 같은 입력에서 같은 결과(argv 계획·오류 문구)를 내는지 본다.
// 이름이 기존 TS 테스트(tests/external*.test.ts)의 `it` 제목과 같은 사례는 그 `it` 의 단언을 옮긴 것이다(대응표 tests/golden/external/test-map.json).
// 경로·플랫폼은 입력이다 — plan.ts 는 플랫폼을 인자로 받아 `node:path` 의 win32·posix 를 고르므로 어느 OS 에서 돌려도 같은 골든이 나온다.

import type { EditorKind, ExternalAction } from '../../src/shared/types'
import type { Target } from '../../src/main/external/plan'

export type Plat = 'win32' | 'linux' | 'darwin'

const GUID = '{0AFACED1-E828-11D1-9187-B532F1E9575D}'

const local: Target = { kind: 'local', root: '/home/u/repo' }
const winLocal: Target = { kind: 'local', root: 'C:\\work\\repo' }
const wsl: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'Ubuntu-22.04' }
const ssh: Target = { kind: 'ssh', root: '/srv/repo', host: 'devbox' }

// ── relSegments ───────────────────────────────────────────────────────────────

export interface RelCase {
  name: string
  /** 생략하면 `undefined`(JSON 에서는 키가 없다) */
  rel?: unknown
  target: Target
  platform: Plat
}

export const REL_CASES: RelCase[] = [
  { name: '생략', target: local, platform: 'linux' },
  { name: '빈 문자열', rel: '', target: local, platform: 'linux' },
  { name: '중복 슬래시와 .', rel: 'a//b/./c.txt', target: local, platform: 'linux' },
  { name: '끝 슬래시', rel: 'a/b/', target: local, platform: 'linux' },
  ...['..', '../x', 'a/../../x', 'a/..', '/etc/passwd', 'C:/Windows', 'c:x', '/'].map((rel): RelCase => ({ name: `거부 ${rel}`, rel, target: local, platform: 'linux' })),
  { name: 'NUL', rel: 'a\0b', target: local, platform: 'linux' },
  { name: '숫자', rel: 42, target: local, platform: 'linux' },
  { name: '불리언', rel: true, target: local, platform: 'linux' },
  { name: '배열', rel: ['a'], target: local, platform: 'linux' },
  { name: '객체', rel: {}, target: local, platform: 'linux' },
  { name: '너무 긴 경로', rel: 'a/'.repeat(3000), target: local, platform: 'linux' },
  { name: '4096 단위 경계(통과)', rel: 'a'.repeat(4096), target: local, platform: 'linux' },
  { name: '4097 단위(거부)', rel: 'a'.repeat(4097), target: local, platform: 'linux' },
  { name: '이모지는 2단위(통과)', rel: '😀'.repeat(2048), target: local, platform: 'linux' },
  { name: '이모지는 2단위(거부)', rel: '😀'.repeat(2049), target: local, platform: 'linux' },
  { name: 'Windows 로컬 역슬래시', rel: 'a\\..\\b', target: winLocal, platform: 'win32' },
  { name: 'Windows 로컬 대체 데이터 스트림', rel: 'a.txt:stream', target: winLocal, platform: 'win32' },
  { name: 'Windows 로컬 정상', rel: 'src/a b/c.txt', target: winLocal, platform: 'win32' },
  { name: '리눅스 로컬은 역슬래시·콜론 가능', rel: 'a\\b:c', target: local, platform: 'linux' },
  { name: 'WSL 은 Windows 에서도 역슬래시 가능', rel: 'a\\b', target: wsl, platform: 'win32' },
  { name: 'SSH 는 콜론 가능', rel: 'a:b', target: ssh, platform: 'win32' },
  { name: '옵션처럼 보이는 이름 -rf', rel: '-rf', target: local, platform: 'linux' },
  { name: '옵션처럼 보이는 이름 --remote', rel: '--remote', target: local, platform: 'linux' },
  { name: '점이 든 이름은 그냥 이름', rel: '.../x/..y/.hidden', target: local, platform: 'linux' }
]

// ── absoluteTarget ────────────────────────────────────────────────────────────

export interface AbsCase {
  name: string
  target: Target
  segs: string[]
  platform: Plat
}

export const ABS_CASES: AbsCase[] = [
  { name: '리눅스 로컬', target: local, segs: ['a', 'b'], platform: 'linux' },
  { name: 'Windows 로컬', target: winLocal, segs: ['a', 'b'], platform: 'win32' },
  { name: 'WSL', target: wsl, segs: ['x'], platform: 'win32' },
  { name: 'SSH 루트', target: ssh, segs: [], platform: 'linux' },
  { name: '옵션 이름 파일 리눅스', target: local, segs: ['--remote'], platform: 'linux' },
  { name: '옵션 이름 파일 Windows', target: winLocal, segs: ['-x'], platform: 'win32' },
  { name: '상대 루트', target: { kind: 'local', root: 'repo' }, segs: [], platform: 'linux' },
  { name: '~ 루트', target: { kind: 'ssh', root: '~/repo', host: 'h' }, segs: [], platform: 'linux' },
  { name: '네트워크 공유 루트', target: { kind: 'local', root: '\\\\srv\\share\\repo' }, segs: [], platform: 'win32' },
  { name: '슬래시 네트워크 공유 루트', target: { kind: 'local', root: '//srv/share/repo' }, segs: ['a'], platform: 'win32' },
  { name: '빈 루트', target: { kind: 'local', root: '' }, segs: [], platform: 'linux' },
  { name: 'NUL 루트', target: { kind: 'local', root: '/a\0b' }, segs: [], platform: 'linux' },
  { name: '끝 슬래시 루트는 정규화', target: { kind: 'local', root: '/a/b//' }, segs: [], platform: 'linux' },
  { name: '끝 슬래시 루트에 합치기', target: { kind: 'local', root: '/a/b//' }, segs: ['c'], platform: 'linux' },
  { name: 'Windows 슬래시 루트', target: { kind: 'local', root: 'c:/work/repo' }, segs: ['a'], platform: 'win32' },
  { name: 'Windows 드라이브 루트', target: { kind: 'local', root: 'C:\\' }, segs: ['a'], platform: 'win32' },
  { name: 'Windows 드라이브 없는 절대', target: { kind: 'local', root: '\\work' }, segs: ['a'], platform: 'win32' },
  { name: 'Windows 에서 POSIX 루트(WSL)는 POSIX 규칙', target: { kind: 'wsl', root: '/a/../b', distro: 'U' }, segs: [], platform: 'win32' },
  { name: 'Windows 에서 윈도 루트를 SSH 로', target: { kind: 'ssh', root: 'C:\\x', host: 'h' }, segs: [], platform: 'win32' }
]

// ── checkRealContainment ──────────────────────────────────────────────────────

export interface ContainCase {
  name: string
  root: string
  abs: string
  /** 경로 → 푼 결과. 목록에 없는 경로는 존재하지 않는다 */
  links: Record<string, string>
  mod: 'posix' | 'win32'
}

export const CONTAIN_CASES: ContainCase[] = [
  { name: '저장소 안의 파일', root: '/r', abs: '/r/a.txt', links: { '/r': '/r', '/r/a.txt': '/r/a.txt' }, mod: 'posix' },
  { name: '밖을 가리키는 링크', root: '/r', abs: '/r/link', links: { '/r': '/r', '/r/link': '/etc' }, mod: 'posix' },
  { name: '이름이 비슷한 형제 폴더', root: '/r', abs: '/r/link', links: { '/r': '/r', '/r/link': '/r2/x' }, mod: 'posix' },
  { name: '폴더 링크를 거쳐 밖으로', root: '/r', abs: '/r/d/x', links: { '/r': '/r', '/r/d': '/tmp/out', '/r/d/x': '/tmp/out/x' }, mod: 'posix' },
  { name: '루트 자체가 링크', root: '/link', abs: '/link/a', links: { '/link': '/real', '/link/a': '/real/a' }, mod: 'posix' },
  { name: '없는 파일은 가장 가까운 상위로', root: '/r', abs: '/r/d/gone.txt', links: { '/r': '/r', '/r/d': '/r/d' }, mod: 'posix' },
  { name: '없는 파일의 상위가 밖을 가리킴', root: '/r', abs: '/r/d/gone.txt', links: { '/r': '/r', '/r/d': '/etc' }, mod: 'posix' },
  { name: '루트를 풀 수 없음', root: '/r', abs: '/r/a', links: {}, mod: 'posix' },
  { name: '루트까지 없음', root: '/r', abs: '/r/a/b', links: { '/r': '/r' }, mod: 'posix' },
  { name: '루트와 같은 경로', root: '/r', abs: '/r', links: { '/r': '/r' }, mod: 'posix' },
  { name: '루트가 루트(/)', root: '/', abs: '/a', links: { '/': '/', '/a': '/a' }, mod: 'posix' },
  { name: '끝 슬래시가 있는 푼 루트', root: '/r', abs: '/r/a', links: { '/r': '/r/', '/r/a': '/r/a' }, mod: 'posix' },
  { name: '..로 시작하는 이름은 밖이 아니다', root: '/r', abs: '/r/..x', links: { '/r': '/r', '/r/..x': '/r/..x' }, mod: 'posix' },
  { name: 'Windows 대소문자 무시', root: 'C:\\Repo', abs: 'C:\\Repo\\a', links: { 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\a': 'c:\\repo\\a' }, mod: 'win32' },
  { name: 'Windows 다른 드라이브', root: 'C:\\Repo', abs: 'C:\\Repo\\a', links: { 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\a': 'D:\\x' }, mod: 'win32' },
  { name: 'Windows 형제 폴더', root: 'C:\\Repo', abs: 'C:\\Repo\\a', links: { 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\a': 'C:\\Repo2\\a' }, mod: 'win32' },
  { name: 'Windows 정션이 밖으로', root: 'C:\\Repo', abs: 'C:\\Repo\\j\\x', links: { 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\j': 'C:\\Other', 'C:\\Repo\\j\\x': 'C:\\Other\\x' }, mod: 'win32' },
  { name: 'Windows 없는 파일', root: 'C:\\Repo', abs: 'C:\\Repo\\d\\gone', links: { 'C:\\Repo': 'C:\\Repo', 'C:\\Repo\\d': 'C:\\Repo\\d' }, mod: 'win32' },
  {
    name: 'WSL UNC 안',
    root: '\\\\wsl.localhost\\U\\r',
    abs: '\\\\wsl.localhost\\U\\r\\a',
    links: { '\\\\wsl.localhost\\U\\r': '\\\\wsl.localhost\\U\\r', '\\\\wsl.localhost\\U\\r\\a': '\\\\wsl.localhost\\U\\r\\a' },
    mod: 'win32'
  },
  {
    name: 'WSL UNC 밖(다른 배포판)',
    root: '\\\\wsl.localhost\\U\\r',
    abs: '\\\\wsl.localhost\\U\\r\\a',
    links: { '\\\\wsl.localhost\\U\\r': '\\\\wsl.localhost\\U\\r', '\\\\wsl.localhost\\U\\r\\a': '\\\\wsl.localhost\\V\\r\\a' },
    mod: 'win32'
  },
  {
    name: 'WSL UNC 밖(같은 배포판의 다른 폴더)',
    root: '\\\\wsl.localhost\\U\\home\\u\\r',
    abs: '\\\\wsl.localhost\\U\\home\\u\\r\\evil',
    links: { '\\\\wsl.localhost\\U\\home\\u\\r': '\\\\wsl.localhost\\U\\home\\u\\r', '\\\\wsl.localhost\\U\\home\\u\\r\\evil': '\\\\wsl.localhost\\U\\etc' },
    mod: 'win32'
  }
]

// ── buildEditorLaunch ─────────────────────────────────────────────────────────

export interface EditorCase {
  name: string
  target: Target
  paths: string[]
  editor: EditorKind
  exe: string
  platform: Plat
}

export const EDITOR_CASES: EditorCase[] = [
  { name: 'VS Code 로컬: 경로만', target: local, paths: ['/home/u/repo', '/home/u/repo/-x'], editor: 'vscode', exe: '/usr/bin/code', platform: 'linux' },
  { name: 'VS Code WSL', target: wsl, paths: ['/home/u/repo', '/home/u/repo/--help'], editor: 'vscode', exe: 'Code.exe', platform: 'win32' },
  { name: 'VS Code SSH', target: ssh, paths: ['/srv/repo'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: 'Cursor SSH 사용자@호스트', target: { ...ssh, host: 'me@dev-box.example.com' }, paths: ['/srv/repo'], editor: 'cursor', exe: 'cursor', platform: 'linux' },
  { name: 'JetBrains Windows 로컬', target: winLocal, paths: ['C:\\work\\repo'], editor: 'jetbrains', exe: 'idea64.exe', platform: 'win32' },
  { name: '사용자 지정 WSL 은 UNC', target: wsl, paths: ['/home/u/repo', '/home/u/repo/a b.txt'], editor: 'custom', exe: 'x.exe', platform: 'win32' },
  { name: 'JetBrains SSH 거부', target: ssh, paths: ['/srv/repo'], editor: 'jetbrains', exe: 'x', platform: 'linux' },
  { name: '사용자 지정 WSL 은 Windows 에서만', target: wsl, paths: ['/home/u/repo'], editor: 'custom', exe: 'x', platform: 'linux' },
  { name: '옵션 경로 거부', target: local, paths: ['--remote'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '옵션 경로 거부(둘째 항목)', target: local, paths: ['/r', '-x'], editor: 'custom', exe: 'x', platform: 'linux' },
  { name: 'NUL 경로 거부', target: local, paths: ['/r\0x'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '배포판 이름이 옵션', target: { ...wsl, distro: '--exec' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'win32' },
  { name: '배포판 이름에 공백', target: { ...wsl, distro: 'a b' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'win32' },
  { name: '배포판 이름 100자', target: { ...wsl, distro: 'a'.repeat(100) }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'win32' },
  { name: '배포판 이름 101자', target: { ...wsl, distro: 'a'.repeat(101) }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'win32' },
  { name: '배포판 이름 역슬래시(UNC 주입)', target: { ...wsl, distro: '..\\evil' }, paths: ['/r'], editor: 'custom', exe: 'x', platform: 'win32' },
  { name: '배포판 이름 빈 문자열', target: { ...wsl, distro: '' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'win32' },
  { name: '호스트 옵션 주입', target: { ...ssh, host: '-oProxyCommand=x' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '호스트에 세미콜론', target: { ...ssh, host: 'h;rm' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '호스트 200자', target: { ...ssh, host: 'h'.repeat(200) }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '호스트 201자', target: { ...ssh, host: 'h'.repeat(201) }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '호스트 비 ASCII', target: { ...ssh, host: 'hé' }, paths: ['/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: '중복 경로는 하나로', target: local, paths: ['/r', '/r', '/r/a', '/r'], editor: 'vscode', exe: 'code', platform: 'linux' },
  { name: 'WSL 루트 경로 UNC', target: wsl, paths: ['/'], editor: 'jetbrains', exe: 'x.exe', platform: 'win32' }
]

// ── buildTerminalLaunch ───────────────────────────────────────────────────────

const WT = 'C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\wt.exe'
export const WITH_WT = { systemRoot: 'C:\\Windows', wt: WT }
export const NO_WT = { systemRoot: 'C:\\Windows', wt: null }

export interface TerminalCase {
  name: string
  target: Target
  dir: string
  platform: Plat
  tools: { systemRoot?: string; wt?: string | null; linuxTerminal?: string | null; macTerminal?: string | null }
}

export const TERMINAL_CASES: TerminalCase[] = [
  { name: 'Windows 로컬 wt', target: winLocal, dir: 'C:\\work\\repo', platform: 'win32', tools: WITH_WT },
  { name: 'Windows 로컬 wt 없음은 cmd start + cwd', target: winLocal, dir: 'C:\\work\\a&b', platform: 'win32', tools: NO_WT },
  { name: 'Windows 로컬 도구 없음', target: winLocal, dir: 'C:\\r', platform: 'win32', tools: {} },
  { name: 'Windows 로컬 다른 SystemRoot', target: winLocal, dir: 'C:\\r', platform: 'win32', tools: { systemRoot: 'D:\\WINNT', wt: null } },
  { name: 'Windows 로컬 SystemRoot 끝 슬래시', target: winLocal, dir: 'C:\\r', platform: 'win32', tools: { systemRoot: 'C:\\Windows\\', wt: null } },
  ...['C:\\a;b', 'C:\\a"b', 'C:\\a\nb', 'C:\\a\u007fb'].map((dir): TerminalCase => ({ name: `wt 를 쓰지 않는 경로 ${JSON.stringify(dir)}`, target: winLocal, dir, platform: 'win32', tools: WITH_WT })),
  { name: 'wt 빈 문자열은 없는 것', target: winLocal, dir: 'C:\\r', platform: 'win32', tools: { systemRoot: 'C:\\Windows', wt: '' } },
  { name: 'WSL wt', target: wsl, dir: '/home/u/repo', platform: 'win32', tools: WITH_WT },
  { name: 'WSL cmd start', target: wsl, dir: '/home/u/repo', platform: 'win32', tools: NO_WT },
  ...['/r/a&calc', '/r/%PATH%', '/r/a b', '/r/"x', '/r/a|b', '/r/^x', '/r/a;b', '/r/가', 'r/a', '/r/a\n'].map(
    (dir): TerminalCase => ({ name: `WSL wt 없음 거부 ${JSON.stringify(dir)}`, target: wsl, dir, platform: 'win32', tools: NO_WT })
  ),
  { name: 'WSL wt 가 있어도 따옴표가 든 경로', target: wsl, dir: '/r/a" --exec x', platform: 'win32', tools: WITH_WT },
  { name: 'WSL wt 가 있어도 세미콜론이 든 경로', target: wsl, dir: '/r/a;b', platform: 'win32', tools: WITH_WT },
  { name: 'WSL wt 는 공백 경로를 인자 배열로', target: wsl, dir: '/r/a b', platform: 'win32', tools: WITH_WT },
  { name: 'WSL 허용 문자 전부', target: wsl, dir: '/A-z_0.9/+=,:@-/x', platform: 'win32', tools: NO_WT },
  { name: 'WSL 이상한 배포판', target: { ...wsl, distro: 'a b' }, dir: '/r', platform: 'win32', tools: NO_WT },
  { name: 'SSH 거부', target: ssh, dir: '/srv/repo', platform: 'linux', tools: {} },
  { name: '옵션 경로 거부', target: local, dir: '-x', platform: 'linux', tools: { linuxTerminal: '/usr/bin/x-terminal-emulator' } },
  { name: 'macOS', target: local, dir: '/home/u/repo', platform: 'darwin', tools: { macTerminal: '/System/Applications/Utilities/Terminal.app' } },
  { name: 'macOS Terminal.app 없음', target: local, dir: '/home/u/repo', platform: 'darwin', tools: {} },
  { name: '리눅스', target: local, dir: '/home/u/repo', platform: 'linux', tools: { linuxTerminal: '/usr/bin/x-terminal-emulator' } },
  { name: '리눅스 터미널 없음', target: local, dir: '/home/u/repo', platform: 'linux', tools: {} },
  { name: 'WSL 은 Windows 에서만', target: wsl, dir: '/r', platform: 'linux', tools: {} }
]

// ── hasDotComponent · clsidComponent · buildRevealPlan ────────────────────────

export const DOT_CASES: { name: string; root: string; p: string }[] = [
  { name: '루트 아래 구성 요소만', root: '/a/repo', p: '/a/repo/x/y' },
  { name: '루트 자신', root: '/a/repo', p: '/a/repo' },
  { name: '루트 이름에 점', root: '/a/repo.git', p: '/a/repo.git' },
  { name: '루트 위쪽 점은 보지 않음', root: '/a.b/repo', p: '/a.b/repo/x' },
  { name: '아래 구성 요소 점', root: '/a/repo', p: '/a/repo/x.y/z' },
  { name: '숨김 폴더', root: '/a/repo', p: '/a/repo/.git' },
  { name: '밖으로 나간 경로', root: '/a/repo', p: '/a/other' },
  { name: '루트가 /', root: '/', p: '/x' }
]

export const CLSID_CASES: { name: string; root: string; p: string }[] = [
  { name: '없음', root: 'C:\\work\\repo', p: 'C:\\work\\repo\\src\\lib' },
  { name: '바로 아래', root: 'C:\\work\\repo', p: `C:\\work\\repo\\fs.${GUID}` },
  { name: '더 깊이', root: 'C:\\work\\repo', p: `C:\\work\\repo\\a\\fs.${GUID}\\b\\c` },
  { name: '루트 자신(소문자)', root: `C:\\work\\x.${GUID.toLowerCase()}`, p: `C:\\work\\x.${GUID.toLowerCase()}\\src` },
  { name: 'GUID 뒤에 확장자', root: 'C:\\work\\repo', p: `C:\\work\\repo\\fs.${GUID}.txt` },
  { name: '점 없이 붙은 GUID', root: 'C:\\work\\repo', p: `C:\\work\\repo\\fs${GUID}` },
  { name: '루트와 같음', root: 'C:\\work\\repo', p: 'C:\\work\\repo' },
  { name: '루트 위쪽 이름은 보지 않음', root: 'C:\\a.' + GUID + '\\repo', p: 'C:\\a.' + GUID + '\\repo\\src' },
  { name: '대소문자 다른 루트 표기', root: 'C:\\Work\\Repo', p: `c:\\work\\repo\\FS.${GUID}\\x` },
  { name: '슬래시 구분', root: 'C:\\work\\repo', p: `C:\\work\\repo\\a/fs.${GUID}/b` },
  { name: '35자 GUID', root: 'C:\\work\\repo', p: 'C:\\work\\repo\\fs.{0AFACED1-E828-11D1-9187-B532F1E9575}' },
  { name: '16진수가 아닌 문자', root: 'C:\\work\\repo', p: `C:\\work\\repo\\fs.${GUID.replace('E', 'G')}` },
  { name: '밖으로 나간 경로', root: 'C:\\work\\repo', p: 'D:\\x' }
]

export interface RevealCase {
  name: string
  target: Target
  state: { path: string; isDir: boolean; root: string }
  platform: Plat
}

const UNC = '\\\\wsl.localhost\\U\\r'
const wslU: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'U' }
const rt: Target = { kind: 'local', root: '/r' }
const winRoot = 'C:\\work\\repo'

export const REVEAL_CASES: RevealCase[] = [
  { name: '폴더는 열기', target: rt, state: { path: '/r', isDir: true, root: '/r' }, platform: 'linux' },
  { name: '파일은 보여 주기', target: rt, state: { path: '/r/a.sh', isDir: false, root: '/r' }, platform: 'linux' },
  { name: 'SSH 거부', target: ssh, state: { path: '/srv/repo', isDir: true, root: '/srv/repo' }, platform: 'linux' },
  { name: 'WSL 은 Windows 에서만', target: wslU, state: { path: '/r', isDir: true, root: '/r' }, platform: 'linux' },
  { name: 'WSL UNC 폴더', target: wslU, state: { path: UNC, isDir: true, root: UNC }, platform: 'win32' },
  { name: 'WSL 이상한 배포판', target: { ...wslU, distro: '..\\x' }, state: { path: UNC, isDir: true, root: UNC }, platform: 'win32' },
  ...['X.app', 'App.xcodeproj', 'a/b.service', 'x/.hidden', 'Pad.playground', 'W.wdgt'].map(
    (d): RevealCase => ({ name: `macOS ${d}`, target: local, state: { path: `/Users/u/repo/${d}`, isDir: true, root: '/Users/u/repo' }, platform: 'darwin' })
  ),
  { name: 'macOS 일반 폴더', target: local, state: { path: '/Users/u/repo/src/lib', isDir: true, root: '/Users/u/repo' }, platform: 'darwin' },
  { name: 'macOS 루트가 tool.app', target: local, state: { path: '/Users/u/tool.app/src', isDir: true, root: '/Users/u/tool.app' }, platform: 'darwin' },
  { name: 'macOS 루트 위쪽 점', target: local, state: { path: '/Users/u.name/repo/src', isDir: true, root: '/Users/u.name/repo' }, platform: 'darwin' },
  { name: '리눅스는 이름 규칙 없음', target: rt, state: { path: '/r/X.app', isDir: true, root: '/r' }, platform: 'linux' },
  { name: 'macOS 파일', target: local, state: { path: '/Users/u/repo/a.txt', isDir: false, root: '/Users/u/repo' }, platform: 'darwin' },
  { name: 'Windows CLSID 하위', target: { kind: 'local', root: winRoot }, state: { path: `${winRoot}\\fs.${GUID}\\x`, isDir: true, root: winRoot }, platform: 'win32' },
  { name: 'Windows CLSID 자신', target: { kind: 'local', root: winRoot }, state: { path: `${winRoot}\\fs.${GUID}`, isDir: true, root: winRoot }, platform: 'win32' },
  { name: 'Windows 일반 폴더', target: { kind: 'local', root: winRoot }, state: { path: `${winRoot}\\src`, isDir: true, root: winRoot }, platform: 'win32' },
  { name: 'Windows 파일', target: { kind: 'local', root: winRoot }, state: { path: `${winRoot}\\a.exe`, isDir: false, root: winRoot }, platform: 'win32' },
  { name: '리눅스에서 CLSID 이름은 보통 폴더', target: rt, state: { path: `/r/fs.${GUID}`, isDir: true, root: '/r' }, platform: 'linux' }
]

// ── 링크(code:openLink) ───────────────────────────────────────────────────────

export const LINK_CASES: { name: string; raw: string }[] = [
  { name: '쿼리·해시 보존', raw: 'https://example.com/a?b=1#c' },
  { name: '경로 없는 주소는 / 를 붙인다', raw: 'https://example.com' },
  { name: '대문자 호스트는 소문자', raw: 'HTTPS://Example.COM:443/A' },
  { name: '기본이 아닌 포트', raw: 'https://example.com:8443/x' },
  { name: 'http 거부', raw: 'http://example.com' },
  { name: 'file 거부', raw: 'file:///etc/passwd' },
  { name: 'javascript 거부', raw: 'javascript:alert(1)' },
  { name: '사용자 이름', raw: 'https://user@example.com' },
  { name: '사용자·비밀번호', raw: 'https://user:pw@example.com' },
  { name: '비밀번호만', raw: 'https://:pw@example.com' },
  { name: '빈 사용자 정보는 자격 증명이 아니다', raw: 'https://@example.com/' },
  { name: '주소가 아님', raw: 'not a url' },
  { name: '빈 문자열', raw: '' },
  { name: '상대 주소', raw: '/a/b' },
  { name: '앞뒤 공백은 정리', raw: '  https://example.com/x  ' },
  { name: '4096 단위 경계(통과)', raw: 'https://e.com/' + 'a'.repeat(4096 - 14) },
  { name: '4097 단위(거부)', raw: 'https://e.com/' + 'a'.repeat(4096 - 13) },
  { name: '이모지는 2단위(통과)', raw: 'https://e.com/' + '😀'.repeat(2041) },
  { name: '이모지는 2단위(거부)', raw: 'https://e.com/' + '😀'.repeat(2042) },
  { name: '경로의 한글은 퍼센트 인코딩', raw: 'https://example.com/한글' },
  { name: 'IPv6 호스트', raw: 'https://[::1]:8080/x' },
  { name: '프로토콜 상대 주소', raw: '//example.com/x' },
  { name: 'ftp 거부', raw: 'ftp://example.com/x' }
]

// ── 실행기(ExternalOpener) 시나리오 ───────────────────────────────────────────

export interface Step {
  target: Target
  action: ExternalAction
  rel?: unknown
  ui?: { editor?: EditorKind; editorExePath?: string }
}

/** 가짜 파일 시스템: files·dirs 는 존재하는 실제 경로, links 는 경로 → 푼 결과(통째 대응). 목록에 없는 경로는 없다 */
export interface Scenario {
  name: string
  platform: Plat
  files?: string[]
  dirs?: string[]
  links?: Record<string, string>
  env?: Record<string, string>
  steps: Step[]
}

const LINUX = {
  files: ['/home/u/repo/a.txt', '/home/u/repo/-rf', '/usr/bin/code', '/usr/bin/x-terminal-emulator'],
  dirs: ['/home/u/repo', '/home/u/repo/src', '/etc']
}

const WROOT = 'C:\\work\\repo'
const WUNC = (p: string): string => '\\\\wsl.localhost\\Ubuntu' + p.replace(/\//g, '\\')
const WCODE = 'C:\\L\\Programs\\Microsoft VS Code\\Code.exe'
const WSL_FS = { files: [WCODE, WUNC('/home/u/repo/a.txt')], dirs: [WUNC('/home/u/repo')], env: { LOCALAPPDATA: 'C:\\L' } }
const wslT: Target = { kind: 'wsl', root: '/home/u/repo', distro: 'Ubuntu' }
const sshT: Target = { kind: 'ssh', root: '/srv/repo', host: 'devbox' }
const MAC_ROOT = '/Users/u/repo'
const MAC_TERM = '/System/Applications/Utilities/Terminal.app'
const MAC_FS = {
  dirs: [MAC_TERM, MAC_ROOT, `${MAC_ROOT}/src`, `${MAC_ROOT}/Evil.app`, `${MAC_ROOT}/Evil.app/Contents`, `${MAC_ROOT}/P.xcodeproj`, `${MAC_ROOT}/plain`],
  files: [`${MAC_ROOT}/a.txt`],
  // 구성 요소별 링크(TS externalReveal 의 가짜 fs 는 경로의 각 구성 요소를 풀었다): innocent/Contents 는 링크를 거쳐 존재하는 폴더다
  links: {
    [`${MAC_ROOT}/innocent`]: `${MAC_ROOT}/Evil.app`,
    [`${MAC_ROOT}/innocent/Contents`]: `${MAC_ROOT}/Evil.app/Contents`,
    [`${MAC_ROOT}/plainlink`]: `${MAC_ROOT}/plain`
  }
}
const macT: Target = { kind: 'local', root: MAC_ROOT }
const winT: Target = { kind: 'local', root: WROOT }
const BAD = `${WROOT}\\fs.${GUID}`
const CLSID_DIRS = [WROOT, `${WROOT}\\src`, BAD, `${BAD}\\sub`]

export const SCENARIOS: Scenario[] = [
  {
    name: '리눅스 로컬: 에디터는 폴더와 파일을 절대 경로로',
    platform: 'linux',
    ...LINUX,
    env: { PATH: '/usr/bin' },
    steps: [
      { target: local, action: 'editor', rel: '-rf', ui: {} },
      { target: local, action: 'editor', ui: { editor: 'vscode' } },
      { target: local, action: 'editor', rel: 'src/gone.txt', ui: {} },
      { target: local, action: 'editor', rel: 'src', ui: { editor: 'cursor' } }
    ]
  },
  {
    name: '리눅스 로컬: 파일 관리자',
    platform: 'linux',
    ...LINUX,
    steps: [
      { target: local, action: 'fileManager' },
      { target: local, action: 'fileManager', rel: 'a.txt' },
      { target: local, action: 'fileManager', rel: 'src/gone/x.txt' },
      { target: local, action: 'fileManager', rel: 'src' }
    ]
  },
  {
    name: '리눅스 로컬: 존재하는 파일이 상위인 없는 경로',
    platform: 'linux',
    ...LINUX,
    files: [...LINUX.files, '/home/u/repo/evil.exe'],
    steps: [
      { target: local, action: 'fileManager', rel: 'evil.exe/x' },
      { target: local, action: 'fileManager', rel: 'evil.exe' },
      { target: local, action: 'terminal', rel: 'evil.exe/x' },
      { target: local, action: 'terminal', rel: 'evil.exe' }
    ]
  },
  {
    name: '리눅스 로컬: 터미널',
    platform: 'linux',
    ...LINUX,
    steps: [
      { target: local, action: 'terminal', rel: 'a.txt' },
      { target: local, action: 'terminal', rel: 'src' },
      { target: local, action: 'terminal' }
    ]
  },
  {
    name: '리눅스 로컬: 상대 경로 거부',
    platform: 'linux',
    ...LINUX,
    steps: ['../x', 'src/../../etc', '/etc/passwd', 'a\0b', 'C:/x', 42].map((rel): Step => ({ target: local, action: 'terminal', rel }))
  },
  {
    name: '리눅스 로컬: 밖을 가리키는 링크',
    platform: 'linux',
    ...LINUX,
    links: { '/home/u/repo/evil': '/etc' },
    steps: [
      { target: local, action: 'fileManager', rel: 'evil' },
      { target: local, action: 'terminal', rel: 'evil' },
      { target: local, action: 'editor', rel: 'evil', ui: { editor: 'custom', editorExePath: '/opt/ide' } }
    ]
  },
  {
    name: '리눅스 로컬: 저장소 루트를 찾을 수 없음',
    platform: 'linux',
    steps: [{ target: local, action: 'fileManager' }]
  },
  {
    name: '리눅스 로컬: 에디터 실행 파일',
    platform: 'linux',
    files: ['/opt/idea/bin/idea.sh', '/home/u/repo/a.txt', 'bin/code', './code', '/usr/bin/code'],
    dirs: ['/home/u/repo'],
    env: { PATH: '.:bin:/usr/bin' },
    steps: [
      { target: local, action: 'editor', rel: 'a.txt', ui: {} },
      { target: local, action: 'editor', ui: { editor: 'jetbrains' } },
      { target: local, action: 'editor', ui: { editor: 'custom', editorExePath: '/opt/idea/bin/idea.sh' } },
      { target: local, action: 'editor', ui: { editor: 'custom', editorExePath: '/opt/gone' } },
      { target: local, action: 'editor', ui: { editor: 'custom', editorExePath: 'idea.sh' } }
    ]
  },
  {
    name: '리눅스 로컬: VS Code·Cursor 를 찾지 못함',
    platform: 'linux',
    dirs: ['/home/u/repo'],
    env: { PATH: '/usr/bin' },
    steps: [
      { target: local, action: 'editor', ui: {} },
      { target: local, action: 'editor', ui: { editor: 'cursor' } }
    ]
  },
  {
    name: 'macOS: 번들 디렉터리',
    platform: 'darwin',
    dirs: ['/System/Applications/Utilities/Terminal.app', '/home/u/repo', '/home/u/repo/X.app', '/home/u/repo/X.app/Contents'],
    steps: [
      { target: local, action: 'fileManager', rel: 'X.app' },
      { target: local, action: 'fileManager', rel: 'X.app/gone' },
      { target: local, action: 'terminal', rel: 'X.app/Contents' },
      { target: local, action: 'terminal', rel: 'X.app' }
    ]
  },
  {
    name: 'macOS: realpath 기준 번들 판정',
    platform: 'darwin',
    ...MAC_FS,
    steps: [
      { target: macT, action: 'fileManager', rel: 'innocent' },
      { target: macT, action: 'terminal', rel: 'innocent' },
      { target: macT, action: 'terminal', rel: 'innocent/Contents' },
      { target: macT, action: 'fileManager', rel: 'P.xcodeproj' },
      { target: macT, action: 'fileManager', rel: 'plain' },
      { target: macT, action: 'fileManager', rel: 'plainlink' },
      { target: macT, action: 'fileManager', rel: 'src/gone/x' },
      { target: macT, action: 'fileManager', rel: 'Evil.app/gone' },
      { target: macT, action: 'fileManager', rel: 'a.txt' },
      { target: macT, action: 'terminal', rel: 'src' }
    ]
  },
  {
    name: 'macOS: 저장소 루트가 tool.app',
    platform: 'darwin',
    dirs: [MAC_TERM, '/Users/u/tool.app', '/Users/u/tool.app/src'],
    steps: [
      { target: { kind: 'local', root: '/Users/u/tool.app' }, action: 'fileManager', rel: 'src' },
      { target: { kind: 'local', root: '/Users/u/tool.app' }, action: 'terminal', rel: 'src' }
    ]
  },
  {
    name: 'macOS: Terminal.app 이 없으면 거부',
    platform: 'darwin',
    dirs: ['/Users/u/repo'],
    steps: [{ target: { kind: 'local', root: '/Users/u/repo' }, action: 'terminal' }]
  },
  {
    name: 'macOS: /Applications/Utilities 의 Terminal.app',
    platform: 'darwin',
    dirs: ['/Applications/Utilities/Terminal.app', '/Users/u/repo'],
    steps: [{ target: { kind: 'local', root: '/Users/u/repo' }, action: 'terminal' }]
  },
  {
    name: '정션·링크도 realpath 가 폴더일 때만 openPath',
    platform: 'linux',
    dirs: ['/r', '/r/d'],
    files: ['/r/evil.exe'],
    links: { '/r/junction': '/r/d', '/r/lnk': '/r/evil.exe' },
    steps: [
      { target: { kind: 'local', root: '/r' }, action: 'fileManager', rel: 'junction' },
      { target: { kind: 'local', root: '/r' }, action: 'fileManager', rel: 'lnk' }
    ]
  },
  {
    name: 'Windows 로컬: VS Code 는 Code.exe',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' },
    files: ['C:\\Users\\u\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe', 'C:\\work\\repo\\-x.txt'],
    dirs: [WROOT],
    steps: [
      { target: winT, action: 'editor', rel: '-x.txt', ui: {} },
      { target: winT, action: 'editor', ui: { editor: 'vscode' } }
    ]
  },
  {
    name: 'Windows 로컬: 에디터 설치 위치 탐색',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\L', ProgramFiles: 'C:\\PF', 'ProgramFiles(x86)': 'C:\\PF86' },
    files: ['C:\\PF86\\Microsoft VS Code\\Code.exe', 'C:\\L\\Programs\\cursor\\Cursor.exe'],
    dirs: [WROOT],
    steps: [
      { target: winT, action: 'editor', ui: { editor: 'vscode' } },
      { target: winT, action: 'editor', ui: { editor: 'cursor' } }
    ]
  },
  {
    name: 'Windows 로컬: Program Files 의 Cursor',
    platform: 'win32',
    env: { ProgramFiles: 'C:\\PF' },
    files: ['C:\\PF\\cursor\\Cursor.exe'],
    dirs: [WROOT],
    steps: [{ target: winT, action: 'editor', ui: { editor: 'cursor' } }]
  },
  {
    name: 'Windows 로컬: 사용자 지정 에디터는 .exe/.com 만',
    platform: 'win32',
    files: ['C:\\a\\idea64.exe', 'C:\\a\\run.cmd', 'C:\\a\\run.bat', 'C:\\a\\x.COM', '\\\\srv\\s\\a.exe'],
    dirs: [WROOT],
    steps: ['C:\\a\\idea64.exe', 'C:\\a\\run.cmd', 'C:\\a\\run.bat', 'C:\\a\\x.COM', '\\\\srv\\s\\a.exe', 'idea64.exe', 'C:\\a\\none.exe'].map(
      (p): Step => ({ target: winT, action: 'editor', ui: { editor: 'custom', editorExePath: p } })
    )
  },
  {
    name: 'Windows 로컬: 터미널 wt 가 있을 때',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\L' },
    files: ['C:\\L\\Microsoft\\WindowsApps\\wt.exe'],
    dirs: [WROOT],
    steps: [{ target: winT, action: 'terminal' }]
  },
  {
    name: 'Windows 로컬: 터미널 wt 가 없을 때',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' },
    dirs: [WROOT],
    steps: [{ target: winT, action: 'terminal' }]
  },
  {
    name: 'Windows 로컬: 비정상 LOCALAPPDATA·SystemRoot 는 쓰지 않는다',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\a;b', SystemRoot: 'C:\\a"b' },
    files: ['C:\\a;b\\Microsoft\\WindowsApps\\wt.exe'],
    dirs: [WROOT],
    steps: [{ target: winT, action: 'terminal' }]
  },
  {
    name: 'Windows 로컬: 정상 SystemRoot',
    platform: 'win32',
    env: { SystemRoot: 'D:\\WINNT' },
    dirs: [WROOT],
    steps: [{ target: winT, action: 'terminal' }]
  },
  {
    name: 'Windows 로컬: 역슬래시·드라이브가 든 상대 경로는 거부',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' },
    dirs: [WROOT],
    steps: [
      { target: winT, action: 'terminal', rel: '..\\x' },
      { target: winT, action: 'terminal', rel: 'C:/x' },
      { target: winT, action: 'terminal', rel: 'a:b' }
    ]
  },
  {
    name: 'Windows 로컬: 네트워크 공유 루트는 거부',
    platform: 'win32',
    dirs: ['\\\\srv\\share\\repo'],
    steps: [{ target: { kind: 'local', root: '\\\\srv\\share\\repo' }, action: 'fileManager' }]
  },
  {
    name: 'Windows: name.{CLSID} 폴더',
    platform: 'win32',
    dirs: CLSID_DIRS,
    steps: [
      { target: winT, action: 'fileManager', rel: 'src' },
      { target: winT, action: 'fileManager', rel: `fs.${GUID}/sub` },
      { target: winT, action: 'terminal', rel: `fs.${GUID}/sub` },
      { target: winT, action: 'terminal', rel: `fs.${GUID}` }
    ]
  },
  {
    name: 'Windows: 저장소 루트 자체가 name.{CLSID}',
    platform: 'win32',
    dirs: [`C:\\work\\x.${GUID}`, `C:\\work\\x.${GUID}\\src`],
    steps: [
      { target: { kind: 'local', root: `C:\\work\\x.${GUID}` }, action: 'terminal', rel: 'src' },
      { target: { kind: 'local', root: `C:\\work\\x.${GUID}` }, action: 'fileManager', rel: 'src' }
    ]
  },
  {
    name: 'WSL: VS Code·파일 관리자·터미널',
    platform: 'win32',
    ...WSL_FS,
    steps: [
      { target: wslT, action: 'editor', rel: 'a.txt', ui: {} },
      { target: wslT, action: 'fileManager', rel: 'a.txt' },
      { target: wslT, action: 'fileManager' },
      { target: wslT, action: 'terminal', rel: 'a.txt' },
      { target: wslT, action: 'terminal' }
    ]
  },
  {
    name: 'WSL: 이상한 배포판은 UNC 를 만들기 전에 거부',
    platform: 'win32',
    ...WSL_FS,
    steps: ['..\\evil', 'a b', '--exec', ''].flatMap((distro): Step[] =>
      (['fileManager', 'editor', 'terminal'] as const).map((action): Step => ({ target: { ...wslT, distro }, action, ui: {} }))
    )
  },
  {
    name: 'WSL: 링크가 저장소 밖을 가리키면 거부',
    platform: 'win32',
    ...WSL_FS,
    dirs: [...WSL_FS.dirs, WUNC('/etc')],
    links: { [WUNC('/home/u/repo/evil')]: WUNC('/etc') },
    steps: [{ target: wslT, action: 'editor', rel: 'evil', ui: {} }]
  },
  {
    name: 'WSL: 배포판이 꺼져 있으면 어휘 검사만으로 진행',
    platform: 'win32',
    env: { LOCALAPPDATA: 'C:\\L' },
    files: [WCODE],
    steps: [
      { target: wslT, action: 'editor', rel: 'a.txt', ui: {} },
      { target: wslT, action: 'editor', rel: '../x', ui: {} },
      { target: wslT, action: 'fileManager', rel: 'a.txt' },
      { target: wslT, action: 'terminal', rel: 'a.txt' }
    ]
  },
  {
    name: 'WSL: 사용자 지정 에디터는 UNC 경로',
    platform: 'win32',
    files: ['C:\\ide\\idea64.exe', WUNC('/home/u/repo/a.txt')],
    dirs: [WUNC('/home/u/repo')],
    steps: [{ target: wslT, action: 'editor', rel: 'a.txt', ui: { editor: 'jetbrains', editorExePath: 'C:\\ide\\idea64.exe' } }]
  },
  {
    name: 'WSL: 리눅스에서는 열 수 없다',
    platform: 'linux',
    dirs: ['/home/u/repo'],
    steps: [
      { target: wslT, action: 'fileManager' },
      { target: wslT, action: 'editor', ui: {} },
      { target: wslT, action: 'terminal' }
    ]
  },
  {
    name: 'SSH: VS Code 만. 파일 관리자·터미널·다른 에디터는 거부',
    platform: 'linux',
    files: ['/usr/bin/code'],
    env: { PATH: '/usr/bin' },
    steps: [
      { target: sshT, action: 'editor', rel: 'a/b.txt', ui: {} },
      { target: sshT, action: 'fileManager' },
      { target: sshT, action: 'terminal' },
      { target: sshT, action: 'editor', ui: { editor: 'custom', editorExePath: '/opt/ide' } },
      { target: sshT, action: 'editor', rel: '../x', ui: {} },
      { target: sshT, action: 'editor', ui: { editor: 'cursor' } }
    ]
  },
  {
    name: 'SSH: 이상한 호스트',
    platform: 'linux',
    files: ['/usr/bin/code'],
    env: { PATH: '/usr/bin' },
    steps: ['-oProxyCommand=x', 'h;rm', 'a b', ''].map((host): Step => ({ target: { ...sshT, host }, action: 'editor', ui: {} }))
  },
  {
    name: '루트가 절대 경로가 아니다',
    platform: 'linux',
    dirs: ['repo'],
    steps: [
      { target: { kind: 'local', root: 'repo' }, action: 'fileManager' },
      { target: { kind: 'ssh', root: '~/repo', host: 'h' }, action: 'editor', ui: {} }
    ]
  }
]

// ── 에디터 실행 파일·터미널 도구 ──────────────────────────────────────────────

export interface ExeCase {
  name: string
  platform: Plat
  files?: string[]
  env?: Record<string, string>
  dirs?: string[]
  /** 모든 lstat 이 파일로 성공한다(앱 실행 별칭 wt.exe 는 stat 이 실패하고 lstat 만 된다) */
  lstatAlways?: boolean
}

export const FIND_CASES: ExeCase[] = [
  { name: 'Windows 알려진 설치 위치', platform: 'win32', env: { LOCALAPPDATA: 'C:\\L', ProgramFiles: 'C:\\PF' }, files: ['C:\\PF\\Microsoft VS Code\\Code.exe', 'C:\\L\\Programs\\cursor\\Cursor.exe'] },
  { name: 'Windows 없음', platform: 'win32', env: { LOCALAPPDATA: 'C:\\L', ProgramFiles: 'C:\\PF' } },
  { name: 'Windows 환경변수 없음', platform: 'win32', files: ['C:\\PF\\Microsoft VS Code\\Code.exe'] },
  { name: 'Windows LOCALAPPDATA 우선', platform: 'win32', env: { LOCALAPPDATA: 'C:\\L', ProgramFiles: 'C:\\PF' }, files: ['C:\\PF\\Microsoft VS Code\\Code.exe', 'C:\\L\\Programs\\Microsoft VS Code\\Code.exe'] },
  { name: '상대 PATH 항목은 쓰지 않는다', platform: 'linux', env: { PATH: '.:bin:/usr/bin' }, files: ['bin/code', './code', '/usr/bin/code'] },
  { name: 'PATH 에 없으면 고정 위치', platform: 'linux', env: { PATH: '/opt/x' }, files: ['/usr/local/bin/code', '/snap/bin/cursor'] },
  { name: 'PATH 가 없다', platform: 'linux', files: ['/usr/bin/cursor'] },
  { name: '리눅스 없음', platform: 'linux', env: { PATH: '/usr/bin' } },
  { name: 'macOS', platform: 'darwin', files: ['/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code', '/Applications/Cursor.app/Contents/Resources/app/bin/cursor'] },
  { name: 'macOS 없음', platform: 'darwin' }
]

export const CHOSEN_CASES: { name: string; p: string; platform: Plat; files?: string[] }[] = [
  { name: '리눅스 절대 경로', p: '/opt/x', platform: 'linux' },
  { name: '상대 경로', p: 'x', platform: 'linux' },
  { name: '.cmd', p: 'C:\\a\\run.cmd', platform: 'win32' },
  { name: '.bat', p: 'C:\\a\\run.bat', platform: 'win32' },
  { name: 'UNC', p: '\\\\srv\\s\\a.exe', platform: 'win32' },
  { name: '슬래시 UNC', p: '//srv/s/a.exe', platform: 'win32' },
  { name: '.exe', p: 'C:\\a\\idea64.exe', platform: 'win32' },
  { name: '.EXE 대문자', p: 'C:\\a\\Idea64.EXE', platform: 'win32' },
  { name: '.com', p: 'C:\\a\\x.com', platform: 'win32' },
  { name: '.exe 뒤 공백', p: 'C:\\a\\x.exe ', platform: 'win32' },
  { name: '드라이브 없는 절대', p: '\\a\\x.exe', platform: 'win32' },
  { name: '드라이브 상대', p: 'C:a\\x.exe', platform: 'win32' },
  { name: 'NUL', p: '/a\0b', platform: 'linux' },
  { name: '빈 문자열', p: '', platform: 'linux' },
  { name: '4096 단위(통과)', p: '/' + 'a'.repeat(4095), platform: 'linux' },
  { name: '4097 단위(거부)', p: '/' + 'a'.repeat(4096), platform: 'linux' },
  { name: '존재 확인 통과', p: '/opt/x', platform: 'linux', files: ['/opt/x'] },
  { name: '존재 확인 실패', p: '/opt/x', platform: 'linux', files: [] },
  { name: '존재 확인(폴더는 실패)', p: '/opt', platform: 'linux', files: ['/opt/x'] }
]

export const TOOLS_CASES: ExeCase[] = [
  { name: 'SystemRoot 정상', platform: 'win32', env: { SystemRoot: 'D:\\WINNT' } },
  ...['', 'Windows', '\\\\evil\\share', 'C:\\a;b', 'C:\\a"b', '..\\x', '/etc', 'C:\\a?b', 'C:\\a*b', 'C:\\a<b', 'C:\\a|b', 'C:/x'].map(
    (bad): ExeCase => ({ name: `SystemRoot 이상 ${JSON.stringify(bad)}`, platform: 'win32', env: { SystemRoot: bad } })
  ),
  { name: 'SystemRoot 없음', platform: 'win32', env: {} },
  { name: 'SystemRoot 에 & 는 통과(TS 와 같다)', platform: 'win32', env: { SystemRoot: 'C:\\Win&dows' } },
  { name: 'wt 없음', platform: 'win32', env: { LOCALAPPDATA: 'C:\\Users\\u\\AppData\\Local' } },
  { name: 'wt 있음(lstat)', platform: 'win32', env: { LOCALAPPDATA: 'C:\\L' }, lstatAlways: true },
  ...['relative', '\\\\evil\\share', 'C:\\a;b'].map((bad): ExeCase => ({ name: `LOCALAPPDATA 이상 ${bad}`, platform: 'win32', env: { LOCALAPPDATA: bad }, lstatAlways: true })),
  { name: 'LOCALAPPDATA 없음', platform: 'win32', lstatAlways: true },
  { name: 'macOS 시스템 위치 우선', platform: 'darwin', dirs: ['/System/Applications/Utilities/Terminal.app', '/Applications/Utilities/Terminal.app'] },
  { name: 'macOS /Applications/Utilities', platform: 'darwin', dirs: ['/Applications/Utilities/Terminal.app'] },
  { name: 'macOS 없음', platform: 'darwin', dirs: [] },
  { name: 'macOS 파일은 쓰지 않는다', platform: 'darwin', files: ['/System/Applications/Utilities/Terminal.app'] },
  { name: '리눅스 PATH 의 절대 항목', platform: 'linux', env: { PATH: '.:rel:/opt/bin' }, files: ['/opt/bin/x-terminal-emulator', 'rel/x-terminal-emulator'] },
  { name: '리눅스 상대 PATH 만', platform: 'linux', env: { PATH: '.:rel' }, files: [] },
  { name: '리눅스 /usr/bin 고정', platform: 'linux', env: { PATH: '/opt/x' }, files: ['/usr/bin/x-terminal-emulator'] }
]
