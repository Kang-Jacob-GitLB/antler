// SSH 순수 로직(설정 파서·known_hosts·원격 명령 인용)의 입력 표. tests/goldenSsh.test.ts 가 TS 구현(src/main/executors/ssh)으로 골든(tests/golden/ssh/*.json)을 만들고,
// Rust 이식(src-tauri/src/git/exec/ssh, 검사: src-tauri/tests/ssh_golden.rs)이 같은 입력에서 같은 결과를 내는지 본다.
// 설정 파서의 파일·홈·환경변수·로컬 사용자는 입력에 포함한다(임시 경로에 기대지 않는다) — 가상 파일 시스템의 뿌리는 모두 `C:\golden` 이다.
// 이름이 기존 TS 테스트(tests/ssh*.test.ts)의 `it` 제목과 같은 사례는 그 `it` 의 단언을 그대로 옮긴 것이다(대응표 tests/golden/ssh/test-map.json).

import { createHmac } from 'node:crypto'

export const HOME = 'C:\\golden\\home'
export const LOCAL_USER = 'me'

/** 질의 하나. `rust` 는 Rust 가 TS 와 **다르게** 내는 필드(알려진 차이 — 비 ASCII 호스트·범위 밖 포트 관문, docs/tauri.md "TS 와 다른 점")다 */
export type Query = string | { spec: string; rust: Record<string, unknown> }

/** Rust `resolve_host` 가 덧붙이는 `unsupported` 문구(config.rs 의 상수와 같다) */
const NON_ASCII = { unsupported: '비 ASCII 호스트 이름(punycode 로 적으세요)' }
const BAD_PORT = { unsupported: '포트가 1~65535 범위 밖' }

export interface ConfigCase {
  /** 이식 대응표에 쓰이는 이름(해당하는 TS `it` 제목과 같다) */
  name: string
  text: string
  /** 가상 파일 시스템(Include 가 읽는다). 키는 절대 경로 */
  files?: Record<string, string>
  env?: Record<string, string>
  queries: Query[]
}

const F = (rel: string): string => `${HOME}\\.ssh\\${rel}`

export const CONFIG_CASES: ConfigCase[] = [
  {
    name: 'Host 복수 패턴·와일드카드·부정',
    text: '\nHost web-* !web-old\n  User deploy\nHost a b\n  Port 2200\n',
    queries: ['web-1', 'web-old', 'b', 'a', 'c', 'WEB-2']
  },
  {
    name: '? 와일드카드와 대소문자 무시',
    text: 'HOST node?\n  USER=u1\n',
    queries: ['node7', 'node77', 'NODE7', { spec: 'node😀', rust: NON_ASCII }, 'node\u2028', 'nod']
  },
  {
    name: '먼저 얻은 값이 이긴다(블록 순서 기준)',
    text: '\nHost dev\n  User first\n  HostName dev.example.com\nHost *\n  User second\n  Port 2222\n  HostName ignored\n',
    queries: ['dev', 'other']
  },
  { name: '전역(Host 이전) 옵션도 적용된다', text: 'User g\nHost x\n  Port 1\n', queries: ['x', 'y'] },
  {
    name: 'IdentityFile 은 일치하는 모든 블록에서 누적되고 ~ 와 토큰이 확장된다',
    text: '\nHost srv\n  HostName 10.0.0.5\n  User bob\n  IdentityFile ~/.ssh/a\n  IdentityFile "%d/keys/%h-%r-%u-%p"\nHost *\n  IdentityFile ~/.ssh/z\n  IdentitiesOnly yes\n',
    queries: ['srv', 'srv:2200', 'zed@other']
  },
  { name: 'HostName 의 %h 토큰', text: 'Host app\n  HostName %h.internal.example.com\n', queries: ['app'] },
  {
    name: 'ProxyJump 체인과 user@host:port 형태, none',
    text: '\nHost target\n  ProxyJump j1, ops@j2:2201,[::1]:22\nHost direct\n  ProxyJump none\nHost *\n  ProxyJump ignored\n',
    queries: ['target', 'direct', 'other']
  },
  {
    name: 'IdentityAgent none / SSH_AUTH_SOCK / 경로',
    text: '\nHost a\n  IdentityAgent none\nHost b\n  IdentityAgent SSH_AUTH_SOCK\nHost c\n  IdentityAgent ~/.agent.sock\n',
    queries: ['a', 'b', 'c']
  },
  {
    name: '따옴표 값·주석·행 끝 주석',
    text: '\n# 주석\nHost "my host"   # 끝 주석\n  IdentityFile "C:\\Users\\me\\my key"\n  Port 2022 # 포트\n',
    queries: ['my host']
  },
  {
    name: 'Match 블록은 적용하지 않고 Match all 만 전역으로 취급한다',
    text: '\nMatch host foo\n  User matched\nHost foo\n  Port 1\nMatch all\n  User all\n',
    queries: ['foo']
  },
  {
    name: 'user@host:port 직접 입력은 설정의 user/port 보다 우선하되 host 설정은 조회한다',
    text: 'Host box\n  HostName 1.2.3.4\n  User cfg\n  Port 9\n  IdentityFile ~/k\n',
    queries: ['root@box:2200', 'ann@raw.example.com', 'box', 'ssh://root@box/', '[box]:77']
  },
  {
    name: 'Include: ~/.ssh 기준 상대 경로와 글롭, 포함 파일의 Host 는 거기서 끝난다',
    text: 'Include conf.d/*.conf extra\nHost after\n  Port 4444\nHost *\n  User tail\n',
    files: {
      [F('conf.d\\10-a.conf')]: 'Host inc-a\n  Port 1111\nHost inc-b\n  Port 2222\n',
      [F('extra')]: 'Host inc-c\n  Port 3333\n'
    },
    queries: ['inc-a', 'inc-b', 'inc-c', 'after']
  },
  {
    name: 'Include: 없는 Include 는 조용히 무시',
    text: 'Include nope/*\nInclude missing.conf\nHost x\n  Port 5\n',
    queries: ['x']
  },
  {
    name: 'listHosts: 와일드카드·부정 패턴 제외, 별칭 순서·중복 제거',
    text: '\nHost * !x\n  User all\nHost alpha beta\n  HostName 10.1.1.1\n  Port 2200\n  ProxyJump bastion\nHost alpha\n  User dup\nHost bastion\n  HostName b.example.com\n',
    queries: ['alpha', 'beta', 'bastion']
  },
  {
    name: '지정한 known_hosts 파일들과 별칭을 돌려준다',
    text: 'Host box\n  HostName 10.0.0.5\n  UserKnownHostsFile ~/.ssh/kh1 ~/.ssh/kh2\n  HostKeyAlias box-key\nHost x\n  UserKnownHostsFile none\nHost y\n  UserKnownHostsFile NONE ~/kh\n',
    queries: ['box', 'x', 'y']
  },
  {
    name: 'ProxyCommand 가 걸린 호스트는 unsupported',
    text: 'Host inner\n  ProxyCommand ssh -W %h:%p bastion\nHost none\n  ProxyCommand none\nHost NONE2\n  ProxyCommand NONE\nHost empty\n  ProxyCommand ""\nHost *\n  ProxyCommand late\n',
    queries: ['inner', 'none', 'NONE2', 'empty', 'other']
  },
  {
    name: '이 호스트에 걸릴 수 있는 Match 블록만 unsupported',
    text: 'Match host *.corp exec "true"\n  IdentitiesOnly yes\nMatch user root\n  User admin\n',
    queries: ['box.home', 'db.corp', 'DB.CORP']
  },
  {
    name: '다른 호스트만 겨냥한 Match·옵션 없는 Match 는 영향 없음',
    text: 'Match host other\n  User x\nMatch exec "true"\n',
    queries: ['mine', 'other']
  },
  // ── 추가: TS 테스트에 없는 경계 ─────────────────────────────────────────────
  {
    name: '추가: Match 조건 모양(부정·originalhost·홀수 인자·HostName 대조)',
    text: [
      'Host real',
      '  HostName real.corp',
      'Match host !real.corp',
      '  User a',
      'Match !host nothing',
      '  User b',
      'Match originalhost real,other',
      '  User c',
      'Match host x,!real.corp',
      '  User d',
      'Match canonical',
      '  User e',
      'Match host real.corp exec',
      '  User f',
      'Match HOST REAL.CORP',
      '  User g',
      ''
    ].join('\n'),
    queries: ['real', 'other', 'zzz']
  },
  {
    name: '추가: 경로 정규화(win32 규칙)',
    text: [
      'Host p',
      '  IdentityFile ./a/../b',
      '  IdentityFile C:foo',
      '  IdentityFile //server/share/x',
      '  IdentityFile a:b/c',
      '  IdentityFile x:',
      '  IdentityFile conx',
      '  IdentityFile CON:file',
      '  IdentityFile ~\\k',
      '  IdentityFile ~user/k',
      '  IdentityFile ~',
      '  IdentityFile none',
      '  IdentityFile NONE',
      '  IdentityFile ""',
      '  IdentityFile dir/',
      '  IdentityFile ..\\..\\up',
      '  IdentityFile %%d/%x/%n/%',
      '  IdentityFile C:\\a\\..\\..\\..\\b',
      '  IdentityFile /abs/path//with///slashes',
      '  IdentityFile \\\\?\\C:\\x',
      '  IdentityFile ~/../../up',
      '  UserKnownHostsFile ~/.ssh/kh%n C:/x/./y',
      ''
    ].join('\n'),
    queries: ['p']
  },
  {
    name: '추가: 줄 모양(BOM·CRLF·탭·= 구분·알 수 없는 키·닫히지 않은 따옴표)',
    text: '\uFEFFHost a\r\n\tPort=2201\r\nHost\u00a0b\r\n  Port   =   2202\r\nHost c\n  Hostname=c.example\n  Unknown thing\n  User "no close\nHost-x d\n  Port 1\nHost e\n  Port a\rb\n  User ok\n한글 f\n#Host g\n',
    queries: ['a', 'b', 'c', 'e', 'f']
  },
  {
    name: '추가: 포트 값(범위 밖·숫자 아님·앞의 0)',
    text: 'Host p0\n  Port 0\nHost p1\n  Port 65536\nHost p2\n  Port abc\nHost p3\n  Port 22x\nHost p4\n  Port 0022\nHost p5\n  Port 65535\nHost p6\n  Port ""\n',
    queries: ['p0', 'p1', 'p2', 'p3', 'p4', 'p5', 'p6', { spec: 'p0:70000', rust: BAD_PORT }, { spec: 'p4:0', rust: BAD_PORT }]
  },
  {
    name: '추가: ProxyJump 모양(대소문자·빈 항목·여러 인자·IPv6)',
    text: 'Host a\n  ProxyJump NONE\nHost b\n  ProxyJump j1,,j2 ,\nHost c\n  ProxyJump j1 j2\nHost d\n  ProxyJump ssh://u@j:2201/,[::1]\nHost e\n  ProxyJump ""\nHost f\n  ProxyJump ,\n',
    queries: ['a', 'b', 'c', 'd', 'e', 'f']
  },
  {
    name: '추가: 토큰·HostName·User 가장자리',
    text: 'Host t\n  HostName %h-%n-%%-%q-%\n  User ""\n  IdentityFile %d|%u|%h|%r|%p|%n\nHost u\n  HostName ""\n  IdentitiesOnly YES\nHost v\n  IdentitiesOnly yes please\n',
    queries: ['t', 'u', 'v', '@u', 'x@u', 'ssh://SSH://u']
  },
  {
    name: '추가: IdentityAgent 환경변수·토큰·경로',
    text: 'Host a\n  IdentityAgent $AGENT_DIR/agent.sock\nHost b\n  IdentityAgent "${AGENT_DIR}/x-%u-%h"\nHost c\n  IdentityAgent $UNSET_VAR/s\nHost d\n  IdentityAgent ${AGENT_DIR\nHost e\n  IdentityAgent $\nHost f\n  IdentityAgent ssh_auth_sock\nHost g\n  IdentityAgent ""\nHost h\n  IdentityAgent ~\\pipe\\x\n',
    env: { AGENT_DIR: '/run/user/1000' },
    queries: ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']
  },
  {
    name: '추가: 비 ASCII 와 줄 끝 문자 패턴',
    text: 'Host Éxample ünï*\n  User u\nHost a*b\n  Port 9\nHost ?\n  Port 8\n',
    queries: [
      { spec: 'Éxample', rust: NON_ASCII },
      { spec: 'éxample', rust: { user: LOCAL_USER, ...NON_ASCII } },
      { spec: 'ÜNÏcode', rust: { user: LOCAL_USER, ...NON_ASCII } },
      { spec: 'ünïcode', rust: NON_ASCII },
      { spec: 'a\u2028b', rust: NON_ASCII },
      'axb',
      { spec: '😀', rust: NON_ASCII },
      'x'
    ]
  },
  {
    name: '추가: RevokedHostKeys·범위 밖 ProxyJump 포트(TS 는 무시·통과 — Rust 는 unsupported)',
    text: 'Host r\n  RevokedHostKeys C:\\revoked\nHost j\n  ProxyJump ok,b@bad:0\nHost k\n  ProxyJump j1:65535\n',
    queries: [{ spec: 'r', rust: { unsupported: 'RevokedHostKeys' } }, { spec: 'j', rust: BAD_PORT }, 'k']
  },
  {
    name: '추가: Include 상태·순서·경로 모양',
    text: 'Host top\n  Port 1\nInclude inc\\a.conf\n  Port 2\n',
    files: {
      [F('inc\\a.conf')]: 'User from-inc\nHost in-a\n  Port 10\n',
      [F('inc\\b.CONF')]: 'Host in-b\n  Port 11\n'
    },
    queries: ['top', 'in-a', 'zzz']
  },
  {
    name: '추가: Include 글롭 정렬은 UTF-16 순서·대소문자 없는 파일 시스템',
    text: 'Include sort/*.conf\nInclude C:\\golden\\other\\*.CONF\nInclude "~/rel/x.conf" ~/rel/*.cfg\nInclude [x].conf\nInclude dir*\\a.conf\nInclude ssh/sub\\*\n',
    files: {
      [F('sort\\b.conf')]: 'IdentityFile b\n',
      [F('sort\\A.conf')]: 'IdentityFile A\n',
      [F('sort\\10.conf')]: 'IdentityFile 10\n',
      [F('sort\\é.conf')]: 'IdentityFile e-acute\n',
      [F('sort\\z.txt')]: 'IdentityFile no\n',
      'C:\\golden\\other\\one.conf': 'IdentityFile other-one\n',
      [`${HOME}\\rel\\x.conf`]: 'IdentityFile rel-x\n',
      [`${HOME}\\rel\\y.cfg`]: 'IdentityFile rel-y\n',
      [F('[x].conf')]: 'IdentityFile bracket\n',
      [F('dir1\\a.conf')]: 'IdentityFile dir-glob\n'
    },
    queries: ['h']
  },
  {
    name: '추가: Include 깊이 16',
    text: 'Include loop\n',
    files: { [F('loop')]: 'Host *\n  IdentityFile k\nInclude loop\n' },
    queries: ['x']
  },
  {
    name: '추가: Include 안의 Include 와 Match',
    text: 'Host a\nInclude one\n  Port 1\n',
    files: {
      [F('one')]: 'Include two\nMatch host zz\n  User skipped\n',
      [F('two')]: '  User deep\nHost deep\n  Port 7\n'
    },
    queries: ['a', 'deep', 'zz']
  }
]

/** `parseHostSpec` 표 */
export const SPEC_CASES: string[] = [
  'host',
  'user@host',
  'user@host:2200',
  'host:2200',
  '[::1]:22',
  '[::1]',
  '[fe80::1]:2222',
  'fe80::1',
  'a:b:c',
  'ssh://u@h:22/',
  'SSH://H/',
  'sSh://h//',
  ' padded ',
  '@host',
  'u@',
  'h:',
  ':22',
  'h:0022',
  'h:70000',
  'x@y@z:3',
  '[a]b',
  '[]:1',
  '[a]:',
  '[a]:1x',
  '',
  'ssh://',
  'h:١٢',
  'a\u2028b',
  '\u00a0h\ufeff'
]

// ── known_hosts ───────────────────────────────────────────────────────────────

const u32 = (n: number): Buffer => {
  const b = Buffer.alloc(4)
  b.writeUInt32BE(n)
  return b
}
const str = (s: Buffer | string): Buffer => {
  const b = typeof s === 'string' ? Buffer.from(s, 'latin1') : s
  return Buffer.concat([u32(b.length), b])
}
const blob = (type: string, seed: number): Buffer => Buffer.concat([str(type), str(Buffer.alloc(32, seed))])

/** 키 blob(결정적) */
export const KEYS: Record<string, Buffer> = {
  k1: blob('ssh-ed25519', 1),
  k2: blob('ssh-ed25519', 2),
  rsa: blob('ssh-rsa', 3),
  ecdsa: blob('ecdsa-sha2-nistp256', 4),
  short: Buffer.from([1, 2, 3]),
  over: Buffer.concat([u32(16), Buffer.from('ab')]),
  // 키 종류 문자열의 5 번째 바이트가 0xE5 — Node 'ascii' 디코딩은 최상위 비트를 지워 'ssh-ed25519' 로 읽는다
  hi: Buffer.concat([u32(11), Buffer.from([0x73, 0x73, 0x68, 0x2d, 0xe5, 0x64, 0x32, 0x35, 0x35, 0x31, 0x39]), str(Buffer.alloc(32, 5))]),
  empty: Buffer.alloc(0)
}

export const KEY_B64: Record<string, string> = Object.fromEntries(Object.entries(KEYS).map(([k, v]) => [k, v.toString('base64')]))

const hashedWith = (salt: Buffer, host: string, fix: (s: string) => string = (s) => s): string =>
  `|1|${fix(salt.toString('base64'))}|${fix(createHmac('sha1', salt).update(host).digest().toString('base64'))}`
const SALT = Buffer.from('0123456789abcdefghij')
const urlSafe = (s: string): string => s.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')

export interface KnownHostsQuery {
  host: string
  port: number
  key: string
}

export interface KnownHostsCase {
  name: string
  text: (b: Record<string, string>) => string
  queries: KnownHostsQuery[]
  /** 알려진 차이(비 ASCII 대소문자)라 Rust 가 TS 와 다르게 내는 질의의 `status` — 질의 순서대로 키(인덱스)와 값 */
  rust?: Record<number, string>
}

const q = (host: string, port: number, key: string): KnownHostsQuery => ({ host, port, key })

export const KNOWN_HOSTS_CASES: KnownHostsCase[] = [
  {
    name: '평문·쉼표 목록·와일드카드',
    text: (b) => `# c\nexample.com,10.0.0.1 ssh-ed25519 ${b.k1} comment\n*.corp.local ssh-ed25519 ${b.k1}\n`,
    queries: [q('example.com', 22, 'k1'), q('10.0.0.1', 22, 'k1'), q('a.corp.local', 22, 'k1'), q('EXAMPLE.com', 22, 'k1'), q('other.com', 22, 'k1')]
  },
  {
    name: '비표준 포트는 [host]:port 로만 일치한다',
    text: (b) => `[example.com]:2222 ssh-ed25519 ${b.k1}\n`,
    queries: [q('example.com', 2222, 'k1'), q('example.com', 22, 'k1'), q('example.com', 2223, 'k1')]
  },
  {
    name: '해시 항목(HMAC-SHA1), 포트 포함 형태도',
    text: (b) => `${hashedWith(SALT, 'example.com')} ssh-ed25519 ${b.k1}\n${hashedWith(SALT, '[example.com]:2200')} ssh-ed25519 ${b.k2}\n`,
    queries: [q('example.com', 22, 'k1'), q('example.com', 2200, 'k2'), q('example.com', 2200, 'k1'), q('nope.com', 22, 'k1')]
  },
  {
    name: '같은 종류의 다른 키는 불일치(mismatch)',
    text: (b) => `example.com ssh-ed25519 ${b.k1}\n`,
    queries: [q('example.com', 22, 'k2')]
  },
  {
    name: '@revoked 는 거부, @cert-authority 는 무시',
    text: (b) => `@revoked * ssh-ed25519 ${b.k2}\n@cert-authority *.x.com ssh-ed25519 ${b.k1}\nz.x.com ssh-ed25519 ${b.k1}\n`,
    queries: [q('z.x.com', 22, 'k2'), q('z.x.com', 22, 'k1'), q('a.x.com', 22, 'k1')]
  },
  {
    name: '부정 패턴과 빈 파일',
    text: (b) => `*.x.com,!bad.x.com ssh-ed25519 ${b.k1}\n`,
    queries: [q('good.x.com', 22, 'k1'), q('bad.x.com', 22, 'k1')]
  },
  {
    name: '등록된 키 종류로 serverHostKey 알고리즘을 제한한다',
    text: () => 'h ssh-rsa AAAA\nh ecdsa-sha2-nistp256 AAAA\n',
    queries: [q('h', 22, 'k1'), q('zz', 22, 'k1')]
  },
  // ── 추가 ────────────────────────────────────────────────────────────────────
  {
    name: '추가: 빈 파일·주석·CRLF·공백 모양·알 수 없는 표지',
    text: (b) => `\r\n# 주석\r\n  \t example.com\tssh-ed25519   ${b.k1}\r\n@unknown example.com ssh-ed25519 ${b.k2}\r\nshort ssh-ed25519\r\n@revoked\r\nexample.com\u00a0ssh-rsa\u3000${b.rsa}\r\n`,
    queries: [q('example.com', 22, 'k1'), q('example.com', 22, 'k2'), q('example.com', 22, 'rsa'), q('short', 22, 'k1')]
  },
  {
    name: '추가: 빈 텍스트',
    text: () => '',
    queries: [q('a', 22, 'k1')]
  },
  {
    name: '추가: revoked 는 같은 종류가 아니어도 그 키 자체만 거부한다',
    text: (b) => `@revoked h ssh-rsa ${b.rsa}\nh ssh-ed25519 ${b.k1}\n@revoked h ssh-ed25519 ${b.k2}\n`,
    queries: [q('h', 22, 'rsa'), q('h', 22, 'k1'), q('h', 22, 'k2'), q('h', 22, 'ecdsa')]
  },
  {
    name: '추가: 호스트 표기(대문자·IPv6·부정만 있는 목록·빈 항목)',
    text: (b) => `EXAMPLE.COM,[::1]:2222 ssh-ed25519 ${b.k1}\n!only.com ssh-ed25519 ${b.k1}\n,,x ssh-ed25519 ${b.k2}\n*? ssh-ed25519 ${b.k2}\n`,
    queries: [q('Example.Com', 22, 'k1'), q('::1', 2222, 'k1'), q('::1', 22, 'k1'), q('only.com', 22, 'k1'), q('x', 22, 'k2'), q('', 22, 'k2'), q('ab', 22, 'k2')]
  },
  {
    name: '추가: 해시 항목의 느슨한 base64(URL-safe·= 없음)와 깨진 항목',
    text: (b) =>
      [
        `${hashedWith(Buffer.alloc(20, 0xfb), 'a.com', urlSafe)} ssh-ed25519 ${b.k1}`,
        `${hashedWith(Buffer.alloc(20, 0xfe), 'b.com')} ssh-ed25519 ${b.k1}`,
        `|1|only-salt ssh-ed25519 ${b.k2}`,
        `|1||x ssh-ed25519 ${b.k2}`,
        `|1|${SALT.toString('base64')}|${Buffer.alloc(20).toString('base64')} ssh-ed25519 ${b.k2}`,
        `|2|a|b ssh-ed25519 ${b.k2}`,
        ''
      ].join('\n'),
    queries: [q('a.com', 22, 'k1'), q('b.com', 22, 'k1'), q('c.com', 22, 'k2'), q('A.COM', 22, 'k1')]
  },
  {
    name: '추가: 이상한 키 blob(짧음·길이 넘침·최상위 비트·빈 키)',
    text: (b) => `h ssh-ed25519 ${b.hi}\nh ssh-ed25519 ${b.k1}\n`,
    queries: [q('h', 22, 'hi'), q('h', 22, 'short'), q('h', 22, 'over'), q('h', 22, 'empty'), q('h', 22, 'k1')]
  },
  {
    name: '추가: 비 ASCII 호스트(대소문자)',
    text: (b) => `Éxample.com ssh-ed25519 ${b.k1}\n@revoked *.É ssh-ed25519 ${b.k2}\n`,
    queries: [q('Éxample.com', 22, 'k1'), q('éxample.com', 22, 'k1'), q('x.é', 22, 'k2'), q('x.É', 22, 'k2')],
    // TS 는 `é`·`É` 를 같은 글자로 본다(ok, ok, revoked, revoked). Rust 는 비 ASCII 호스트를 판정하지 않고 거부한다 — 전부 unsupported
    rust: { 0: 'unsupported', 1: 'unsupported', 2: 'unsupported', 3: 'unsupported' }
  }
]

// ── 원격 명령 인용 ─────────────────────────────────────────────────────────────

export const TRICKY = [
  '',
  'plain',
  'with space',
  "it's",
  "'",
  "''",
  'a"b',
  '$HOME `id` $(id) ${x}',
  '줄\n바꿈\r\n과 탭\t',
  '한글 경로/파일 이름.txt',
  '-leading-dash',
  '--format=%H%x00%s',
  'back\\slash\\',
  '\\',
  "mix \\' \\\\ '\" ; | & > < * ? ! ~ #",
  ':(top)*.c'
]

/** shQuote 골든에 더하는 입력(왕복 시험에는 쓰지 않는다) */
export const QUOTE_EXTRA = ['😀 이모지', 'a\u2028b\u0085c', "''\\\\''", '\\\'\\', 'a\0b', '\0', ' ', '\t', '~', '*', '-c', 'x=y']

/** buildRemoteCommand 골든 입력. `env` 는 닫힌 fetch 환경 둘(null 이면 GIT_ENV 만) */
export const REMOTE_ENVS: { label: string; fetch: { sshCommand: string; allowFile: boolean } | null }[] = [
  { label: 'GIT_ENV', fetch: null },
  { label: 'fetch', fetch: { sshCommand: 'ssh -F x', allowFile: false } },
  { label: 'fetch+file+특수 문자', fetch: { sshCommand: `ssh -i "k k" -o ProxyCommand='x y' \\ $HOME \`id\`\n한글`, allowFile: true } }
]

export const REMOTE_SAMPLES: { repoPath: string; args: string[] }[] = [
  { repoPath: '/home/me/repo', args: ['log', '-z'] },
  { repoPath: "/home/me/my repo's", args: ['log', '--format=%s', '--', '한글 파일.txt', "o'neil"] },
  { repoPath: '/tmp/a"b\'c $HOME', args: ['-c', 'alias.x=!echo $HOME', 'x', '--', '-weird path'] },
  { repoPath: 'C:\\Users\\me\\repo', args: ['show', 'back\\slash\\', '`id`', '$(id)', 'a\nb'] },
  { repoPath: '~/repo', args: [] },
  { repoPath: '/r', args: ['-c', 'alias.x=!rm -rf /', 'x'] },
  { repoPath: '/r', args: ['x\0'] },
  { repoPath: '/r\0', args: [] }
]
