// 업데이트 판정(src/main/update/{logic,releaseNotes}.ts)의 입력 표. tests/goldenUpdate.test.ts 가 TS 구현으로 골든(tests/golden/update/*.json)을 만들고,
// Rust 이식(src-tauri/src/update/logic.rs, 검사: src-tauri/tests/update.rs)이 같은 입력에서 같은 결과를 내는지 본다.
// 이름이 기존 TS 테스트(tests/update.test.ts)의 `it` 제목과 같은 사례는 그 `it` 의 단언을 옮긴 것이다(대응표 tests/golden/update/test-map.json).

export const BASE = 'https://github.com/Kang-Jacob-GitLB/antler/releases/download'

// ── semver ───────────────────────────────────────────────────────────

/** parseSemver 입력(문자열만 — 비문자열은 Rust 의 타입이 막는다. IPC 의 비문자열은 DISMISS 가 본다) */
export const SEMVER_INPUTS: string[] = [
  '1.2.3',
  'v1.2.3',
  'V1.2.3',
  'vv1.2.3',
  ' 1.2.3 ',
  '\t1.2.3\n',
  '\ufeff1.2.3',
  '1.2.3\u00a0',
  '1.2.3\u0085',
  '\u20281.2.3',
  'v 1.2.3',
  '1.2.3-rc.1',
  '1.2.3-rc.1+build.5',
  '1.2.3+b',
  '1.2.3+build-1.2',
  '1.2.3+',
  '1.2.3-',
  '1.2.3-.a',
  '1.2.3-a..b',
  '1.2.3-a.',
  '1.2.3-a_b',
  '1.2.3-01',
  '1.2.3-0',
  '1.2.3-a-b',
  '1.0.0-0.3.7',
  '1.0.0-x.7.z.92',
  '1.0.0-x-y-z.--',
  '01.2.3',
  '1.02.3',
  '1.2.03',
  '0.0.0',
  '1.2',
  '1.2.3.4',
  '1.2.',
  '1..3',
  '',
  'x',
  'abc',
  '../../etc',
  '; rm -rf',
  '１.2.3',
  '1.2.3-٣',
  '99999999999999999999.0.0',
  '1.99999999999999999999.0',
  '1.0.0-99999999999999999999',
  `1.2.3-${'a'.repeat(58)}`,
  `1.2.3-${'a'.repeat(59)}`,
  `${' '.repeat(60)}1.2.3`,
  `${' '.repeat(70)}1.2.3`
]

/** 비교·알림 대상 판정에 쓰는 버전 목록(모든 쌍) */
export const VERSION_LIST: string[] = [
  '0.1.0',
  '0.2.0',
  'v0.1.0',
  '1.0.0',
  '1.0.0+a',
  '1.0.0+b',
  '1.0.0-alpha',
  '1.0.0-alpha.1',
  '1.0.0-alpha.beta',
  '1.0.0-beta',
  '1.0.0-beta.2',
  '1.0.0-beta.11',
  '1.0.0-rc.1',
  '1.0.0-rc.2',
  '1.0.0-rc.10',
  '1.0.0-1',
  '1.0.0-01',
  '1.0.0-a-b',
  '1.0.0-Beta',
  '1.0.0-99999999999999999998',
  '1.0.0-99999999999999999999',
  '1.9.9',
  '1.10.0',
  '2.0.0',
  '99999999999999999998.0.0',
  '99999999999999999999.0.0',
  'abc',
  '',
  '1.2'
]

// ── URL ──────────────────────────────────────────────────────────────

const ASSET = `${BASE}/v0.2.0/Antler-0.2.0-arm64.dmg`

export const ALLOWED_URLS: string[] = [
  'https://github.com/a/b',
  'https://objects.githubusercontent.com/x?y=1',
  'https://release-assets.githubusercontent.com/github-production-release-asset/1/2?sig=3',
  'http://github.com/a',
  'https://evil.com/a',
  'https://github.com.evil.com/a',
  'https://evilgithub.com/a',
  'https://github.com./a',
  'https://GITHUB.COM/a',
  'https://user:pw@github.com/a',
  'https://user@github.com/a',
  'https://:pw@github.com/a',
  'https://github.com:8443/a',
  'https://github.com:443/a',
  'https://github.com:/a',
  'not a url',
  '',
  '/relative/path',
  '//github.com/a',
  'https:github.com/a',
  'https:/github.com/a',
  'https:\\\\github.com\\a',
  'HTTPS://github.com/a',
  'ftp://github.com/a',
  'https://github.com\\@evil.com/',
  'https://github.com%2eevil.com/',
  'https://github.com#@evil.com/',
  'https://[::1]/a',
  'https://127.0.0.1/a',
  'https://gıthub.com/a',
  'https://objects.githubusercontent.com.evil.com/',
  ' https://github.com/a',
  'https://github.com/a b',
  'https://github.com/\t/a'
]

export const ASSET_URLS: string[] = [
  `${BASE}/v0.2.0/a.dmg`,
  ASSET,
  'https://github.com/other/repo/releases/download/v1/a.dmg',
  'https://github.com/kang-jacob-gitlb/antler/releases/download/v1/a.dmg',
  `${BASE}/../../../../evil/repo/releases/download/v9.9.9/x.dmg`,
  `${BASE}/v1/%2e%2e/%2e%2e/x.dmg`,
  `${BASE}/v1/..%2f..%2fx.dmg`,
  `${BASE}/v1/..%2F..%2Fx.dmg`,
  `${BASE}/v1/a%5cb.dmg`,
  `${BASE}/v1/a%5Cb.dmg`,
  `${BASE}/v1/a\\b.dmg`,
  `${BASE}/v1/./a.dmg`,
  `${BASE}/v1/../v1/a.dmg`,
  `${BASE}/v1/a.dmg?x=1`,
  `${BASE}/v1/a.dmg?`,
  `${BASE}/v1/a.dmg#`,
  `${BASE}/v1/a.dmg#x`,
  `${BASE}/v1/a b.dmg`,
  `${BASE}/v1/a%20b.dmg`,
  `${BASE}/v1/a"b.dmg`,
  `${BASE}/v1/a<b>.dmg`,
  `${BASE}/v1/a{b}|c^d[e]'f.dmg`,
  `${BASE}/v1/a\`b.dmg`,
  `${BASE}/v1/한글.dmg`,
  `${BASE}/v1/%zz.dmg`,
  `${BASE}/v1/a%2Fb.dmg`,
  `${BASE}/v1/a\tb.dmg`,
  `${BASE}/v1/a\nb.dmg`,
  `${BASE}//v1/a.dmg`,
  `${BASE}v1/a.dmg`,
  'https://GITHUB.com/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg',
  'https://github.com:443/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg',
  'https://github.com:8443/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg',
  'http://github.com/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg',
  'https://user@github.com/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg',
  'https://github.com/Kang-Jacob-GitLB/antler/releases/download',
  'https://github.com/Kang-Jacob-GitLB/antler/releases/download/',
  'https://github.com/Kang-Jacob-GitLB/antler/releases/latest/download/latest.json',
  'https://github.com/Kang-Jacob-GitLB/antler/releases/download/v1/latest.json',
  ` ${ASSET}`,
  `${ASSET} `,
  'https://objects.githubusercontent.com/Kang-Jacob-GitLB/antler/releases/download/v1/a.dmg'
]

export const NOTES_URL_VERSIONS: string[] = ['0.2.0', 'v0.2.0', 'vv1', '1.0.0-rc.1', '']

export const SAFE_NAMES: string[] = [
  'Antler-Setup-2.0.0-x64.exe',
  'latest.json',
  'a',
  '.hidden',
  '-dash',
  'a..b',
  '..',
  'a/b',
  'a\\b',
  'a b',
  'a%20b',
  '한글.exe',
  '',
  `a${'b'.repeat(200)}`,
  `a${'b'.repeat(201)}`,
  'a\nb',
  'a\n'
]

// ── 나중에 / 건너뛰기 ────────────────────────────────────────────────

export type PolicyStep =
  | { op: 'snooze'; version: string }
  | { op: 'skip'; value: string | null }
  | { op: 'restart' }
  | { op: 'notify'; version: string; manual: boolean }

export interface PolicyScenario {
  name: string
  steps: PolicyStep[]
}

const notify = (version: string, manual = false): PolicyStep => ({ op: 'notify', version, manual })

export const POLICY_SCENARIOS: PolicyScenario[] = [
  {
    name: '자동 확인은 나중에·건너뛴 버전을 숨기고 수동 확인은 항상 보인다',
    steps: [
      notify('0.2.0'),
      { op: 'snooze', version: '0.2.0' },
      notify('0.2.0'),
      notify('0.2.0', true),
      notify('0.2.1'),
      // 나중에는 메모리에만 있다 — 앱을 다시 시작(새 정책 인스턴스)하면 풀린다
      { op: 'restart' },
      notify('0.2.0'),
      { op: 'skip', value: '0.3.0' },
      notify('0.3.0'),
      notify('0.2.5'),
      notify('0.3.1'),
      notify('0.3.0', true)
    ]
  },
  {
    name: '건너뛴 값이 해석되지 않으면 자동 확인은 모두 숨긴다',
    steps: [{ op: 'skip', value: 'garbage' }, notify('0.2.0'), notify('99.0.0'), notify('0.2.0', true)]
  },
  {
    name: '건너뛴 값이 빈 문자열이거나 없으면 숨기지 않는다',
    steps: [{ op: 'skip', value: '' }, notify('0.2.0'), { op: 'skip', value: null }, notify('0.2.0')]
  },
  {
    name: '건너뛴 프리릴리스와 정식의 순서',
    steps: [{ op: 'skip', value: '1.0.0-rc.2' }, notify('1.0.0-rc.1'), notify('1.0.0-rc.2'), notify('1.0.0-rc.10'), notify('1.0.0')]
  },
  {
    name: 'v 접두사와 나중에는 문자열이 같을 때만 숨긴다',
    steps: [{ op: 'snooze', version: '0.2.0' }, notify('v0.2.0'), notify('0.2.0'), { op: 'skip', value: 'v0.2.0' }, notify('0.2.0'), notify('0.2.1')]
  }
]

// ── IPC 입력 검증 ────────────────────────────────────────────────────

export const DISMISS_CASES: { name: string; action: unknown; version: unknown }[] = [
  { name: 'later + v 접두사', action: 'later', version: 'v0.2.0' },
  { name: 'skip + 프리릴리스', action: 'skip', version: '1.0.0-rc.1' },
  { name: '알 수 없는 동작', action: 'bogus', version: '0.2.0' },
  { name: '버전이 경로', action: 'skip', version: '../../etc' },
  { name: '버전이 숫자', action: 'skip', version: 5 },
  { name: '둘 다 없음', action: null, version: null },
  { name: '동작이 숫자', action: 1, version: '0.2.0' },
  { name: '동작이 배열', action: ['skip'], version: '0.2.0' },
  { name: '동작 대소문자', action: 'Skip', version: '0.2.0' },
  { name: '버전이 객체', action: 'skip', version: {} },
  { name: '버전이 불리언', action: 'later', version: true },
  { name: '버전 앞뒤 공백은 그대로 돌려준다', action: 'skip', version: ' 0.2.0 ' },
  { name: '공백 뒤 v 는 떼지 않는다', action: 'skip', version: ' v0.2.0' },
  { name: '빌드 메타데이터', action: 'later', version: '1.0.0+abc' },
  { name: '명령 주입', action: 'skip', version: '; rm -rf' },
  { name: '빈 버전', action: 'later', version: '' },
  { name: '두 입력이 모두 틀리면 동작 오류가 먼저', action: 'x', version: 'y' }
]

export const SANITIZE_INPUTS: string[] = [
  'fail https://objects.githubusercontent.com/x?token=SECRET ghp_abcdefghijklmnopqrstuvwxyz0123 C:\\Users\\me\\a.dmg',
  'net::ERR_INTERNET_DISCONNECTED',
  'GET https://github.com/a/b?x=1#y failed',
  'http://example.com:8080/path?q=1 and https://u:p@host.example/x',
  'https://[ bad url',
  'https:// (empty)',
  'https://',
  'http://a',
  'httpx://a/b',
  'see HTTPS://github.com/a/b',
  'ghp_abcdefghijklmnopqrst',
  'ghp_abcdefghijklmnopqrs',
  'xghp_abcdefghijklmnopqrstuvwxyz',
  'x ghp_abcdefghijklmnopqrstuvwxyz y',
  'gho_abcdefghijklmnopqrstuvwxyz0123 ghu_abcdefghijklmnopqrstuvwxyz0123 ghs_abcdefghijklmnopqrstuvwxyz0123 ghr_abcdefghijklmnopqrstuvwxyz0123 ghx_abcdefghijklmnopqrstuvwxyz0123',
  'github_pat_abcdefghijklmnopqrstuvwxyz_0123',
  'github_pat_short',
  'ghp_abcdefghijklmnopqrstuvwxyz-tail',
  'token abc',
  'tokens',
  'token',
  'token ',
  'token=',
  'token:',
  'token: ',
  'token :',
  'token = abc def',
  'token:abc',
  'TOKEN=abc',
  'Token:   abc  rest',
  'mytoken abc',
  'bearer token x',
  'Authorization: Bearer abc.def.ghi',
  'authorization=basic dXNlcjpwYXNz',
  'bearer\tabc',
  'bearer\u00a0abc',
  'bearer\u0085abc',
  'token::abc',
  'token=:abc',
  'token \n abc',
  'token\nabc',
  'a token b token c',
  'C:\\Users\\me\\a.dmg',
  'C:\\',
  'c:\\x y',
  'D:\\path\\with spaces\\file',
  "C:\\a'b",
  'C:/not/backslash',
  '/home/user/file',
  '/Users/me/Library/x.log',
  '/tmp/a.sock',
  '/var/log/x',
  '/home/',
  '/home',
  '/etc/passwd',
  'in /tmp/x and /var/y end',
  'path "/home/me/x" quoted',
  '에러: 파일을 찾을 수 없습니다 C:\\Users\\나\\a.dmg',
  'first line\nsecond line with token abc',
  'first\r\nsecond',
  '\nleading newline',
  '',
  'a'.repeat(200),
  'a'.repeat(201),
  'a'.repeat(300),
  '가'.repeat(201),
  `${'a'.repeat(198)}😀`,
  `${'a'.repeat(200)}\nnext`,
  `${'a'.repeat(201)}\nnext`
]

// ── 매니페스트(latest.json) — TS 에는 짝이 없다 ──────────────────────

const SIG = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQpSVVE9PQ=='

function entry(version: string, over: Record<string, unknown> = {}): Record<string, unknown> {
  return { url: `${BASE}/v${version}/Antler-Setup-${version}-x64.exe`, signature: SIG, ...over }
}

function manifest(version: unknown, platforms: unknown, extra: Record<string, unknown> = {}): string {
  return JSON.stringify({ version, notes: 'hi', pub_date: '2026-01-01T00:00:00Z', platforms, ...extra })
}

export type ManifestExpect = 'offer' | 'none' | 'err'
export interface ManifestCase {
  name: string
  current: string
  /** 매니페스트 원문 */
  text: string
  expect: ManifestExpect
}

const nsis = (version: string, over: Record<string, unknown> = {}): Record<string, unknown> => ({ 'windows-x86_64-nsis': entry(version, over) })

export const MANIFEST_CASES: ManifestCase[] = [
  { name: '새 버전', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0')), expect: 'offer' },
  { name: 'v 접두사 버전', current: '1.0.0', text: manifest('v1.1.0', nsis('1.1.0')), expect: 'offer' },
  { name: '같은 버전', current: '1.1.0', text: manifest('1.1.0', nsis('1.1.0')), expect: 'none' },
  { name: '다운그레이드', current: '2.0.0', text: manifest('1.1.0', nsis('1.1.0')), expect: 'none' },
  { name: '다운그레이드는 플랫폼 항목이 틀려도 오류가 아니다', current: '2.0.0', text: manifest('1.1.0', {}), expect: 'none' },
  { name: '정식 사용자에게 프리릴리스는 숨긴다', current: '1.0.0', text: manifest('1.1.0-rc.1', nsis('1.1.0-rc.1')), expect: 'none' },
  { name: '프리릴리스 사용자에게 프리릴리스', current: '1.1.0-rc.1', text: manifest('1.1.0-rc.2', nsis('1.1.0-rc.2')), expect: 'offer' },
  { name: '프리릴리스 사용자에게 정식', current: '1.1.0-rc.1', text: manifest('1.1.0', nsis('1.1.0')), expect: 'offer' },
  { name: '일반 키로 내려간다(nsis 키가 없을 때)', current: '1.0.0', text: manifest('1.1.0', { 'windows-x86_64': entry('1.1.0') }), expect: 'offer' },
  {
    name: 'nsis 키가 있으면 틀려도 일반 키로 내려가지 않는다',
    current: '1.0.0',
    text: manifest('1.1.0', { 'windows-x86_64-nsis': entry('1.1.0', { url: 'https://evil.example/a.exe' }), 'windows-x86_64': entry('1.1.0') }),
    expect: 'err'
  },
  { name: '다른 플랫폼만 있다', current: '1.0.0', text: manifest('1.1.0', { 'linux-x86_64': entry('1.1.0') }), expect: 'err' },
  { name: '다른 저장소의 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: 'https://github.com/evil/repo/releases/download/v1.1.0/a.exe' })), expect: 'err' },
  { name: '다른 호스트의 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: 'https://evil.example/Kang-Jacob-GitLB/antler/releases/download/v1.1.0/a.exe' })), expect: 'err' },
  { name: 'http URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE.replace('https', 'http')}/v1.1.0/a.exe` })), expect: 'err' },
  { name: '다른 태그의 URL(태그 결속)', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v0.9.0/Antler-Setup-0.9.0-x64.exe` })), expect: 'err' },
  { name: '태그에 v 가 없는 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/1.1.0/a.exe` })), expect: 'err' },
  { name: '더 새 태그의 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v9.9.9/a.exe` })), expect: 'err' },
  { name: '태그 아래 하위 경로', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/sub/a.exe` })), expect: 'err' },
  { name: '경로 이탈', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/../v0.1.0/a.exe` })), expect: 'err' },
  { name: '인코딩된 점', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/%2e%2e/a.exe` })), expect: 'err' },
  { name: '쿼리가 붙은 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/a.exe?x=1` })), expect: 'err' },
  { name: '해시가 붙은 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/a.exe#x` })), expect: 'err' },
  { name: '자격 증명이 든 URL', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: 'https://u:p@github.com/Kang-Jacob-GitLB/antler/releases/download/v1.1.0/a.exe' })), expect: 'err' },
  { name: '파일 이름 없음', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: `${BASE}/v1.1.0/` })), expect: 'err' },
  { name: '빌드 메타데이터 버전', current: '1.0.0', text: manifest('1.1.0+b1', nsis('1.1.0+b1', { url: `${BASE}/v1.1.0+b1/Antler-Setup-1.1.0-x64.exe` })), expect: 'offer' },
  { name: 'URL 이 없다', current: '1.0.0', text: manifest('1.1.0', { 'windows-x86_64-nsis': { signature: SIG } }), expect: 'err' },
  { name: 'URL 이 숫자다', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { url: 5 })), expect: 'err' },
  { name: '서명이 없다', current: '1.0.0', text: manifest('1.1.0', { 'windows-x86_64-nsis': { url: entry('1.1.0')['url'] } }), expect: 'err' },
  { name: '서명이 비었다', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { signature: '' })), expect: 'err' },
  { name: '서명에 허용되지 않은 문자', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0', { signature: 'abc def' })), expect: 'err' },
  { name: '버전이 없다', current: '1.0.0', text: JSON.stringify({ platforms: nsis('1.1.0') }), expect: 'err' },
  { name: '버전이 숫자다', current: '1.0.0', text: manifest(2, nsis('2.0.0')), expect: 'err' },
  { name: '버전이 semver 가 아니다', current: '1.0.0', text: manifest('latest', nsis('1.1.0')), expect: 'err' },
  { name: '버전에 공백이 있다', current: '1.0.0', text: manifest(' 1.1.0', nsis('1.1.0')), expect: 'err' },
  { name: '버전에 개행이 있다', current: '1.0.0', text: manifest('1.1.0\n', nsis('1.1.0')), expect: 'err' },
  { name: '플랫폼 표가 없다', current: '1.0.0', text: JSON.stringify({ version: '1.1.0' }), expect: 'err' },
  { name: '플랫폼 표가 배열이다', current: '1.0.0', text: manifest('1.1.0', []), expect: 'err' },
  { name: '항목이 객체가 아니다', current: '1.0.0', text: manifest('1.1.0', { 'windows-x86_64-nsis': 'x' }), expect: 'err' },
  { name: '최상위가 배열이다', current: '1.0.0', text: '[]', expect: 'err' },
  { name: '최상위가 null 이다', current: '1.0.0', text: 'null', expect: 'err' },
  { name: 'JSON 이 아니다', current: '1.0.0', text: 'not json', expect: 'err' },
  { name: '빈 본문', current: '1.0.0', text: '', expect: 'err' },
  { name: '현재 버전을 해석할 수 없다', current: 'dev', text: manifest('1.1.0', nsis('1.1.0')), expect: 'none' },
  { name: '매니페스트의 notes 등 다른 필드는 무시한다', current: '1.0.0', text: manifest('1.1.0', nsis('1.1.0'), { notes: '<script>', extra: { a: 1 } }), expect: 'offer' },
]

// ── 릴리스 노트 ──────────────────────────────────────────────────────

export interface NoteRoute {
  status?: number
  /** 객체면 JSON 으로, 문자열이면 그대로 */
  body?: unknown
}
export interface NotesCase {
  name: string
  current: string
  /** API 경로(`/releases/…`) → 응답. 없는 경로는 404 */
  routes: Record<string, NoteRoute>
}

const rel = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
  tag_name: 'v1.2.3',
  name: 'Antler 1.2.3',
  body: '## 변경\n- 고침',
  html_url: 'https://evil.example/phish',
  published_at: '2026-10-15T09:23:46Z',
  draft: false,
  ...over
})

export const NOTES_CASES: NotesCase[] = [
  { name: '그 버전의 릴리스가 있다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel() } } },
  { name: 'v 접두사 현재 버전', current: 'v1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel() } } },
  { name: '태그가 없으면 최신 릴리스로 대신한다', current: '1.2.3', routes: { '/releases/latest': { body: rel({ tag_name: 'v1.2.4' }) } } },
  { name: '초안이면 최신 릴리스로 대신한다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ draft: true }) }, '/releases/latest': { body: rel({ tag_name: 'v1.2.4' }) } } },
  { name: 'draft 가 불리언이 아니면 초안이 아니다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ draft: 'true' }) } } },
  { name: '둘 다 없다', current: '1.2.3', routes: {} },
  { name: 'tag_name 이 문자열이 아니다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ tag_name: 5 }) }, '/releases/latest': { body: rel({ tag_name: null }) } } },
  { name: 'tag_name 이 문자열이 아니고 exact 인 응답', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ tag_name: 5 }) } } },
  { name: '태그 이름이 이상하면 링크는 릴리스 페이지', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ tag_name: 'v1 2/3' }) } } },
  { name: '버전에 + 가 있으면 경로에서 인코딩한다', current: '1.2.3+b', routes: { '/releases/tags/v1.2.3%2Bb': { body: rel({ tag_name: 'v1.2.3+b' }) } } },
  { name: '버전 문자가 이상하면 태그를 묻지 않는다', current: '1.2.3 rc', routes: { '/releases/latest': { body: rel() } } },
  { name: '버전이 비었으면 태그를 묻지 않는다', current: '', routes: { '/releases/latest': { body: rel() } } },
  { name: '버전이 64자를 넘으면 태그를 묻지 않는다', current: `1.2.3-${'a'.repeat(60)}`, routes: { '/releases/latest': { body: rel() } } },
  { name: '제목은 200자로 자른다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ name: 'n'.repeat(250) }) } } },
  { name: '빈 제목은 넣지 않는다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ name: '' }) } } },
  { name: '제목이 문자열이 아니다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ name: 7 }) } } },
  { name: '게시 시각이 날짜가 아니면 넣지 않는다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ published_at: 'yesterday-ish' }) } } },
  { name: '본문이 문자열이 아니면 빈 본문', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: rel({ body: null }) } } },
  { name: '태그 조회가 한도(403)에 걸린다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { status: 403 } } },
  { name: '최신 조회가 한도(429)에 걸린다', current: '1.2.3', routes: { '/releases/latest': { status: 429 } } },
  { name: '태그 조회가 서버 오류(500)', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { status: 500 } } },
  { name: '최신 조회가 서버 오류(502)', current: '1.2.3', routes: { '/releases/latest': { status: 502 } } },
  { name: '응답이 객체가 아니면(배열) 없는 것으로', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: [] }, '/releases/latest': { body: [] } } },
  { name: '응답이 null 이면 없는 것으로', current: '1.2.3', routes: { '/releases/latest': { body: 'null' } } },
  { name: '응답 본문이 비었다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { body: '' } } },
  { name: '3xx 응답은 실패다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { status: 301, body: rel() } } },
  { name: '2xx 의 다른 상태(204)도 성공으로 본다', current: '1.2.3', routes: { '/releases/tags/v1.2.3': { status: 204, body: '' } } }
]
