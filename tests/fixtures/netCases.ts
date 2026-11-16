// GitHub·Jira 순수 로직(검증·URL 판정·응답 해석·Device Flow·ghEnv·토큰 가리기)의 입력 표. tests/goldenNet.test.ts 가 TS 구현으로 골든(tests/golden/net/*.json)을 만들고,
// Rust 이식(src-tauri/src/{net,github,jira}, 검사: src-tauri/tests/net.rs)이 같은 입력에서 같은 결과를 내는지 본다.
// 입력은 JSON 으로 오가는 값이다(IPC 인자와 같다) — 짝 없는 서로게이트·NaN·undefined 는 담지 않는다(JSON 텍스트 안의 `\ud800` 이스케이프는 된다).

const A = (c: string, n: number): string => c.repeat(n)
export const SHA40 = '0123456789abcdef0123456789abcdef01234567'
export const SHA64 = SHA40 + '89abcdef01234567'.repeat(1) + '89abcdef'

// ── 검증(validate) ────────────────────────────────────────────────────────

export const NUMBERS: unknown[] = [5, 1, 0, -1, 1.5, 999999999, 1e9, 1e9 - 1, 2147483648, 1e21, 5.0, '5', null, true, [5], {}, 9007199254740991, 9007199254740992, -0.5, 0.1]

export const IDS: unknown[] = ['abc', '', null, 5, A('a', 200), A('a', 201), 'a\nb', 'a\x7fb', '😀'.repeat(100), '😀'.repeat(101), ' ', 'a\u2028b', 'a\u0085b', ['a'], {}]

export const LIST_QUERIES: unknown[] = [
  { state: 'open' },
  { state: 'closed' },
  { state: 'all' },
  { state: 'bad' },
  { state: 'OPEN' },
  { state: 5 },
  {},
  null,
  [],
  [{ state: 'open' }],
  'open',
  5,
  { state: 'all', label: 'bug', assignee: 'x', page: 3 },
  { state: 'open', label: '', assignee: '' },
  { state: 'open', label: null, assignee: null },
  { state: 'open', label: 5 },
  { state: 'open', label: [] },
  { state: 'open', assignee: {} },
  { state: 'open', label: A('a', 100) },
  { state: 'open', label: A('a', 101) },
  { state: 'open', label: '😀'.repeat(50) },
  { state: 'open', label: '😀'.repeat(51) },
  { state: 'open', label: 'a\nb' },
  { state: 'open', assignee: 'a\x7f' },
  { state: 'open', label: A('a', 101), assignee: 5 },
  { state: 'open', page: 0 },
  { state: 'open', page: 1 },
  { state: 'open', page: 1000 },
  { state: 'open', page: 1001 },
  { state: 'open', page: 1.5 },
  { state: 'open', page: '2' },
  { state: 'open', page: null },
  { state: 'open', page: true },
  { state: 'open', page: 2.0 },
  { state: 'open', extra: 1, page: 7 }
]

export const PATS: unknown[] = [
  'ghp_abc123',
  ' ghp_x ',
  '',
  ' ',
  'a b',
  'a\tb',
  '\u00a0x',
  '\u00a0',
  '\ufeffabc\ufeff',
  '\u0085abc',
  'abc\u0085',
  '\u2028abc\u2029',
  A('x', 255),
  A('x', 256),
  ' ' + A('x', 255) + ' ',
  '한글',
  '~',
  '!',
  ' ',
  '\x7f',
  'ab\n',
  'ab\ncd',
  '\x1fabc',
  '😀',
  123,
  null,
  ['x'],
  {}
]

export const AVATAR_AUTHORS: unknown[] = [
  [],
  [{ email: 'a@b.c', sha: SHA40 }],
  [{ email: 'A@B.C', sha: SHA40.toUpperCase() }],
  [{ email: 'a@b.c', sha: SHA40 }, { email: 'A@b.C', sha: SHA64 }],
  [{ email: 'a@b.c', sha: SHA64 }, { email: 'b@b.c', sha: SHA40 }],
  [{ email: 'İ@x.y', sha: SHA40 }, { email: 'i̇@x.y', sha: SHA40 }],
  [{ email: 'ΑΣ@x.y', sha: SHA40 }, { email: 'ας@x.y', sha: SHA40 }],
  [{ email: 'ab', sha: SHA40 }],
  [{ email: 'abc', sha: SHA40 }],
  [{ email: A('a', 254), sha: SHA40 }],
  [{ email: A('a', 255), sha: SHA40 }],
  [{ email: '😀'.repeat(127), sha: SHA40 }],
  [{ email: '😀'.repeat(128), sha: SHA40 }],
  [{ email: 'a\n@b.c', sha: SHA40 }],
  [{ email: 5, sha: SHA40 }],
  [{ sha: SHA40 }],
  [{ email: 'a@b.c' }],
  [{ email: 'a@b.c', sha: 5 }],
  [{ email: 'a@b.c', sha: SHA40.slice(1) }],
  [{ email: 'a@b.c', sha: SHA40 + '0' }],
  [{ email: 'a@b.c', sha: A('g', 40) }],
  [{ email: 'a@b.c', sha: SHA40 + '\n' }],
  [null],
  [5],
  ['x'],
  [[]],
  [[{ email: 'a@b.c', sha: SHA40 }]],
  [{ email: 'a@b.c', sha: SHA40 }, null],
  [{ email: 'bad', sha: SHA40 }, { email: 'ok@x.y', sha: 'zz' }],
  [{ email: 'ok@x.y', sha: 'zz' }, { email: 'bad', sha: SHA40 }],
  Array.from({ length: 200 }, (_, i) => ({ email: `u${i}@x.y`, sha: SHA40 })),
  Array.from({ length: 201 }, (_, i) => ({ email: `u${i}@x.y`, sha: SHA40 })),
  Array.from({ length: 201 }, () => ({ email: 'same@x.y', sha: SHA40 })),
  null,
  'x',
  {},
  5
]

export const RUN_IDS: unknown[] = [1, 0, -1, 123456789012, 9007199254740991, 9007199254740992, 1.5, '1', 1e21, null, true, [1], 2 ** 53 - 2, 4294967296]

export const SHAS: unknown[] = [
  SHA40,
  SHA40.toUpperCase(),
  'ABCDEF0123456789abcdef0123456789ABCDEF01',
  SHA64,
  SHA64.toUpperCase(),
  SHA40.slice(1),
  SHA40 + '0',
  A('g', 40),
  SHA40 + '\n',
  ' ' + SHA40,
  'ｆ'.repeat(40),
  'ſ' + SHA40.slice(1),
  5,
  null,
  []
]

export const CHECK_SHAS: unknown[] = [
  [],
  [SHA40],
  [SHA40, SHA40.toUpperCase(), SHA64],
  [SHA64, SHA40, SHA64.toUpperCase()],
  [SHA40, 'bad'],
  ['bad', 5],
  [5, 'bad'],
  Array.from({ length: 100 }, (_, i) => i.toString(16).padStart(40, '0')),
  Array.from({ length: 101 }, (_, i) => i.toString(16).padStart(40, '0')),
  Array.from({ length: 100 }, () => SHA40),
  null,
  'x',
  {},
  5,
  [null]
]

export const PAGES: unknown[] = [1, 2, 1000, 1001, 0, -1, 1.5, 1.0, '1', null, true, [], 1e21]

const GITHUB_URLS = [
  'https://github.com/x',
  'https://github.com',
  'https://github.com/',
  'http://github.com',
  'https://github.com:443/',
  'https://github.com:8443/',
  'https://user@github.com',
  'https://:pw@github.com',
  'https://@github.com',
  'https://:@github.com/x',
  'https://api.github.com',
  'https://github.com.evil.com',
  'https://evil.com@github.com',
  'https://github.com@evil.com',
  'HTTPS://GITHUB.COM/Path',
  'https://github.com\\x',
  'https://github.com/?q=1#f',
  ' https://github.com ',
  'javascript:alert(1)',
  'file:///c:/x',
  '',
  'github.com',
  '//github.com',
  'https://github.com./',
  'https://git\thub.com/',
  'https://github.com/%2e%2e/x',
  'blob:https://github.com/uuid',
  'https://github.com/' + '😀'.repeat(1012),
  'https://github.com/' + '😀'.repeat(1013),
  'https://github.com/' + 'a'.repeat(2029),
  'https://github.com/' + 'a'.repeat(2030)
]
export const EXTERNAL_URLS: unknown[] = [...GITHUB_URLS, 5, null, true, {}, []]

// ── Jira ──────────────────────────────────────────────────────────────────

export const SITE_RAWS: unknown[] = [
  'https://a.atlassian.net',
  'https://a.atlassian.net/',
  'https://a.atlassian.net///',
  ' https://a.atlassian.net ',
  '\u00a0https://a.atlassian.net\u00a0',
  '\ufeffhttps://a.atlassian.net',
  '\u0085https://a.atlassian.net',
  'https://h.example.com/jira',
  'https://h.example.com/jira/',
  'https://h.example.com/jira//',
  'https://h.example.com:8443/jira',
  'http://127.0.0.1:8080',
  'http://localhost/jira',
  'http://LOCALHOST:8080',
  'http://127.0.0.1.:8080',
  'http://[::1]:8080',
  'https://127.0.0.1:8080',
  'http://a.atlassian.net',
  'https://user@a.atlassian.net',
  'https://u:p@a.atlassian.net',
  'https://:p@a.atlassian.net',
  'https://@a.atlassian.net',
  'https://a.atlassian.net/?',
  'https://a.atlassian.net/#',
  'https://a.atlassian.net/?#',
  'https://a.atlassian.net?x=1',
  'https://a.atlassian.net#x',
  'https://a.atlassian.net/jira?',
  'ftp://a.atlassian.net',
  'a.atlassian.net',
  '',
  ' ',
  '\u2028',
  'https://',
  'https:///',
  'https://a b.com',
  'https://a.atlassian.net/a b',
  'https://a.atlassian.net/%2e%2e/x',
  'https://a.atlassian.net/한글',
  'https://é.example.com/jira',
  'https://münchen.de/j',
  'https://日本語.jp',
  'https://A.ATLASSIAN.NET/JIRA',
  'https://a.atlassian.net/\u0000',
  'https://a.atlassian.net/\u001f',
  'https://a.atlassian.net/\u007f',
  'https://a.atlassian.net/\t',
  'https://a.atlassian.net/\n',
  'https://a.atlassian.net/' + A('a', 2024),
  'https://a.atlassian.net/' + A('a', 2025),
  'https://a.atlassian.net/' + '😀'.repeat(1012),
  'https://a.atlassian.net/' + '😀'.repeat(1013),
  ' ' + 'https://a.atlassian.net/' + A('a', 2023) + ' ',
  5,
  null,
  true,
  {},
  []
]

const CONNECT_BASE = { kind: 'cloud', siteUrl: 'https://a.atlassian.net', email: 'me@x.y', token: 'tok123' }
export const CONNECT_INPUTS: unknown[] = [
  CONNECT_BASE,
  { ...CONNECT_BASE, kind: 'server' },
  { ...CONNECT_BASE, kind: 'server', email: '' },
  { ...CONNECT_BASE, kind: 'server', email: 'bad:email' },
  { ...CONNECT_BASE, kind: 'other' },
  { ...CONNECT_BASE, kind: undefined },
  { ...CONNECT_BASE, kind: 5 },
  { ...CONNECT_BASE, siteUrl: 'http://a.atlassian.net' },
  { ...CONNECT_BASE, siteUrl: 5 },
  { ...CONNECT_BASE, siteUrl: undefined },
  { ...CONNECT_BASE, siteUrl: 'https://a.atlassian.net/jira/?x' },
  { ...CONNECT_BASE, siteUrl: 'http://127.0.0.1:8080' },
  { ...CONNECT_BASE, token: undefined },
  { ...CONNECT_BASE, token: 5 },
  { ...CONNECT_BASE, token: '' },
  { ...CONNECT_BASE, token: '  ' },
  { ...CONNECT_BASE, token: ' tok ' },
  { ...CONNECT_BASE, token: 'a b' },
  { ...CONNECT_BASE, token: A('t', 1024) },
  { ...CONNECT_BASE, token: A('t', 1025) },
  { ...CONNECT_BASE, token: '한글' },
  { ...CONNECT_BASE, token: '\u0085tok' },
  { ...CONNECT_BASE, email: undefined },
  { ...CONNECT_BASE, email: 5 },
  { ...CONNECT_BASE, email: '' },
  { ...CONNECT_BASE, email: '   ' },
  { ...CONNECT_BASE, email: ' me@x.y ' },
  { ...CONNECT_BASE, email: 'a:b' },
  { ...CONNECT_BASE, email: 'a\nb' },
  { ...CONNECT_BASE, email: A('e', 254) },
  { ...CONNECT_BASE, email: A('e', 255) },
  { ...CONNECT_BASE, email: '😀'.repeat(127) },
  { ...CONNECT_BASE, email: '😀'.repeat(128) },
  { kind: 'cloud', siteUrl: 'bad', email: '', token: '' },
  { kind: 'cloud', siteUrl: 'https://a.atlassian.net', email: 'a:b', token: '' },
  { kind: 'cloud', siteUrl: 'https://a.atlassian.net', email: '', token: 'ok' },
  null,
  5,
  'x',
  [],
  [CONNECT_BASE],
  {}
]

export const ISSUE_KEYS: unknown[] = [
  'ABC-1',
  'A1-1',
  'AB-123456789',
  'AB-1234567890',
  'AB-',
  '-1',
  'A-1',
  'AB_C-9',
  'A' + A('B', 19) + '-1',
  'A' + A('B', 20) + '-1',
  'ab-1',
  'AB-1\n',
  'AB-١',
  'AB-1-2',
  'AB--1',
  'AB-0',
  'AB-01',
  '1A-1',
  '_A-1',
  'ＡＢ-1',
  'AB 1',
  '',
  5,
  null,
  ['AB-1']
]

export const TRANSITION_IDS: unknown[] = ['1', '11', '1234567890', '12345678901', '', '0', '01', '1a', ' 1', '1\n', '١', 1, null, '-1', '１']

export const SEARCH_QUERIES: unknown[] = [
  { jql: 'project = X' },
  { jql: '  project = X  ' },
  { jql: '' },
  { jql: '   ' },
  { jql: '\u00a0' },
  { jql: A('j', 2000) },
  { jql: A('j', 2001) },
  { jql: ' ' + A('j', 2000) + ' ' },
  { jql: '😀'.repeat(1000) },
  { jql: '😀'.repeat(1001) },
  { jql: 'a\nb' },
  { jql: 'a\tb' },
  { jql: 'a\x7fb' },
  { jql: 5 },
  {},
  null,
  [],
  'x',
  { jql: 'x', page: 1 },
  { jql: 'x', page: 8 },
  { jql: 'x', page: 9 },
  { jql: 'x', page: 0 },
  { jql: 'x', page: 2.5 },
  { jql: 'x', page: '2' },
  { jql: 'x', page: null },
  { jql: 'x', cursor: 'abc123' },
  { jql: 'x', cursor: '' },
  { jql: 'x', cursor: null },
  { jql: 'x', cursor: 5 },
  { jql: 'x', cursor: 'a b' },
  { jql: 'x', cursor: '한' },
  { jql: 'x', cursor: A('c', 2000) },
  { jql: 'x', cursor: A('c', 2001) },
  { jql: 'x', cursor: '~!' },
  { jql: 'x', cursor: 'a\n' },
  { jql: 'x', page: 3, cursor: 'tok' },
  { jql: A('j', 2001), page: 99 }
]


const WL = { from: '2025-01-01', to: '2025-01-31', users: ['u1'] }
export const WORKLOG_QUERIES: unknown[] = [
  WL,
  { ...WL, includeMe: true },
  { ...WL, includeMe: false },
  { ...WL, includeMe: null },
  { ...WL, includeMe: 'true' },
  { ...WL, includeMe: 1 },
  { ...WL, users: [] },
  { ...WL, users: ['u1', 'u1', 'u2', 'u1'] },
  { ...WL, users: Array.from({ length: 30 }, (_, i) => `u${i}`) },
  { ...WL, users: Array.from({ length: 31 }, (_, i) => `u${i}`) },
  { ...WL, users: Array.from({ length: 31 }, () => 'u') },
  { ...WL, users: ['bad user'] },
  { ...WL, users: [5] },
  { ...WL, users: 'u1' },
  { ...WL, users: undefined },
  { ...WL, users: null },
  { ...WL, from: '2025-02-30' },
  { ...WL, from: '2024-02-29', to: '2024-02-29' },
  { ...WL, from: '2023-02-29', to: '2023-03-01' },
  { ...WL, from: '1900-02-28', to: '1900-03-01' },
  { ...WL, from: '2000-02-29', to: '2000-02-29' },
  { ...WL, from: '1970-01-01', to: '1970-01-01' },
  { ...WL, from: '1969-12-31', to: '1970-01-01' },
  { ...WL, from: '2025-13-01' },
  { ...WL, from: '2025-00-10' },
  { ...WL, from: '2025-01-00' },
  { ...WL, from: '2025-01-32' },
  { ...WL, from: '2025-04-31', to: '2025-05-01' },
  { ...WL, from: '2025-1-01' },
  { ...WL, from: '2025-01-01 ' },
  { ...WL, from: '２０２５-01-01' },
  { ...WL, from: '2025-01-01\n' },
  { ...WL, from: 20250101 },
  { ...WL, from: undefined },
  { ...WL, to: undefined },
  { ...WL, to: '2024-12-31' },
  { ...WL, from: '2025-01-01', to: '2025-03-03' },
  { ...WL, from: '2025-01-01', to: '2025-03-02' },
  { ...WL, from: '2025-01-01', to: '2025-03-01' },
  { ...WL, from: '2024-12-31', to: '2025-03-01' },
  { ...WL, from: '9999-12-31', to: '9999-12-31' },
  { ...WL, from: '0000-01-01', to: '2025-01-01' },
  { ...WL, from: '1999-12-31', to: '2000-01-01' },
  null,
  5,
  'x',
  [],
  {},
  [WL]
]

export const JQL_VALUES: string[] = ['u1', '712020:uuid-1', '2025-01-01', 'a"b', '', 'a b', A('a', 128), A('a', 129), 'a\\b', '한', 'a@b.c', 'a+b']
export const USER_SEARCHES: unknown[] = ['kim', '  kim  ', '', '   ', '\u00a0', A('k', 100), A('k', 101), '😀'.repeat(50), '😀'.repeat(51), 'a\nb', 'a\x7f', ' a\u0085', 5, null, [], {}, ' ' + A('k', 100) + ' ']

// ── URL 격자 ──────────────────────────────────────────────────────────────

export const SITE_A = 'https://a.atlassian.net'
export const SITE_B = 'https://h.example.com/jira'
export const SITE_L = 'http://127.0.0.1:8080/jira'

const HOSTS = [
  'github.com', 'GitHub.COM', 'github.com.', 'github.com:443', 'github.com:', 'github.com:80', 'github.com:8443', 'github.com:0443',
  'user@github.com', 'user:pw@github.com', '@github.com', ':@github.com', 'github.com@evil.com', 'github.com\\@evil.com', 'evil.com\\@github.com',
  'git%68ub.com', 'ｇｉｔｈｕｂ.com', 'git\u00adhub.com', 'github。com',
  'a.atlassian.net', 'A.ATLASSIAN.NET', 'a.atlassian.net.', 'h.example.com', 'h.example.com:443', 'h.example.com:8443', 'H.EXAMPLE.COM:80',
  '127.0.0.1:8080', '127.0.0.1', '127.1', '0x7f.1', '2130706433', 'localhost:8080', 'LOCALHOST', '[::1]:8080', '[::ffff:127.0.0.1]',
  'é.example.com', 'xn--9ca.example.com', 'münchen.de', '日本語.jp', 'a_b.com', 'a b.com', 'a..b.com', '-a.com'
]
const SUFFIXES = [
  '', '/', '/x', '/jira', '/jira/', '/jira/x', '/jira2', '/JIRA/x', '/jira/%2f', '/jira%2Fx', '/jira/%5C', '/jira/%2e/', '/jira/.%2E/x', '/jira/../x',
  '//x', '/jira//x', '\\x', '/jira\\x', '?', '?a=1', '#', '#a', '?#', '/?', '/#', '/x?a#b'
]
const PREFIXES = ['http://', 'HTTPS://', 'https:', 'https:\\\\', '//', '', ' https://', 'https:///', 'https:/', 'ftp://', 'https://\t', 'ht\ttps://']
const EXTRA_HOSTS = ['github.com', 'a.atlassian.net', 'h.example.com', '127.0.0.1:8080', 'user@github.com', 'localhost:8080', '[::1]:8080']
const EXTRA_SUFFIXES = ['', '/', '/jira/x', '?']

/** 허용 판정을 돌릴 주소 문자열 격자(결정적) */
export function urlGrid(): string[] {
  const out: string[] = []
  for (const h of HOSTS) for (const s of SUFFIXES) out.push(`https://${h}${s}`)
  for (const p of PREFIXES) for (const h of EXTRA_HOSTS) for (const s of EXTRA_SUFFIXES) out.push(`${p}${h}${s}`)
  out.push(
    'https:github.com', 'https:\\\\github.com', 'https:///github.com', 'https:////github.com', 'https://github.com\\@evil', 'https://\\github.com', 'https://github.com%2f@evil.com',
    'https://github.com/\u0000', 'https://github.com/\u001f', 'https://github.com/\u007f', 'https://gith\u0000ub.com', 'https://github.com/\n', 'https://github.com/\r\n',
    '\u0000https://github.com', '\u001fhttps://github.com', '  https://github.com', 'https://github.com/ ', 'https://github.com/\u00a0',
    'https://[::1]/', 'https://[::1', 'https://[0:0:0:0:0:0:0:1]/', 'https://[::ffff:1.2.3.4]:443/', 'https://[fe80::1%25eth0]/', 'https://1.2.3/', 'https://1.2.3.4.5/', 'https://0x7f.0.0.1/', 'https://0177.0.0.1/', 'https://256.1.1.1/',
    'https://a.atlassian.net:65535/', 'https://a.atlassian.net:65536/', 'https://a.atlassian.net:-1/', 'https://a.atlassian.net:abc/', 'https://a.atlassian.net:443:443/',
    'https://h.example.com/jira%2f..%2fx', 'https://h.example.com/jira/%252e', 'https://h.example.com/jira/%2', 'https://h.example.com/jira/%2G', 'https://h.example.com/jira/%c0%ae', 'https://h.example.com/jira/\u2028',
    'https://h.example.com/jira/./%2e', 'https://h.example.com/jira/..;/x', 'https://h.example.com/jiraX', 'https://h.example.com/jira%00', 'https://h.example.com/ji%72a/x',
    'http://127.0.0.1:8080/jira/x', 'http://127.0.0.1:8080/jira', 'http://127.0.0.1:8080', 'http://127.0.0.1:80/jira', 'http://127.0.0.1/jira', 'http://localhost:8080/jira',
    'blob:https://a.atlassian.net/uuid', 'blob:https://h.example.com/jira/uuid', 'data:text/plain,x', 'about:blank', 'mailto:a@b.c', 'file:///jira', 'file://a.atlassian.net/x', 'ws://a.atlassian.net/', 'wss://a.atlassian.net/jira/x',
    'https://xn--github-.com', 'https://xn--a.com', 'https://a.com.', 'https://a.com..', 'https://.a.com', 'https://a.com/%', 'https://%41.atlassian.net', 'https://a%2eatlassian.net', 'https://a.atlassian.net%2f',
    'https://ß.example.com', 'https://ǆ.example.com', 'https://ẞ.example.com', 'https://a。atlassian。net', 'https://ａ.atlassian.net', 'https://a.atlassian.net\u3002', 'https://A\u00adB.example.com',
    'https://\u03c2.example.com', 'https://\u03c3.example.com', 'https://\u03a3.example.com', 'https://\u0391\u03a3.example.com', 'https://\u03b1\u03c2.example.com', 'https://a\u200db.example.com', 'https://a\u200cb.example.com', 'https://\uff41\uff42.example.com',
    'https://\u0149.example.com', 'https://\u01f0.example.com', 'https://\u0130.example.com', 'https://\u0131.example.com', 'https://\u017f.example.com', 'https://K.example.com', 'https://\u00c5.example.com', 'https://\u03a9.example.com', 'https://\u2116.example.com',
    'https://\u3392.example.com', 'https://\ufb01.example.com', 'https://\u2460.example.com', 'https://\u2025.example.com', 'https://\uff71.example.com', 'https://\u01c5.example.com', 'https://\u1fb3.example.com', 'https://\ua78e.example.com', 'https://\ua7ce.example.com',
    'https://\uff21.example.com', 'https://a\u0301.example.com', 'https://\u0301a.example.com', 'https://a.\u200d.example.com', 'https://\u00adx.example.com', 'https://x\u034f.example.com', 'https://\u1100.example.com', 'https://\ud55c\uae00.example.com', 'https://\u1e9e.\u1e9e',
    // \ud30c\uc11c\uac00 \uc9c0\uc6b0\ub294 \ud0ed\u00b7\uac1c\ud589\u00b7\ud37c\uc13c\ud2b8 \uc774\uc2a4\ucf00\uc774\ud504\ub85c \ud638\uc2a4\ud2b8 \uac80\uc0ac\ub97c \ube44\ucf1c \uac00\ub824\ub294 \uc785\ub825(U+04C0\u00b7U+FFA0 \uc740 Node \uac00 \uac70\ubd80\ud558\ub294 \ubb38\uc790)
    'ht\ttps://a\u04c0b.example.com', 'https:/\t/a\u04c0b.example.com', 'https://a\t\u04c0b.example.com', 'https://a\n\u04c0b.example.com', 'https://a\u04c0\rb.example.com', 'https://\u04c0.example.com',
    'https://a%D3%80b.example.com', 'https://a%d3%80b.example.com', 'https://a%EF%BE%A0b.example.com', 'https://%D3%80.example.com', 'https://a%c3%a9b.example.com', 'https://a%41b.example.com', 'https://a%2Fb.example.com',
    'https://u%C3%A9@a.example.com', 'https://a.example.com/%D3%80', 'https://a.example.com/\u04c0', 'https://a.example.com:80%D3%80/', 'https://xn--ab-3hc.example.com', 'https://xn--e1b.example.com',
    'blob:https://a.atlassian.net/uuid', 'blob:https://a\u04c0b.example.com/uuid', 'BLOB:https://h.example.com/jira/x',
    'https://hello\u200bworld.com', 'https://a\ufeff.com', 'https://a\u2028.com', 'https://\u202ea.com', 'https://a.com\u0085', 'https://‮.com'
  )
  return out
}

/** Rust 가 IDN 호스트에서 받는 비 ASCII 문자 범위(src-tauri/src/net/url_rules.rs 의 IDNA_SAFE_RANGES 와 같아야 한다 — net.rs 가 대조한다) */
export const IDNA_SWEEP_RANGES: [number, number][] = [
  [0x00a1, 0x024f], [0x0250, 0x02af], [0x0300, 0x036f], [0x0370, 0x03ff], [0x0400, 0x052f], [0x0530, 0x058f], [0x0590, 0x05ff], [0x0600, 0x06ff],
  [0x0e00, 0x0e7f], [0x1100, 0x11ff], [0x1e00, 0x1fff], [0x3000, 0x303f], [0x3040, 0x30ff], [0x3400, 0x4dbf], [0x4e00, 0x9fff], [0xac00, 0xd7a3], [0xff00, 0xffef]
]

// ── 요청 URL(GitHub·Jira) ─────────────────────────────────────────────────

/** [경로, 쿼리 [[키, 값|null]...]]. 값 null 은 `undefined`(건너뛴다) */
export type RequestCase = [string, [string, string | number | null][]]
export const GH_REQUESTS: RequestCase[] = [
  ['/repos/o/r/pulls', []],
  ['/repos/o/r/pulls', [['state', 'open'], ['page', 2], ['per_page', 100]]],
  ['/repos/o/r/pulls', [['state', 'open'], ['label', null]]],
  ['/repos/o/r/pulls', [['q', 'a b&c=d'], ['r', 'é😀'], ['s', "it's *~()!"], ['t', '']]],
  ['/repos/o/r/pulls', [['n', 1e21], ['m', 0.5], ['k', -3], ['z', 0], ['y', 1e-7]]],
  ['/repos/o/r/pulls?a=1&b=2', [['b', 'x']]],
  ['/repos/o/r/pulls?a=1&a=2&b=3', [['a', 'z']]],
  ['/repos/o/r/pulls?a=1', []],
  ['/repos/o/r/pulls?a=%7E+x', [['b', '1']]],
  ['/repos/o/r/pulls?a=%7E+x', []],
  ['/repos/o/r/pulls?', [['a', '1']]],
  ['/repos/o/r/pulls?', []],
  ['/repos/o/r/pulls#frag', [['a', '1']]],
  ['/repos/o/r/pulls?x=1#frag', [['a', '1']]],
  ['/repos/o/r/pulls?a=1', [['a', null]]],
  ['/repos/o/r/pulls?a=1&a=2', [['a', null], ['b', null]]],
  ['/search/issues', [['q', 'repo:o/r is:pr label:"x y"']]],
  ['/a/../b', []],
  ['/a/./b/', [['x', '1']]],
  ['/a b/한글', [['x', '1']]],
  ['/%2e%2e/x', []],
  ['/..', []],
  ['//evil.com/x', []],
  ['/\\evil.com/x', []],
  ['/\\\\evil.com', []],
  ['/@evil.com', []],
  ['/:80/x', []],
  ['/\t/x', []],
  ['/\n', []],
  ['repos', []],
  ['', []],
  [' /x', []],
  ['?x', []],
  ['#x', []],
  ['/x\u0000', []],
  ['/x y', [['a b', 'c d'], ['a b', 'e']]]
]

export type JiraRequestCase = [string, string, [string, string | number | null][]]
const JIRA_SITES = ['https://a.atlassian.net', 'https://h.example.com/jira', 'https://h.example.com/jira/', 'http://127.0.0.1:8080/jira', 'https://a.atlassian.net:8443', 'not a url', '']
export const JIRA_REQUESTS: JiraRequestCase[] = [
  ...JIRA_SITES.flatMap((s): JiraRequestCase[] => [
    [s, '/rest/api/3/issue/AB-1', []],
    [s, '/rest/api/3/search', [['jql', 'project = X AND "a b"'], ['maxResults', 25], ['fields', null]]],
    [s, '//evil.com/x', []],
    [s, 'rest/api', []],
    [s, '', []],
    [s, '/rest/../../x', []],
    [s, '/rest/%2e%2e/x', []],
    [s, '\\x', []],
    [s, '/\\evil.com', []],
    [s, '/@evil.com', []],
    [s, '/x?a=1', [['a', '2'], ['b', '3']]],
    [s, '/x?a=1&a=2#f', [['a', '9']]],
    [s, '/x#f', []]
  ]),
  ['https://a.atlassian.net', '/x/\u0000', []],
  ['https://a.atlassian.net', '/한글/é', [['q', '한글']]]
]

// ── 프로필 사진 홉(JiraClient.image) ──────────────────────────────────────

/** 스크립트: href → [상태, Location|null]. 없는 주소는 404. 상태 200 은 image/png */
export interface ImageCase {
  site: string
  href: string
  script: Record<string, [number, string | null]>
}
const PNG = 'https://secure.gravatar.com/avatar/x.png'
export const IMAGE_CASES: ImageCase[] = [
  { site: SITE_B, href: 'https://h.example.com/jira/secure/useravatar?id=1', script: { 'https://h.example.com/jira/secure/useravatar?id=1': [200, null] } },
  { site: SITE_B, href: 'https://h.example.com/jira/secure/useravatar?id=1', script: { 'https://h.example.com/jira/secure/useravatar?id=1': [302, 'https://secure.gravatar.com/avatar/x'], 'https://secure.gravatar.com/avatar/x': [200, null] } },
  { site: SITE_B, href: 'https://h.example.com/jira/secure/useravatar?id=1', script: { 'https://h.example.com/jira/secure/useravatar?id=1': [404, null] } },
  { site: SITE_B, href: 'https://h.example.com/other', script: { 'https://h.example.com/other': [200, null] } },
  { site: SITE_B, href: 'https://h.example.com/jira/%2fx', script: { 'https://h.example.com/jira/%2fx': [200, null] } },
  { site: SITE_B, href: 'https://h.example.com/jira/x', script: { 'https://h.example.com/jira/x': [301, '/jira/y'], 'https://h.example.com/jira/y': [200, null] } },
  { site: SITE_B, href: 'https://u@h.example.com/jira/x', script: { 'https://h.example.com/jira/x': [200, null] } },
  { site: SITE_B, href: 'https://:p@h.example.com/jira/x', script: {} },
  { site: SITE_B, href: PNG, script: { [PNG]: [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://i0.wp.com/a.png'], 'https://i0.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'http://i0.wp.com/a.png'], 'http://i0.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://i4.wp.com/a.png'], 'https://i4.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://h.example.com/jira/x'], 'https://h.example.com/jira/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://u@i0.wp.com/a.png'], 'https://i0.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://i0.wp.com:443/a.png'], 'https://i0.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://i0.wp.com:8443/a.png'], 'https://i0.wp.com:8443/a.png': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, '//i1.wp.com/a.png'], 'https://i1.wp.com/a.png': [200, null] } },
  { site: SITE_B, href: 'https://a.atl-paas.net/x', script: { 'https://a.atl-paas.net/x': [302, 'https://b.atl-paas.net/y'], 'https://b.atl-paas.net/y': [302, 'https://c.atl-paas.net/z'], 'https://c.atl-paas.net/z': [302, 'https://secure.gravatar.com/q'], 'https://secure.gravatar.com/q': [200, null] } },
  {
    site: SITE_B,
    href: 'https://a.atl-paas.net/x',
    script: { 'https://a.atl-paas.net/x': [302, '/1'], 'https://a.atl-paas.net/1': [302, '/2'], 'https://a.atl-paas.net/2': [302, '/3'], 'https://a.atl-paas.net/3': [302, '/4'], 'https://a.atl-paas.net/4': [200, null] }
  },
  { site: SITE_B, href: 'https://a.atl-paas.net.evil.com/x', script: { 'https://a.atl-paas.net.evil.com/x': [200, null] } },
  { site: SITE_B, href: 'https://A.ATL-PAAS.NET/x', script: { 'https://a.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: 'https://secure.gravatar.com:443/x', script: { 'https://secure.gravatar.com/x': [200, null] } },
  { site: SITE_B, href: 'http://secure.gravatar.com/x', script: { 'http://secure.gravatar.com/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://aӀb.atl-paas.net/x'], 'https://xn--ab-3hc.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, '//aӀb.atl-paas.net/x'], 'https://xn--ab-3hc.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://a%D3%80b.atl-paas.net/x'], 'https://xn--ab-3hc.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://a\tb.atl-paas.net/x'], 'https://ab.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'https://é.atl-paas.net/x'], 'https://xn--9ca.atl-paas.net/x': [200, null] } },
  { site: SITE_B, href: PNG, script: { [PNG]: [302, 'blob:https://i0.wp.com/x'] } },
  { site: SITE_B, href: 'not a url', script: {} },
  { site: SITE_B, href: '/relative', script: {} },
  { site: SITE_L, href: 'http://127.0.0.1:8080/jira/avatar', script: { 'http://127.0.0.1:8080/jira/avatar': [200, null] } },
  { site: SITE_A, href: 'https://a.atlassian.net/secure/x', script: { 'https://a.atlassian.net/secure/x': [200, null] } },
  { site: SITE_A, href: 'https://a.atlassian.net:443/secure/x', script: { 'https://a.atlassian.net/secure/x': [200, null] } }
]

export const AVATAR_HOSTS: string[] = [
  'secure.gravatar.com', 'i0.wp.com', 'i1.wp.com', 'i2.wp.com', 'i3.wp.com', 'i4.wp.com', 'i.wp.com', 'ii.wp.com', 'I0.wp.com', 'a.atl-paas.net', 'a-b.c1.atl-paas.net',
  'x.atl-paas.net.atl-paas.net', '.atl-paas.net', 'a..atl-paas.net', 'atl-paas.net', 'A.atl-paas.net', 'a.atl-paas.net.', 'a_b.atl-paas.net', 'a.atl-paas.net\n', 'gravatar.com',
  'secure.gravatar.com.', 'evil.secure.gravatar.com', 'secure.gravatar.com.evil.com', 'a.atl-paas.net.evil.com', 'xn--a.atl-paas.net', 'é.atl-paas.net', 'a.atl‐paas.net', 'a.atl-paas.net ',
  '-.atl-paas.net', '0.atl-paas.net', '', 'i0.wp.co', 'i0xwp.com', 'secure-gravatar.com', 'a.b.c.d.e.atl-paas.net'
]

// ── 응답 해석 ─────────────────────────────────────────────────────────────

export const RETRY_AFTER_VALUES: (string | null)[] = [
  null, '', '0', '1', '3', '3.2', '4', '60', '-1', '-0', '1e3', '1e21', '1e400', '0x10', '0b11', '0o7', ' 5 ', '\t5\n', '\u00a05\u00a0', '+5', '.5', '5.', '1_000', 'Infinity', '-Infinity', 'NaN', 'abc',
  '5s', '５', '   ', '0.0001', '1e-7', '  ', '0x', '0xg', '-0x10', '9007199254740993', '1e308', '+Infinity', 'infinity', '١',
  // 날짜(now 는 NOW_MS)
  'Sun, 06 Nov 1994 08:49:37 GMT', 'Wed, 21 Oct 2015 07:28:00 GMT', 'Mon, 21 Oct 2015 07:28:00 GMT', 'Thu, 01 Jan 1970 00:00:00 GMT', 'Fri, 31 Dec 9999 23:59:59 GMT',
  'Sat, 29 Feb 2020 12:00:00 GMT', 'Sun, 29 Feb 2023 12:00:00 GMT', 'Mon, 31 Apr 2024 12:00:00 GMT', 'Tue, 01 Jan 2030 00:00:00 GMT', 'Tue, 01 Jan 2030 00:00:00.5 GMT',
  'Tue, 01 Jan 2030 00:00:01 GMT', 'Tue, 01 Jan 2030 00:00:00 UTC', 'Tue, 01 Jan 2030 00:00:00 +0000', 'Tue, 01 Jan 2030 00:00:00', 'Tue, 1 Jan 2030 00:00:00 GMT', 'Tue, 01 Jan 30 00:00:00 GMT',
  'Tue, 01 Jan 2030 24:00:00 GMT', 'Tue, 01 Jan 2030 23:60:00 GMT', 'Tue, 01 Jan 2030 23:59:60 GMT', 'Tue, 01 Jan 2030 3:00:00 GMT', 'Tue, 01 jan 2030 00:00:00 GMT', 'TUE, 01 JAN 2030 00:00:00 GMT',
  'Foo, 01 Jan 2030 00:00:00 GMT', 'Tue 01 Jan 2030 00:00:00 GMT', 'Tue,  01 Jan 2030 00:00:00 GMT', 'Tue, 01 Jan 2030  00:00:00 GMT', ' Tue, 01 Jan 2030 00:00:00 GMT', 'Tue, 01 Jan 2030 00:00:00 GMT ',
  'Tue, 01 Jan 0001 00:00:00 GMT', 'Tue, 01 Jan 0099 00:00:00 GMT', 'Tue, 01 Jan 1969 23:59:59 GMT',
  'Sunday, 06-Nov-94 08:49:37 GMT', 'Sunday, 06-Nov-1994 08:49:37 GMT', 'Wednesday, 21-Oct-15 07:28:00 GMT', 'Tuesday, 01-Jan-30 00:00:00 GMT',
  'Tuesday, 01-Jan-48 00:00:00 GMT', 'Tuesday, 01-Jan-49 00:00:00 GMT', 'Tuesday, 01-Jan-50 00:00:00 GMT', 'Tuesday, 01-Jan-51 00:00:00 GMT', 'Tuesday, 01-Jan-69 00:00:00 GMT', 'Tuesday, 01-Jan-70 00:00:00 GMT',
  'Tuesday, 01-Jan-99 00:00:00 GMT', 'Tuesday, 01-Jan-00 00:00:00 GMT', 'Tuesday, 01-Jan-0 00:00:00 GMT', 'Tuesday, 01-Jan-100 00:00:00 GMT', 'Tuesday, 32-Jan-30 00:00:00 GMT', 'Tuesday, 29-Feb-00 00:00:00 GMT',
  'Tuesday, 29-Feb-100 00:00:00 GMT', 'Tue, 01-Jan-30 00:00:00 GMT', 'Tuesday, 01-Jan-30 00:00:00 UTC', 'Tuesday, 01-Jan-30 00:00:00', 'Tuesday 01-Jan-30 00:00:00 GMT',
  'Tue Jan  1 00:00:00 2030', 'Tue Jan 01 00:00:00 2030', 'Sun Nov  6 08:49:37 1994', 'Tue Jan  1 00:00:00 30', 'Tue Jan 32 00:00:00 2030',
  '2030-01-01T00:00:00Z', '2030-01-01T00:00:00+09:00', '2030-01-01', '2030/01/01', 'Jan 1, 2030', '1 January 2030', '12/31/2030', '2000', '-5', '-1e3', 'garbage', 'Tue, 01 Jan 2030 00:00:00 GMT garbage'
]
export const NOW_MS = 1_700_000_000_000

export const LINK_HEADERS: (string | null)[] = [
  null, '', '<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last"', '<https://x>; rel="last"', '<https://x>; rel="prev", <https://y>; rel="next"',
  '<https://x>;rel=next', '<https://x> ; rel = "next"', '<https://x>;   rel="next"', '<https://x>;\trel="next"', '<https://x>;\nrel="next"', '<https://x>;\u00a0rel="next"', '<https://x>;\u2028rel="next"', '<https://x>;\u0085rel="next"',
  '<https://x>; REL="NEXT"', '<https://x>; Rel="Next"', '<https://x>; rel="nextt"', '<https://x>; rel="next', '<https://x>; rel=next"', '<https://x>; rel=nex', '<https://x>; rel=', '<https://x>; rel="prev next"', '<https://x>; title="a"; rel="next"',
  '<https://x>; rel="ñext"', '<https://x>; rel="ne\u212Ant"', '<https://x>; rel="\u017Fnext"', '<>; rel="next"', '<; rel="next"', '>; rel="next"', '<a<b>; rel="next"', '<a>b>; rel="next"', '<a>; rel="last", <b; rel="next"',
  '<a>; rel="last"; <b>; rel="next"', 'x <a>; rel="next"', '<a> ; ; rel="next"', '<a>, rel="next"', '<a>; rel="next"<b>', '<a\n>; rel="next"', '<a>;rel="next"', 'rel="next"', '<a>; rel=\'next\'', '<a>; rel="next"' + ' '.repeat(5)
]

export const RATE_REMAINING: (string | null)[] = [null, '0', '5', '', ' ', 'abc', '-0', '0x0', '0.0', '00', ' 0 ', '1e0', '0e5', 'Infinity', '-1']
export const RATE_LIMIT: (string | null)[] = [null, '60', 'x', '', 'Infinity']
export const RATE_RESET: (string | null)[] = [null, '1700000000', 'abc', '1e20', '', '0', '-5', '1700000000.7', '8.64e15', '8.64e15']
export const RATE_RETRY: (string | null)[] = [null, '0', '30', '1.2', '-5', 'abc', '', '1e21', '0.0001', ' 7 ', 'Infinity', '0x1f', '1e400']

export const BODY_TEXTS: string[] = [
  '',
  '   ',
  'not json',
  'null',
  'true',
  '5',
  '"str"',
  '[]',
  '[1,2]',
  '{}',
  '{"message":"hi"}',
  '{"message":""}',
  '{"message":5}',
  '{"message":null}',
  '{"message":["x"]}',
  '{"message":"a","message":"b"}',
  '{"message":"한글 메시지 😀"}',
  `{"message":"${A('m', 199)}"}`,
  `{"message":"${A('m', 200)}"}`,
  `{"message":"${A('m', 201)}"}`,
  `{"message":"${A('m', 199)}😀x"}`,
  `{"message":"${A('m', 198)}😀x"}`,
  `{"message":"${A('m', 199)}\\ud83d\\ude00x"}`,
  '{"message":"lone \\ud83d end"}',
  '{"message":"lone \\ude00 end"}',
  '{"message":"\\ud83d\\ud83d\\ude00"}',
  '{"message":"\\ud83dx\\ude00"}',
  '{"message":"pair \\ud83d\\ude00"}',
  '{"message":"a\\u0000b"}',
  '{"message":"a\\nb\\tc"}',
  '{"message":" \\n lead and trail \\r\\n "}',
  '{"message":"a \\u00a0\\u2028\\ufeff b"}',
  '{"message":"a\\u0085b"}',
  '{"message":"a\\u007fb\\u001fc"}',
  '{"message":"x\\u0001\\u0002y"}',
  '{"message":"\\n\\n"}',
  '{"message":"\\u00a0"}',
  '{"errorMessages":["first","second"],"message":"m"}',
  '{"errorMessages":[],"message":"m"}',
  '{"errorMessages":[5],"message":"m"}',
  '{"errorMessages":[""],"message":"m"}',
  '{"errorMessages":[""],"errors":{"a":"b"}}',
  '{"errorMessages":"x","message":"m"}',
  '{"errorMessages":{"0":"x"},"message":"m"}',
  '{"errorMessages":[null,"x"],"message":"m"}',
  '{"errors":{"summary":"required","priority":"bad"}}',
  '{"errors":{"b":"1","a":"2"}}',
  '{"errors":{"z":"z","10":"ten","2":"two","a":"a","01":"zero-one","4294967295":"big","4294967294":"max"}}',
  '{"errors":{"__proto__":"p","x":"y"}}',
  '{"errors":{"a":5,"b":"x","c":null,"d":["y"],"e":"z"}}',
  '{"errors":{}}',
  '{"errors":[]}',
  '{"errors":["a","b",5]}',
  '{"errors":"text"}',
  '{"errors":null}',
  '{"errors":5}',
  '{"errors":{"a":"b"},"message":"m"}',
  '{"errors":{"a":"b"},"message":""}',
  '{"errors":{"a":"b"},"message":5}',
  '{"errors":{"a":"b"},"errorMessages":[]}',
  '{"errors":{"a":"line1\\nline2","b":"  c  "}}',
  `{"errors":{"a":"${A('e', 150)}","b":"${A('f', 150)}"}}`,
  '{"errors":{"a":"x","a":"y"}}',
  ' {"message":"padded"}\n',
  '\t{"message":"tab"}',
  '\ufeff{"message":"bom"}',
  '{"message":"a"} x',
  '{"message":"a",}',
  '{"message":"a"',
  "{'message':'a'}",
  '{"message":01}',
  '{"message":"a","n":1e999}',
  '{"message":"a","n":-1e999}',
  '{"message":"a","n":1.}',
  '{"message":"a","n":.5}',
  '{"message":"a","n":+1}',
  '{"message":"a","n":-}',
  '{"message":"a","n":1e}',
  '{"message":"a","n":-0}',
  '{"message":"a","n":NaN}',
  '{"message":"tab\there"}',
  '{"message":"raw\nnewline"}',
  '{"message":"bad \\x escape"}',
  '{"message":"bad \\u12 escape"}',
  '{"message":"bad \\uZZZZ escape"}',
  '{"message":"slash \\/ ok \\b\\f ok"}',
  '{"mess\\u0061ge":"escaped key"}',
  '{"message":"\\ud83d"}',
  '{"message":"\\ude00"}',
  '{"message":"\\ud83d\\u0041"}',
  '{"message":"\\ud83d\\ud83d"}',
  '{"message":"\\ud83d\\\\ude00"}',
  '{"message":"\\uD83D\\uDE00"}',
  '[{"message":"in array"}]',
  '{"a":{"b":{"c":[1,2,{"message":"deep"}]}},"message":"top"}',
  '{"message":"x","nested":' + '['.repeat(30) + ']'.repeat(30) + '}'
]

/** 깊게 중첩된 텍스트(골든에는 명세만 적는다): `pre + open×depth + inner + close×depth + post`. Rust 의 JSON 중첩 상한(MAX_DEPTH=64 → 배열 65겹까지)을 넘으면 알려진 차이다 */
export interface Nest {
  pre: string
  open: string
  inner: string
  close: string
  post: string
  depth: number
}
export const nestText = (n: Nest): string => n.pre + n.open.repeat(n.depth) + n.inner + n.close.repeat(n.depth) + n.post
export const NEST_CASES: Nest[] = [
  ...[1, 10, 62, 63, 64, 65, 66, 100, 512, 1000, 5000, 100000].flatMap((depth): Nest[] => [
    { pre: '', open: '[', inner: '', close: ']', post: '', depth },
    { pre: '', open: '{"a":', inner: '1', close: '}', post: '', depth },
    { pre: '{"message":"x","errorMessages":["y"],"n":', open: '[', inner: '', close: ']', post: '}', depth },
    { pre: '{"errors":{"k":"v"},"n":', open: '[', inner: '', close: ']', post: '}', depth }
  ])
]

export const STATUSES_JIRA = [200, 301, 400, 401, 403, 404, 408, 409, 418, 422, 429, 500, 502, 503, 599]
export const STATUSES_GH = [200, 301, 400, 401, 403, 404, 409, 422, 429, 451, 500, 502, 503, 599]
export const NOT_FOUNDS: (string | null)[] = [null, '이슈를 찾을 수 없습니다', '']

// ── Device Flow ───────────────────────────────────────────────────────────

export interface DeviceStartCase {
  clientId: string
  status: number
  text: string
}
const DC = { device_code: 'dc123', user_code: 'ABCD-1234', verification_uri: 'https://github.com/login/device', expires_in: 900, interval: 5 }
const j = (o: unknown): string => JSON.stringify(o)
export const DEVICE_STARTS: DeviceStartCase[] = [
  { clientId: 'cid', status: 200, text: j(DC) },
  { clientId: '', status: 200, text: j(DC) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: 0 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: 0.5 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: 1 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: 7.5 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: -3 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: null }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, interval: undefined }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, expires_in: 0 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, expires_in: 1.5 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, expires_in: null }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, expires_in: undefined }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, expires_in: 1e20 }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, device_code: '' }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, device_code: undefined }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, user_code: '' }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, user_code: undefined }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, verification_uri: '' }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, verification_uri: undefined }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, verification_uri: undefined, error_description: '이유' }) },
  { clientId: 'cid', status: 200, text: j({ ...DC, verification_uri: undefined, error_description: '' }) },
  { clientId: 'cid', status: 200, text: j({ error: 'device_flow_disabled' }) },
  { clientId: 'cid', status: 200, text: j({ error: 'incorrect_client_credentials' }) },
  { clientId: 'cid', status: 200, text: j({ error: 'unauthorized_client' }) },
  { clientId: 'cid', status: 200, text: j({ error: 'other', error_description: 'desc' }) },
  { clientId: 'cid', status: 200, text: j({ error: 'device_flow_disabled', ...DC }) },
  { clientId: 'cid', status: 200, text: j({ error: 'other', ...DC }) },
  { clientId: 'cid', status: 200, text: j({ error: 5, error_description: 'x' }) },
  { clientId: 'cid', status: 200, text: '{}' },
  { clientId: 'cid', status: 200, text: '[]' },
  { clientId: 'cid', status: 200, text: '"str"' },
  { clientId: 'cid', status: 200, text: '5' },
  { clientId: 'cid', status: 200, text: 'true' },
  { clientId: 'cid', status: 200, text: 'null' },
  { clientId: 'cid', status: 200, text: '' },
  { clientId: 'cid', status: 200, text: 'not json' },
  { clientId: 'cid', status: 200, text: '<html>x</html>' },
  { clientId: 'cid', status: 500, text: j(DC) },
  { clientId: 'cid', status: 502, text: 'bad gateway' },
  { clientId: 'cid', status: 599, text: '' },
  { clientId: 'cid', status: 499, text: j(DC) },
  { clientId: 'cid', status: 404, text: j({ error: 'x' }) },
  { clientId: 'cid', status: 401, text: '{"message":"Bad credentials"}' },
  { clientId: 'cid', status: 200, text: '{"device_code":"d\\ud83d","user_code":"u","verification_uri":"v"}' }
]

/** 폴링 한 단계: 응답(상태·본문) 또는 네트워크 실패 */
export type PollStepCase = { status: number; text: string } | { net: true }
export interface DevicePollCase {
  interval: number
  /** 세션 만료까지(초) */
  expiresIn: number
  steps: PollStepCase[]
}
const tok = (t: string): PollStepCase => ({ status: 200, text: j({ access_token: t, token_type: 'bearer' }) })
const err = (e: string, extra: Record<string, unknown> = {}): PollStepCase => ({ status: 200, text: j({ error: e, ...extra }) })
const pending = err('authorization_pending')
export const DEVICE_POLLS: DevicePollCase[] = [
  { interval: 5, expiresIn: 900, steps: [tok('gho_a')] },
  { interval: 5, expiresIn: 900, steps: [pending, pending, tok('gho_b')] },
  { interval: 5, expiresIn: 900, steps: [err('slow_down'), pending, tok('gho_c')] },
  { interval: 5, expiresIn: 900, steps: [err('slow_down', { interval: 20 }), err('slow_down', { interval: 20 }), err('slow_down', { interval: 3 }), tok('gho_d')] },
  { interval: 5, expiresIn: 900, steps: [err('slow_down', { interval: 0 }), err('slow_down', { interval: null }), tok('gho_e')] },
  { interval: 5, expiresIn: 900, steps: [err('slow_down', { interval: 5 }), err('slow_down', { interval: 5.5 }), tok('gho_f')] },
  { interval: 1, expiresIn: 900, steps: [{ net: true }, { net: true }, { net: true }, tok('gho_g')] },
  { interval: 1, expiresIn: 900, steps: Array.from({ length: 14 }, (): PollStepCase => ({ status: 503, text: '' })).concat([tok('gho_h')]) },
  { interval: 5, expiresIn: 900, steps: [{ status: 500, text: 'x' }, { status: 200, text: 'not json' }, { net: true }, pending, { status: 502, text: '' }, tok('gho_i')] },
  { interval: 5, expiresIn: 900, steps: [{ net: true }, { net: true }, pending, { net: true }, tok('gho_j')] },
  { interval: 5, expiresIn: 900, steps: [err('expired_token')] },
  { interval: 5, expiresIn: 900, steps: [pending, err('access_denied')] },
  { interval: 5, expiresIn: 900, steps: [err('bad_verification_code', { error_description: '설명' })] },
  { interval: 5, expiresIn: 900, steps: [err('bad_verification_code', { error_description: '' })] },
  { interval: 5, expiresIn: 900, steps: [err('bad_verification_code')] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: '{}' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: '[]' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: '"str"' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: '7' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: 'null' }] },
  { interval: 5, expiresIn: 900, steps: [pending, { status: 200, text: 'null' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: j({ access_token: '', error: 'authorization_pending' }) }, tok('gho_k')] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: j({ access_token: 'gho_l', error: 'access_denied' }) }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 400, text: j({ error: 'authorization_pending' }) }, { status: 403, text: j({ error: 'access_denied' }) }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 429, text: j({ error: 'slow_down' }) }, tok('gho_m')] },
  { interval: 5, expiresIn: 12, steps: [pending, pending, pending, pending] },
  { interval: 5, expiresIn: 10, steps: [pending, pending] },
  { interval: 5, expiresIn: 10.5, steps: [pending, pending, pending] },
  { interval: 5, expiresIn: 5, steps: [tok('gho_n')] },
  { interval: 5, expiresIn: 5.001, steps: [tok('gho_o')] },
  { interval: 5, expiresIn: 0, steps: [tok('gho_p')] },
  { interval: 1, expiresIn: 30, steps: Array.from({ length: 20 }, (): PollStepCase => ({ net: true })) },
  { interval: 1, expiresIn: 30, steps: [err('slow_down'), err('slow_down'), err('slow_down'), err('slow_down'), err('slow_down'), err('slow_down')] },
  { interval: 2.5, expiresIn: 900, steps: [pending, err('slow_down'), tok('gho_q')] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: '{"access_token":"gho_\\ud83d"}' }] },
  { interval: 5, expiresIn: 900, steps: [{ status: 200, text: j({ error: 'x', error_description: 'a\nb' }) }] }
]

// ── ghEnv ─────────────────────────────────────────────────────────────────

export type EnvPairs = [string, string][]
export const ENV_CASES: EnvPairs[] = [
  [],
  [['PATH', '/bin'], ['HOME', '/h'], ['SECRET', 'x'], ['GH_TOKEN', 't'], ['GITHUB_TOKEN', 't2']],
  [['Path', 'C:\\a'], ['PATH', 'C:\\b'], ['path', 'C:\\c']],
  [['PATH', '1'], ['Path', '2']],
  [['Path', '2'], ['PATH', '1']],
  [['SystemRoot', 'C:\\Windows'], ['SYSTEMROOT', 'X'], ['ComSpec', 'cmd'], ['COMSPEC', 'cmd2']],
  [['TEMP', 't'], ['TMP', 'm'], ['APPDATA', 'a'], ['LOCALAPPDATA', 'l'], ['USERPROFILE', 'u'], ['PATHEXT', '.EXE']],
  [['XDG_CONFIG_HOME', 'c'], ['XDG_DATA_HOME', 'd'], ['XDG_STATE_HOME', 's'], ['XDG_CACHE_HOME', 'k'], ['GH_CONFIG_DIR', 'g'], ['GH_HOST', 'h'], ['GH_ENTERPRISE_TOKEN', 'e']],
  [['GH_PROMPT_DISABLED', '0'], ['NO_COLOR', '0'], ['PATH', 'p']],
  [['gh_host', 'x'], ['Gh_Config_Dir', 'y'], ['home', 'z'], ['Home', 'w']],
  [['PATH', ''], ['HOME', '']],
  [['ＰＡＴＨ', 'full'], ['PATH ', 'space'], [' PATH', 'space'], ['PATHS', 'x'], ['PAT', 'x']],
  [['İ', 'x'], ['ı', 'y'], ['K', 'kelvin'], ['ſ', 'long s'], ['PATH', 'ok']],
  [['SİSTEMROOT', 'x'], ['SYSTEMROOT', 'ok']],
  [['1', 'a'], ['2', 'b'], ['PATH', 'p'], ['10', 'c']],
  [['HOME', '/h'], ['', 'empty-name'], ['=C:', 'drive'], ['PATH', '/p']]
]

// ── 토큰 가리기 ───────────────────────────────────────────────────────────

export const USABLE_TOKENS: string[] = [
  'ghp_abc', 'a', '', ' ', 'a b', 'a\tb', 'a\nb', 'a\rb', 'a\0b', '\0', 'a\u00a0b', 'a\u2028b', 'a\ufeffb', 'a\u0085b', 'a\u200bb', 'a\u180eb', '한글', '😀', A('a', 4096), A('a', 4097), '😀'.repeat(2048), '😀'.repeat(2049), 'a\x7fb', 'a\x1fb', 'a\vb', 'a\fb', 'a\u3000b'
]
export interface RedactCase {
  text: string
  token: string | null
}
const B64 = (t: string): string => Buffer.from(`x-access-token:${t}`).toString('base64')
export const REDACT_CASES: RedactCase[] = [
  { text: 'error ghp_abc here', token: 'ghp_abc' },
  { text: 'ghp_abc ghp_abc ghp_abc', token: 'ghp_abc' },
  { text: `header ${B64('ghp_abc')} end`, token: 'ghp_abc' },
  { text: `both ghp_abc and ${B64('ghp_abc')}`, token: 'ghp_abc' },
  { text: 'nothing', token: 'ghp_abc' },
  { text: 'ghp_abc', token: null },
  { text: 'ghp_abc', token: '' },
  { text: 'ghp_abc x', token: 'ghp_abc x' },
  { text: 'a b', token: 'a b' },
  { text: 'tok\ntok', token: 'tok\ntok' },
  { text: `${B64('한글')} 한글`, token: '한글' },
  { text: `${B64('😀')} 😀 x`, token: '😀' },
  { text: 'aaa', token: 'a' },
  { text: 'abab', token: 'ab' },
  { text: '', token: 'x' },
  { text: 'x', token: A('y', 4097) },
  { text: `${A('y', 4096)} z`, token: A('y', 4096) },
  { text: `${B64('a')}`, token: 'a' },
  { text: `${B64('tok')}tok`, token: 'tok' },
  { text: '***', token: '*' },
  { text: 'tok-ghp_abc-tok', token: 'tok' }
]

export const AUTH_CASES: { kind: 'cloud' | 'server'; email: string; token: string }[] = [
  { kind: 'cloud', email: 'me@x.y', token: 'tok' },
  { kind: 'cloud', email: '', token: 'tok' },
  { kind: 'cloud', email: '한글@x.y', token: '토큰' },
  { kind: 'cloud', email: 'a:b', token: 'c:d' },
  { kind: 'cloud', email: 'é😀', token: 'x' },
  { kind: 'server', email: '', token: 'pat' },
  { kind: 'server', email: 'ignored@x.y', token: 'pat' },
  { kind: 'server', email: '', token: '' },
  { kind: 'cloud', email: '', token: '' }
]

// ── JSON 해석 ─────────────────────────────────────────────────────────────

/** `JSON.parse` 동작을 비교할 텍스트(BODY_TEXTS 와 합쳐 쓴다) */
export const JSON_EXTRA: string[] = [
  '0', '-0', '1', '-1', '1.5', '1e3', '1E3', '1e+3', '1e-3', '0.1', '123456789012345678901234567890', '1e999', '-1e999', '4.9e-324', '1e-999', '0e0', '0.0',
  '"\\u0041\\u00e9\\u4e2d"', '"\\ud83d\\ude00"', '"\\ud83d"', '"\\ude00\\ud83d"', '"\\ud83d\\ud83d\\ude00"', '"\\ud83d\\\\ude00"', '"a\\"b"', '"\\\\"', '""',
  '{"a":1,"a":2}', '{"a":1,"b":2,"a":3}', '{"b":1,"a":2}', '{"10":1,"9":2,"a":3,"1":4}', '{"__proto__":1}', '{"":1}', '{"\\u0000":1}',
  '[1,2,3]', '[ 1 , 2 ]', '[,]', '[1,]', '[1 2]', '[', ']', '{', '}', '{"a"}', '{"a":}', '{a:1}', '{"a":1 "b":2}',
  'true', 'false', 'null', 'True', 'nul', 'tru', 'nulll', 'undefined', 'NaN', 'Infinity', '-Infinity',
  '"tab\there"', '"line\nbreak"', '"del\u007f"', '"\u2028"', '"\u0000"', '\u00a05', '5\u00a0', '5 ', ' 5', '\n5\n', '\v5', '\f5',
  '01', '00', '-01', '1.e3', '+1', '--1', '0x10', '1_000',
  '{"a":[1,{"b":[2,{"c":null}]}],"d":{"e":"f"}}',
  '[' + '['.repeat(30) + ']'.repeat(30) + ']',
  '{"a":'.repeat(30) + '1' + '}'.repeat(30)
]
