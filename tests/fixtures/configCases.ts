// ConfigStore(설정 저장소)·GitHub 링크 순수 함수의 골든 입력 표(Phase 6b PR 1). 검사하는 쪽은 tests/goldenConfig.test.ts(TS 가 기준)와
// src-tauri/tests/config.rs(Rust 이식). 이 표의 값은 모두 JSON 으로 직렬화되어 골든에 그대로 들어간다(Rust 가 골든만 읽는다).
//
// 경계값은 "받는 최대 / 거부하는 최소+1" 쌍으로 둔다. 길이는 JS `length`(UTF-16 코드 단위)라 astral 문자('😀' 는 2단위)로도 한 번씩 건드린다.
// 알려진 차이(짝 없는 서로게이트·astral 문자열 펼침)는 표에 넣지 않는다 — docs/tauri.md 의 Phase 6b PR 1 절.

export type Op =
  | { op: 'setUi'; patch: unknown }
  | { op: 'setEditorExePath'; path: string }
  | { op: 'setSkippedUpdateVersion'; version: string }
  | { op: 'addRepo'; input: unknown; source?: unknown }
  | { op: 'updateRepo'; id: string; patch: unknown }
  | { op: 'removeRepo'; id: string }
  | { op: 'setGithubDefaultBranch'; id: string; branch: string }
  | { op: 'setTrustedConfig'; id: string; fingerprint: string }
  | { op: 'setFetchApproval'; id: string; fingerprint: string | null }

export interface ConfigCase {
  name: string
  /** 설정 파일 내용(없으면 파일 없음 = 첫 실행) */
  file?: string
  /** 파일 내용을 바이트(16진)로 준다(잘못된 UTF-8) */
  fileHex?: string
  /** 설정 파일 자리에 폴더를 둔다(읽기 실패 = 손상이지만 백업은 없다) */
  fileIsDir?: boolean
  /** github 캐시 기준 폴더를 준다(고정 가상 경로 — 디스크에 닿지 않는다) */
  cacheDir?: boolean
  ops?: Op[]
}

/** 로드·정화 대상 파일을 만드는 도우미(들여쓰기 없는 JSON) */
const cfg = (o: unknown): string => JSON.stringify(o)
const LOCAL = { id: 'local', kind: 'local', label: 'Local' }
const WSL = { id: 'wsl:Ubuntu', kind: 'wsl', label: 'WSL · Ubuntu', distro: 'Ubuntu' }
const SSH = { id: 'ssh:box', kind: 'ssh', label: 'SSH · box', sshHost: 'box' }
const GH_SOURCE = { id: 'github', kind: 'github', label: 'GitHub' }
const UI = { theme: 'dark', locale: 'ko', paneWidths: [248, 380] }
const FP = 'a'.repeat(64)
const FP2 = 'b'.repeat(64)
const CACHE = 'C:\\golden\\cache'

const repo = (id: string, sourceId: string, path: string, name: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({ id, sourceId, path, name, ...extra })
const gh = (id: string, owner: string, name: string, extra: Record<string, unknown> = {}, ghExtra: Record<string, unknown> = {}): Record<string, unknown> =>
  repo(id, 'github', `${CACHE}\\${owner.toLowerCase()}\\${name.toLowerCase()}.git`, name, { github: { owner, repo: name, ...ghExtra }, ...extra })

const up = (patch: unknown): Op => ({ op: 'setUi', patch })
const bad = (c: string, patch: unknown): ConfigCase => ({ name: c, ops: [up(patch)] })

// ── 로드·정화 ───────────────────────────────────────────────────────────────────────────────

const LOAD: ConfigCase[] = [
  { name: '로드: 파일 없음(첫 실행)은 기본값이고 손상이 아니다', ops: [up({ diffMode: 'split' })] },
  { name: '로드: 해석 실패는 손상 — 백업을 남기고 기본값(이후 쓰기는 된다)', file: '{', ops: [{ op: 'addRepo', input: { sourceId: 'local', path: 'C:\\r', name: 'r' } }] },
  { name: '로드: 빈 파일은 손상', file: '' },
  { name: '로드: 최상위 null 은 속성 읽기에서 던지므로 손상', file: 'null' },
  { name: '로드: BOM 이 붙은 파일은 해석 실패라 손상', file: `\uFEFF${cfg({ version: 1 })}` },
  { name: '로드: 최상위가 숫자·문자열·배열이면 던지지 않고 기본값(손상 아님)', file: '5' },
  { name: '로드: 최상위 문자열', file: '"abc"' },
  { name: '로드: 최상위 배열', file: '[1,2]' },
  { name: '로드: 설정 자리에 폴더가 있으면 손상(백업 없음)', fileIsDir: true },
  { name: '로드: 잘못된 UTF-8 은 U+FFFD 로 바뀌고 오류가 아니다', fileHex: Buffer.from(`{"ui":{"theme":"`).toString('hex') + 'ff' + Buffer.from(`"}}`).toString('hex'), ops: [up({ diffMode: 'split' })] },
  { name: '로드: repos 가 숫자·객체면 반복할 수 없어 손상', file: cfg({ repos: 5 }) },
  { name: '로드: repos 가 객체이면 손상', file: cfg({ repos: { a: 1 } }) },
  { name: '로드: repos 가 문자열이면 글자를 훑어 모두 건너뛴다(손상 아님)', file: cfg({ repos: 'abc' }) },
  { name: '로드: repos 가 null 이면 빈 목록', file: cfg({ repos: null }) },
  { name: '로드: repos 의 null·숫자·문자열·배열 항목은 그 항목만 건너뛴다', file: cfg({ sources: [LOCAL], repos: [null, 5, 'x', [1], repo('r1', 'local', 'C:\\a', 'a')] }) },
  { name: '로드: sources 안의 null 은 손상', file: cfg({ sources: [LOCAL, null] }) },
  { name: '로드: sources 가 비어 있지 않은 문자열이면 .filter 가 없어 손상', file: cfg({ sources: 'x' }) },
  { name: '로드: sources 가 length 가 참인 객체면 손상', file: cfg({ sources: { length: 1 } }) },
  { name: '로드: sources 가 length 없는 객체·숫자면 기본 소스', file: cfg({ sources: { a: 1 } }) },
  { name: '로드: sources 가 빈 배열이면 기본 소스', file: cfg({ sources: [] }) },
  { name: '로드: sources 의 비객체 항목(숫자·문자열)은 남고 local 이 없으면 앞에 붙는다', file: cfg({ sources: [5, 'a', WSL] }) },
  { name: '로드: 소스·저장소의 알 수 없는 키와 키 순서는 그대로 보존된다', file: cfg({ sources: [{ label: 'L', id: 'local', kind: 'local', extra: 1 }, WSL], repos: [{ zzz: [1, { a: null }], name: 'r', id: 'r1', path: 'C:\\r', sourceId: 'local', group: 'g' }] }) },
  { name: '로드: github 소스·kind 가 github 인 소스는 버리고 저장소가 있을 때만 다시 만든다', file: cfg({ sources: [LOCAL, GH_SOURCE, { id: 'x', kind: 'github', label: 'X' }, WSL], repos: [] }) },
  {
    name: '로드: github 저장소는 링크를 다시 검증하고 경로를 캐시로 다시 정하며 소스를 다시 만든다',
    cacheDir: true,
    file: cfg({
      sources: [LOCAL, { id: 'github', kind: 'github', label: 'tampered' }],
      repos: [
        repo('g1', 'github', 'D:\\elsewhere\\x.git', 'Web', { github: { owner: 'Acme', repo: 'Web', defaultBranch: 'main' } }),
        repo('g2', 'github', 'whatever', 'web', { github: { owner: 'acme', repo: 'web' } }),
        repo('g3', 'github', 'x', 'bad', { github: { owner: '-a', repo: 'b' } }),
        repo('g4', 'github', 'x', 'nolink'),
        repo('g5', 'github', 'x', 'nullink', { github: null }),
        repo('g6', 'github', 'x', 'unsafe', { github: { owner: 'o', repo: 'r', defaultBranch: '../x' } }),
        repo('l1', 'local', 'C:\\l', 'l', { github: { owner: 'a', repo: 'b' } }),
        repo('l2', 'local', 'C:\\l2', 'l2', { github: null }),
        repo('g7', 'github', 'x', 'extra', { github: { owner: 'o', repo: 'r2', defaultBranch: 'dev', extra: 1 }, flag: true })
      ]
    })
  },
  {
    name: '로드: cacheDir 가 없으면 github 저장소의 path 를 그대로 둔다(중복 판정도 저장된 path 로)',
    file: cfg({
      repos: [
        repo('g1', 'github', 'P1', 'a', { github: { owner: 'a', repo: 'b' } }),
        repo('g2', 'github', 'P1', 'a2', { github: { owner: 'a', repo: 'c' } }),
        repo('g3', 'github', 'P2', 'a3', { github: { owner: 'a', repo: 'd' } })
      ]
    })
  },
  { name: '로드: 예전 프로필 표시와 알 수 없는 최상위 키는 버린다', file: cfg({ version: 7, sources: [LOCAL], repos: [], ui: { ...UI, activeProfileId: 'p1', visibleGroups: ['a'] }, profiles: [{ id: 'p1' }], other: 1 }), ops: [up({ diffMode: 'split' })] },
  { name: '로드: 알 수 없는 ui 키는 보존하고 기본값 위에 덮는다', file: cfg({ ui: { custom: { a: 1 }, theme: 'light', paneWidths: [1, 2], zzz: null } }) },
  { name: '로드: ui 가 배열이면 인덱스 키로 펼쳐지고(정수형 키가 앞), 문자열이면 글자로 펼쳐진다', file: cfg({ ui: ['a', 'b'] }) },
  { name: '로드: ui 가 문자열', file: cfg({ ui: 'xy' }) },
  { name: '로드: ui 가 null·숫자면 기본 ui', file: cfg({ ui: 5 }) },
  { name: '로드: ui 의 정수형 키는 직렬화에서 앞으로 간다', file: cfg({ ui: { b: 1, '10': 'x', a: 2, '2': 'y' } }) },
  { name: '로드: 하루 표시 시각이 한쪽만 있거나 범위·순서가 틀리면 두 키 모두 버린다(정상이면 유지)', file: cfg({ ui: { worklogDayStart: 8 } }) },
  { name: '로드: 하루 표시 시각 정상', file: cfg({ ui: { worklogDayStart: 0, worklogDayEnd: 24 } }) },
  { name: '로드: 하루 표시 시각 시작>=끝', file: cfg({ ui: { worklogDayStart: 9, worklogDayEnd: 9 } }) },
  { name: '로드: 하루 표시 시각 범위 밖', file: cfg({ ui: { worklogDayStart: -1, worklogDayEnd: 25 } }) },
  { name: '로드: 하루 표시 시각이 정수가 아님', file: cfg({ ui: { worklogDayStart: 1.5, worklogDayEnd: '9' } }) },
  { name: '로드: 하루 표시 시각 null 도 정의된 값이라 버린다', file: cfg({ ui: { worklogDayStart: null } }) },
  { name: '로드: 알 수 없는 워크로그 보기 값은 버리고 알려진 값은 유지한다', file: cfg({ ui: { worklogMode: 'year' } }) },
  { name: '로드: 워크로그 보기 week', file: cfg({ ui: { worklogMode: 'week' } }) },
  { name: '로드: fetchApprovals·trustedConfigs 는 참인 값이면 모양과 상관없이 그대로 보존한다', file: cfg({ fetchApprovals: { r1: FP }, trustedConfigs: ['x'] }) },
  { name: '로드: fetchApprovals 가 거짓(null·0·빈 문자열)이면 버리고 {} 는 남긴다', file: cfg({ fetchApprovals: null, trustedConfigs: {} }) },
  { name: '로드: fetchApprovals 가 문자열이어도 보존한다', file: cfg({ fetchApprovals: 'abc', trustedConfigs: 0 }) },
  { name: '로드: window 는 정수 좌표·양수 크기만 받고 maximized 는 true 일 때만 남긴다', file: cfg({ window: { x: -8, y: 10, width: 1200, height: 800, maximized: false } }) },
  { name: '로드: window maximized true', file: cfg({ window: { y: 1, x: 0, width: 5, height: 6, maximized: true, extra: 1 } }) },
  { name: '로드: window 가 틀리면 버린다(0 크기·소수·문자열·한계 초과·배열)', file: cfg({ window: { x: 0, y: 0, width: 0, height: 800 } }) },
  { name: '로드: window 소수 좌표', file: cfg({ window: { x: 0.5, y: 0, width: 100, height: 100 } }) },
  { name: '로드: window 문자열 좌표', file: cfg({ window: { x: '1', y: 0, width: 100, height: 100 } }) },
  { name: '로드: window 한계(100000)는 받고 100001 은 버린다', file: cfg({ window: { x: 100000, y: -100000, width: 100000, height: 1 } }) },
  { name: '로드: window 한계 초과', file: cfg({ window: { x: 100001, y: 0, width: 1, height: 1 } }) },
  { name: '로드: window 가 배열', file: cfg({ window: [1, 2, 3, 4] }) },
  { name: '로드: 같은 키가 중복된 JSON 은 처음 자리에 마지막 값', file: '{"ui":{"theme":"a","locale":"en","theme":"b"},"repos":[]}' }
]

// ── setUi 검증기 ────────────────────────────────────────────────────────────────────────────

const many = <T>(n: number, f: (i: number) => T): T[] => Array.from({ length: n }, (_, i) => f(i))
const SITE = (d: unknown): unknown => ({ jiraSites: { 'https://a.atlassian.net': d } })
const names = (n: number, len = 6): string[] => many(n, (i) => `n${String(i).padStart(len - 1, '0')}`)
const SMILE = '😀'

const SET_UI: ConfigCase[] = [
  { name: 'setUi: 정상 값은 병합되고 기존 키 위치는 유지·새 키는 끝에 붙는다', ops: [up({ theme: 'light', diffMode: 'split', extra: { a: 1 } }), up({ diffMode: 'unified', another: 2, theme: 'dark' })] },
  { name: 'setUi: 정수형 키와 문자 키가 섞인 맵은 JS 열거 순서(정수 먼저)로 쓴다', ops: [up({ groupColors: { b: 200, '2024': 10, a: 5, '7': 1 } })] },
  bad('setUi: githubClientId 정상 100자', { githubClientId: 'a'.repeat(100) }),
  bad('setUi: githubClientId 101자 거부', { githubClientId: 'a'.repeat(101) }),
  bad('setUi: githubClientId 빈 문자열 거부', { githubClientId: '' }),
  bad('setUi: githubClientId 허용 밖 문자 거부', { githubClientId: 'a b' }),
  bad('setUi: githubClientId 허용 문자(._-)', { githubClientId: 'A.b_c-9' }),
  bad('setUi: githubClientId null 거부', { githubClientId: null }),
  bad('setUi: githubClientId 숫자 거부', { githubClientId: 5 }),
  bad('setUi: skippedUpdateVersion 은 값이 정상이어도 거부', { skippedUpdateVersion: '9.9.9' }),
  bad('setUi: skippedUpdateVersion 은 null 이어도 키가 있으면 거부', { skippedUpdateVersion: null }),
  bad('setUi: editorExePath 는 거부', { editorExePath: 'C:\\evil.exe' }),
  bad('setUi: editorExePath null 도 거부', { editorExePath: null }),
  bad('setUi: autoFetchMinutes 0', { autoFetchMinutes: 0 }),
  bad('setUi: autoFetchMinutes 1440', { autoFetchMinutes: 1440 }),
  bad('setUi: autoFetchMinutes 1441 거부', { autoFetchMinutes: 1441 }),
  bad('setUi: autoFetchMinutes -1 거부', { autoFetchMinutes: -1 }),
  bad('setUi: autoFetchMinutes 소수 거부', { autoFetchMinutes: 1.5 }),
  bad('setUi: autoFetchMinutes 문자열 거부', { autoFetchMinutes: '5' }),
  bad('setUi: autoFetchMinutes null 거부', { autoFetchMinutes: null }),
  bad('setUi: autoFetchMinutes 1e3(정수 표기)', { autoFetchMinutes: 1e3 }),
  bad('setUi: 에디터 허용 목록', { editor: 'jetbrains' }),
  bad('setUi: 에디터 허용 밖', { editor: 'emacs' }),
  bad('setUi: 에디터 null 거부', { editor: null }),
  bad('setUi: visibleGroups 중복은 검증만 하고 저장은 원래 값(중복 유지)', { visibleGroups: ['a', 'a', 'b'] }),
  bad('setUi: visibleGroups 빈 목록은 키를 지운다', { visibleGroups: [] }),
  { name: 'setUi: 저장된 visibleGroups 가 있을 때 빈 목록 패치는 키를 지운다', ops: [up({ visibleGroups: ['a'] }), up({ visibleGroups: [] })] },
  bad('setUi: visibleGroups 50개', { visibleGroups: names(50) }),
  bad('setUi: visibleGroups 51개 거부', { visibleGroups: names(51) }),
  bad('setUi: visibleGroups 이름 200단위', { visibleGroups: ['x'.repeat(200)] }),
  bad('setUi: visibleGroups 이름 201단위 거부', { visibleGroups: ['x'.repeat(201)] }),
  bad('setUi: visibleGroups astral 100자(200단위)', { visibleGroups: [SMILE.repeat(100)] }),
  bad('setUi: visibleGroups astral 101자(202단위) 거부', { visibleGroups: [SMILE.repeat(101)] }),
  bad('setUi: visibleGroups 공백뿐인 이름 거부', { visibleGroups: ['  '] }),
  bad('setUi: visibleGroups NBSP(U+00A0)뿐인 이름은 JS trim 이 지우므로 거부', { visibleGroups: ['\u00a0'] }),
  bad('setUi: visibleGroups U+FEFF 뿐인 이름은 JS trim 이 지우므로 거부', { visibleGroups: ['\ufeff'] }),
  bad('setUi: visibleGroups U+0085 는 JS trim 이 지우지 않으므로 허용', { visibleGroups: ['\u0085'] }),
  bad('setUi: visibleGroups NUL 포함 거부', { visibleGroups: ['a\0b'] }),
  bad('setUi: visibleGroups 숫자 항목 거부', { visibleGroups: [1] }),
  bad('setUi: visibleGroups 문자열 거부', { visibleGroups: 'a' }),
  bad('setUi: visibleGroups null 거부', { visibleGroups: null }),
  // Jira
  bad('setUi: jiraProjectKeys 정상 50개', { jiraProjectKeys: many(50, (i) => `AB${i}`) }),
  bad('setUi: jiraProjectKeys 51개 거부', { jiraProjectKeys: many(51, (i) => `AB${i}`) }),
  bad('setUi: jiraProjectKeys 키 형식 20자 허용', { jiraProjectKeys: ['A' + 'B'.repeat(19)] }),
  bad('setUi: jiraProjectKeys 키 형식 21자 거부', { jiraProjectKeys: ['A' + 'B'.repeat(20)] }),
  bad('setUi: jiraProjectKeys 키 형식 1자 거부', { jiraProjectKeys: ['A'] }),
  bad('setUi: jiraProjectKeys 소문자 거부', { jiraProjectKeys: ['abc'] }),
  bad('setUi: jiraProjectKeys 숫자 시작 거부', { jiraProjectKeys: ['1AB'] }),
  bad('setUi: jiraProjectKeys 밑줄·숫자 허용', { jiraProjectKeys: ['A_1', 'ZZ9_9'] }),
  bad('setUi: jiraProjectKeys 배열 아님 거부', { jiraProjectKeys: 'AB' }),
  bad('setUi: jiraLayout list·board', { jiraLayout: 'board' }),
  bad('setUi: jiraLayout 거부', { jiraLayout: 'grid' }),
  bad('setUi: jiraHiddenCategories 정상', { jiraHiddenCategories: ['new', 'done'] }),
  bad('setUi: jiraHiddenCategories 중복 거부', { jiraHiddenCategories: ['new', 'new'] }),
  bad('setUi: jiraHiddenCategories 모르는 값 거부', { jiraHiddenCategories: ['x'] }),
  bad('setUi: worklogDayStart 0·worklogDayEnd 24', { worklogDayStart: 0, worklogDayEnd: 24 }),
  bad('setUi: worklogDayStart 24 거부', { worklogDayStart: 24 }),
  bad('setUi: worklogDayStart 23', { worklogDayStart: 23 }),
  bad('setUi: worklogDayStart -1 거부', { worklogDayStart: -1 }),
  bad('setUi: worklogDayEnd 0 거부', { worklogDayEnd: 0 }),
  bad('setUi: worklogDayEnd 25 거부', { worklogDayEnd: 25 }),
  bad('setUi: worklogDayEnd 1', { worklogDayEnd: 1 }),
  bad('setUi: worklogDayStart 소수 거부', { worklogDayStart: 1.5 }),
  bad('setUi: worklogDayStart null 거부', { worklogDayStart: null }),
  bad('setUi: 시작과 끝이 같으면 거부', { worklogDayStart: 9, worklogDayEnd: 9 }),
  bad('setUi: 시작이 끝보다 늦으면 거부', { worklogDayStart: 10, worklogDayEnd: 9 }),
  bad('setUi: 병합 뒤 검사 — 시작만 바꿔 기본 끝(21)과 같으면 거부', { worklogDayStart: 21 }),
  bad('setUi: 병합 뒤 검사 — 끝만 바꿔 기본 시작(7)과 같으면 거부', { worklogDayEnd: 7 }),
  { name: 'setUi: 병합 뒤 검사 — 저장된 끝(10)보다 늦은 시작만 바꾸면 거부', ops: [up({ worklogDayStart: 8, worklogDayEnd: 10 }), up({ worklogDayStart: 12 }), up({ worklogDayEnd: 8 }), up({ worklogDayEnd: 9 })] },
  bad('setUi: worklogMode', { worklogMode: 'week' }),
  bad('setUi: worklogMode 거부', { worklogMode: 'year' }),
  bad('setUi: jiraSites 정상(필터·팀·최근)', SITE({ filters: [{ name: 'f', jql: 'x' }], team: [{ id: 'u1', name: 'U' }], recent: [{ key: 'AB-1', summary: 's' }] })),
  bad('setUi: jiraSites 20개 허용', { jiraSites: Object.fromEntries(many(20, (i) => [`s${i}`, {}])) }),
  bad('setUi: jiraSites 21개 거부', { jiraSites: Object.fromEntries(many(21, (i) => [`s${i}`, {}])) }),
  bad('setUi: jiraSites 사이트 키 300자 허용', { jiraSites: { ['x'.repeat(300)]: {} } }),
  bad('setUi: jiraSites 사이트 키 301자 거부', { jiraSites: { ['x'.repeat(301)]: {} } }),
  bad('setUi: jiraSites null 거부', { jiraSites: null }),
  bad('setUi: jiraSites 배열 거부', { jiraSites: [] }),
  bad('setUi: jiraSites 사이트 값이 null 이면 거부', { jiraSites: { a: null } }),
  bad('setUi: jiraSites 사이트 값이 배열이면 통과(필드가 없다)', { jiraSites: { a: [1] } }),
  bad('setUi: jiraSites 사이트 값이 숫자면 거부', { jiraSites: { a: 5 } }),
  bad('setUi: filters 50개 허용', SITE({ filters: many(50, () => ({ name: 'f', jql: 'j' })) })),
  bad('setUi: filters 51개 거부', SITE({ filters: many(51, () => ({ name: 'f', jql: 'j' })) })),
  bad('setUi: filters name 100 허용', SITE({ filters: [{ name: 'n'.repeat(100), jql: 'j' }] })),
  bad('setUi: filters name 101 거부', SITE({ filters: [{ name: 'n'.repeat(101), jql: 'j' }] })),
  bad('setUi: filters jql 2000 허용', SITE({ filters: [{ name: 'n', jql: 'j'.repeat(2000) }] })),
  bad('setUi: filters jql 2001 거부', SITE({ filters: [{ name: 'n', jql: 'j'.repeat(2001) }] })),
  bad('setUi: filters 항목이 null 이면 거부', SITE({ filters: [null] })),
  bad('setUi: filters jql 이 없으면 거부', SITE({ filters: [{ name: 'n' }] })),
  bad('setUi: filters 가 배열이 아니면 거부', SITE({ filters: {} })),
  bad('setUi: team 30명 허용', SITE({ team: many(30, (i) => ({ id: `u${i}`, name: 'N' })) })),
  bad('setUi: team 31명 거부', SITE({ team: many(31, (i) => ({ id: `u${i}`, name: 'N' })) })),
  bad('setUi: team id 128자 허용', SITE({ team: [{ id: 'a'.repeat(128), name: 'N' }] })),
  bad('setUi: team id 129자 거부', SITE({ team: [{ id: 'a'.repeat(129), name: 'N' }] })),
  bad('setUi: team id 허용 문자(._@:+-)', SITE({ team: [{ id: 'a.b_c@d:e+f-g', name: 'N' }] })),
  bad('setUi: team id 허용 밖 문자 거부', SITE({ team: [{ id: 'a b', name: 'N' }] })),
  bad('setUi: team id 빈 문자열 거부', SITE({ team: [{ id: '', name: 'N' }] })),
  bad('setUi: team name 200 허용', SITE({ team: [{ id: 'a', name: 'N'.repeat(200) }] })),
  bad('setUi: team name 201 거부', SITE({ team: [{ id: 'a', name: 'N'.repeat(201) }] })),
  bad('setUi: recent 30개 허용', SITE({ recent: many(30, (i) => ({ key: `AB-${i}`, summary: 's' })) })),
  bad('setUi: recent 31개 거부', SITE({ recent: many(31, (i) => ({ key: `AB-${i}`, summary: 's' })) })),
  bad('setUi: recent key 번호 9자리 허용', SITE({ recent: [{ key: 'AB-123456789', summary: 's' }] })),
  bad('setUi: recent key 번호 10자리 거부', SITE({ recent: [{ key: 'AB-1234567890', summary: 's' }] })),
  bad('setUi: recent key 소문자 거부', SITE({ recent: [{ key: 'ab-1', summary: 's' }] })),
  bad('setUi: recent key 번호 없음 거부', SITE({ recent: [{ key: 'AB-', summary: 's' }] })),
  bad('setUi: recent summary 500 허용', SITE({ recent: [{ key: 'AB-1', summary: 's'.repeat(500) }] })),
  bad('setUi: recent summary 501 거부', SITE({ recent: [{ key: 'AB-1', summary: 's'.repeat(501) }] })),
  // Launchpad
  bad('setUi: launchpadPins 정상(GitHub·Jira 키)', { launchpadPins: ['acme/web#12', 'jira:AB-1', 'a.b_c-d/e.f#123456789'] }),
  bad('setUi: launchpadPins 200개 허용', { launchpadPins: many(200, (i) => `o/r#${i + 1}`) }),
  bad('setUi: launchpadPins 201개 거부', { launchpadPins: many(201, (i) => `o/r#${i + 1}`) }),
  bad('setUi: launchpadPins 대문자 GitHub 키 거부', { launchpadPins: ['Acme/web#1'] }),
  bad('setUi: launchpadPins 번호 10자리 거부', { launchpadPins: ['o/r#1234567890'] }),
  bad('setUi: launchpadPins 번호 없음 거부', { launchpadPins: ['o/r#'] }),
  bad('setUi: launchpadPins owner 100자 허용', { launchpadPins: [`${'a'.repeat(100)}/r#1`] }),
  bad('setUi: launchpadPins owner 101자 거부', { launchpadPins: [`${'a'.repeat(101)}/r#1`] }),
  bad('setUi: launchpadPins jira 소문자 거부', { launchpadPins: ['jira:ab-1'] }),
  bad('setUi: launchpadPins jira 번호 10자리 거부', { launchpadPins: ['jira:AB-1234567890'] }),
  bad('setUi: launchpadPins 슬래시 없음 거부', { launchpadPins: ['abc#1'] }),
  bad('setUi: launchpadPins 문자열 거부', { launchpadPins: 'a' }),
  bad('setUi: launchpadSnoozes 정상', { launchpadSnoozes: { 'o/r#1': 1700000000000, 'jira:AB-2': 5 } }),
  bad('setUi: launchpadSnoozes 안전 정수 한계(2^53-1)는 허용', { launchpadSnoozes: { 'o/r#1': 9007199254740991 } }),
  bad('setUi: launchpadSnoozes 2^53 거부', { launchpadSnoozes: { 'o/r#1': 9007199254740992 } }),
  bad('setUi: launchpadSnoozes 0 거부', { launchpadSnoozes: { 'o/r#1': 0 } }),
  bad('setUi: launchpadSnoozes 음수 거부', { launchpadSnoozes: { 'o/r#1': -1 } }),
  bad('setUi: launchpadSnoozes 소수 거부', { launchpadSnoozes: { 'o/r#1': 1.5 } }),
  bad('setUi: launchpadSnoozes 문자열 값 거부', { launchpadSnoozes: { 'o/r#1': '5' } }),
  bad('setUi: launchpadSnoozes 키 형식 거부', { launchpadSnoozes: { nope: 5 } }),
  bad('setUi: launchpadSnoozes 배열 거부', { launchpadSnoozes: [] }),
  bad('setUi: launchpadSnoozes null 거부', { launchpadSnoozes: null }),
  bad('setUi: launchpadSnoozes 500개 허용', { launchpadSnoozes: Object.fromEntries(many(500, (i) => [`o/r#${i + 1}`, 5])) }),
  bad('setUi: launchpadSnoozes 501개 거부', { launchpadSnoozes: Object.fromEntries(many(501, (i) => [`o/r#${i + 1}`, 5])) }),
  // 저장소 목록 순서·색
  bad('setUi: groupColors·sourceColors 색 0·359', { groupColors: { a: 0, b: 359 }, sourceColors: { 'wsl:Ubuntu': 120 } }),
  bad('setUi: groupColors 색 360 거부', { groupColors: { a: 360 } }),
  bad('setUi: groupColors 색 -1 거부', { groupColors: { a: -1 } }),
  bad('setUi: sourceColors 색 소수 거부', { sourceColors: { a: 1.5 } }),
  bad('setUi: groupColors 빈 키 거부', { groupColors: { '': 1 } }),
  bad('setUi: groupColors 키 200단위 허용', { groupColors: { ['k'.repeat(200)]: 1 } }),
  bad('setUi: groupColors 키 201단위 거부', { groupColors: { ['k'.repeat(201)]: 1 } }),
  bad('setUi: groupColors 키 astral 100자 허용(200단위)', { groupColors: { [SMILE.repeat(100)]: 1 } }),
  bad('setUi: groupColors 키 astral 101자 거부(202단위)', { groupColors: { [SMILE.repeat(101)]: 1 } }),
  bad('setUi: groupColors 키 NUL 거부', { groupColors: { 'a\0': 1 } }),
  bad('setUi: groupColors 배열 거부', { groupColors: [] }),
  bad('setUi: groupColors null 거부', { groupColors: null }),
  bad('setUi: groupColors 500개 허용', { groupColors: Object.fromEntries(many(500, (i) => [`g${i}`, 1])) }),
  bad('setUi: groupColors 501개 거부', { groupColors: Object.fromEntries(many(501, (i) => [`g${i}`, 1])) }),
  bad('setUi: sourceOrder 500개 허용', { sourceOrder: many(500, (i) => `s${i}`) }),
  bad('setUi: sourceOrder 501개 거부', { sourceOrder: many(501, (i) => `s${i}`) }),
  bad('setUi: sourceOrder 중복 거부', { sourceOrder: ['a', 'a'] }),
  bad('setUi: sourceOrder 빈 이름 거부', { sourceOrder: [''] }),
  bad('setUi: repoOrder 정상', { repoOrder: ['r1', 'r2'] }),
  bad('setUi: repoOrder 숫자 거부', { repoOrder: [1] }),
  bad('setUi: groupOrder 정상', { groupOrder: { g: ['r1', 'r2'], h: [] } }),
  bad('setUi: groupOrder 값 중복 거부', { groupOrder: { g: ['r1', 'r1'] } }),
  bad('setUi: groupOrder 값이 배열이 아니면 거부', { groupOrder: { g: 'r1' } }),
  bad('setUi: groupOrder 항목 500개 허용', { groupOrder: { g: many(500, (i) => `r${i}`) } }),
  bad('setUi: groupOrder 항목 501개 거부', { groupOrder: { g: many(501, (i) => `r${i}`) } }),
  // 검사 순서(두 가지가 틀린 패치는 먼저 검사하는 쪽의 문구)
  bad('setUi: 순서 — githubClientId 가 skippedUpdateVersion 보다 먼저', { skippedUpdateVersion: 'x', githubClientId: '!' }),
  bad('setUi: 순서 — skippedUpdateVersion 이 autoFetchMinutes 보다 먼저', { autoFetchMinutes: -1, skippedUpdateVersion: 'x' }),
  bad('setUi: 순서 — autoFetchMinutes 가 editorExePath 보다 먼저', { editorExePath: 'x', autoFetchMinutes: -1 }),
  bad('setUi: 순서 — editorExePath 가 editor 보다 먼저', { editor: 'emacs', editorExePath: 'x' }),
  bad('setUi: 순서 — editor 가 visibleGroups 보다 먼저', { visibleGroups: 5, editor: 'emacs' }),
  bad('setUi: 순서 — visibleGroups 가 Jira 보다 먼저', { jiraLayout: 'grid', visibleGroups: 5 }),
  bad('setUi: 순서 — Jira 가 Launchpad 보다 먼저', { launchpadPins: 5, jiraLayout: 'grid' }),
  bad('setUi: 순서 — Launchpad 가 저장소 목록보다 먼저', { repoOrder: 5, launchpadPins: 5 }),
  bad('setUi: 순서 — 병합 뒤 검사는 개별 검사 뒤', { worklogDayStart: 21, editor: 'emacs' }),
  // 패치가 객체가 아닌 경우
  bad('setUi: 패치가 null 이면 TypeError', null),
  bad('setUi: 패치가 숫자면 TypeError', 5),
  bad('setUi: 패치가 문자열이면 TypeError', 'x'),
  bad('setUi: 패치가 배열이면 던지지 않고 인덱스 키가 병합된다', ['a']),
  bad('setUi: 빈 패치도 쓴다(알림이 간다)', {}),
  // 승인은 setUi 로 위조할 수 없다(패치는 ui 아래로만 들어간다)
  { name: 'setUi: fetchApprovals·trustedConfigs 를 패치로 보내도 최상위 승인은 바뀌지 않는다(ui 안에 남을 뿐)', ops: [{ op: 'addRepo', input: { sourceId: 'local', path: 'C:\\r', name: 'r' } }, up({ fetchApprovals: { 'id-1': 'forged' }, trustedConfigs: { 'id-1': 'forged' } })] }
]

// ── 변경 연산 ───────────────────────────────────────────────────────────────────────────────

const R1 = repo('r1', 'local', 'C:\\a', 'a')
const R2 = repo('r2', 'wsl:Ubuntu', '/home/u/b', 'b', { group: 'g' })
const R3 = repo('r3', 'ssh:box', '/srv/c', 'c')
const BASE = cfg({
  version: 1,
  sources: [LOCAL, WSL, SSH],
  repos: [R1, R2, R3],
  ui: { ...UI, lastRepoId: 'r1' },
  fetchApprovals: { r1: FP, r2: FP2 },
  trustedConfigs: { r1: FP, r3: FP2 },
  window: { x: 1, y: 2, width: 3, height: 4 }
})

const OPS: ConfigCase[] = [
  {
    name: 'addRepo: id 는 주입한 순번이고 소스가 함께 추가되며 같은 소스·경로는 기존 항목을 돌려준다(쓰지 않는다)',
    ops: [
      { op: 'addRepo', input: { sourceId: 'local', path: 'C:\\a', name: 'a' } },
      { op: 'addRepo', input: { sourceId: 'local', path: 'C:\\a', name: 'other' } },
      { op: 'addRepo', input: { sourceId: 'wsl:Ubuntu', path: '/x', name: 'x', group: 'g' }, source: WSL },
      { op: 'addRepo', input: { sourceId: 'wsl:Ubuntu', path: '/y', name: 'y' }, source: WSL },
      { op: 'addRepo', input: { sourceId: 'ssh:box', path: '/y', name: 'y' }, source: SSH },
      { op: 'addRepo', input: { sourceId: 'local', path: 'C:\\z', name: 'z', extra: [1, 2], zz: null } }
    ]
  },
  { name: 'addRepo: 파일에서 읽은 저장소와 같은 소스·경로면 기존 항목(알 수 없는 키 포함)을 돌려준다', file: BASE, ops: [{ op: 'addRepo', input: { sourceId: 'local', path: 'C:\\a', name: 'new' } }] },
  { name: 'addRepo: 소스·경로가 없는 입력(undefined 끼리는 같다)', ops: [{ op: 'addRepo', input: { name: 'n' } }, { op: 'addRepo', input: { name: 'm' } }] },
  {
    name: 'addRepo: github 저장소(링크 포함)와 github 소스',
    cacheDir: true,
    ops: [
      { op: 'addRepo', input: { sourceId: 'github', path: `${CACHE}\\acme\\web.git`, name: 'web', github: { owner: 'Acme', repo: 'Web' } }, source: GH_SOURCE },
      { op: 'addRepo', input: { sourceId: 'github', path: `${CACHE}\\acme\\web.git`, name: 'web', github: { owner: 'Acme', repo: 'Web' } }, source: GH_SOURCE },
      { op: 'removeRepo', id: 'id-1' }
    ]
  },
  {
    name: 'updateRepo: 이름 trim·빈 이름 무시·그룹 설정/trim/해제',
    file: BASE,
    ops: [
      { op: 'updateRepo', id: 'r1', patch: { name: '  새 이름  ' } },
      { op: 'updateRepo', id: 'r1', patch: { name: '   ' } },
      { op: 'updateRepo', id: 'r1', patch: { name: '' } },
      { op: 'updateRepo', id: 'r1', patch: { group: '  G1 ' } },
      { op: 'updateRepo', id: 'r1', patch: { group: '   ' } },
      { op: 'updateRepo', id: 'r2', patch: { group: null } },
      { op: 'updateRepo', id: 'r3', patch: { group: '' } },
      { op: 'updateRepo', id: 'r3', patch: { name: '\u00a0n\u00a0', group: '\ufeffx\u0085' } }
    ]
  },
  {
    name: 'updateRepo: 모르는 id·빈 패치·알 수 없는 키도 쓴다(알림이 간다)',
    file: BASE,
    ops: [{ op: 'updateRepo', id: 'nope', patch: { name: 'x' } }, { op: 'updateRepo', id: 'r1', patch: {} }, { op: 'updateRepo', id: 'r1', patch: { id: 'hacked', path: 'D:\\x', sourceId: 'github' } }]
  },
  {
    name: 'updateRepo: 문자열이 아닌 name·group 과 null 패치는 TypeError, 배열·숫자 패치는 필드가 없어 그대로 쓴다',
    file: BASE,
    ops: [
      { op: 'updateRepo', id: 'r1', patch: { name: null } },
      { op: 'updateRepo', id: 'r1', patch: { name: 5 } },
      { op: 'updateRepo', id: 'r1', patch: { group: 5 } },
      { op: 'updateRepo', id: 'r1', patch: { group: false } },
      { op: 'updateRepo', id: 'r1', patch: null },
      { op: 'updateRepo', id: 'r1', patch: [1] },
      { op: 'updateRepo', id: 'r1', patch: 5 }
    ]
  },
  {
    name: 'updateRepo: TypeError 는 id 가 맞는 저장소가 있을 때만 난다(모르는 id 는 던지지 않고 쓴다)',
    file: BASE,
    ops: [
      { op: 'updateRepo', id: 'nope', patch: null },
      { op: 'updateRepo', id: 'nope', patch: { name: null, group: 5 } },
      { op: 'updateRepo', id: 'r1', patch: { name: 'ok', group: 5 } }
    ]
  },
  {
    name: 'removeRepo: 쓰지 않는 wsl·ssh 소스와 승인·허용 지문을 정리하고 local 은 남긴다, lastRepoId 는 undefined 자리가 된다',
    file: BASE,
    ops: [{ op: 'removeRepo', id: 'r2' }, { op: 'removeRepo', id: 'r1' }, { op: 'removeRepo', id: 'r3' }, up({ lastRepoId: 'later' })]
  },
  {
    name: 'removeRepo: 모르는 id 도 쓰고 승인 맵 두 개를 {} 로 남긴다(window 뒤에 붙는다)',
    file: cfg({ version: 1, sources: [LOCAL], repos: [R1], ui: UI, window: { x: 1, y: 2, width: 3, height: 4 } }),
    ops: [{ op: 'removeRepo', id: 'nope' }]
  },
  {
    name: 'removeRepo: lastRepoId 가 다른 저장소면 그대로',
    file: BASE,
    ops: [{ op: 'removeRepo', id: 'r3' }]
  },
  {
    name: 'removeRepo: 마지막 github 저장소를 지우면 github 소스도 사라진다',
    cacheDir: true,
    file: cfg({ sources: [LOCAL], repos: [R1, gh('g1', 'Acme', 'Web'), gh('g2', 'acme', 'api')] }),
    ops: [{ op: 'removeRepo', id: 'g1' }, { op: 'removeRepo', id: 'g2' }]
  },
  {
    name: 'removeRepo: 승인 맵이 비객체(문자열·배열)여도 TS 의 펼침 규칙대로 정리한다',
    file: cfg({ sources: [LOCAL], repos: [R1], fetchApprovals: 'ab', trustedConfigs: ['x', 'y'] }),
    ops: [{ op: 'removeRepo', id: '1' }]
  },
  {
    name: 'removeRepo: lastRepoId 가 있는 ui 에서 키 위치가 유지된다(undefined 자리를 나중에 채운다)',
    ops: [{ op: 'addRepo', input: { sourceId: 'local', path: 'C:\\r', name: 'r' } }, up({ lastRepoId: 'id-1', diffMode: 'split' }), { op: 'removeRepo', id: 'id-1' }, up({ lastRepoId: 'z' })]
  },
  {
    name: 'setEditorExePath: 정상·경계·거부',
    ops: [
      { op: 'setEditorExePath', path: 'C:\\Program Files\\Code\\Code.exe' },
      { op: 'setEditorExePath', path: '' },
      { op: 'setEditorExePath', path: 'a'.repeat(4096) },
      { op: 'setEditorExePath', path: 'a'.repeat(4097) },
      { op: 'setEditorExePath', path: `${SMILE.repeat(2048)}` },
      { op: 'setEditorExePath', path: `${SMILE.repeat(2049)}` },
      { op: 'setEditorExePath', path: 'a\0b' }
    ]
  },
  {
    name: 'setSkippedUpdateVersion: 정상·거부',
    ops: [
      { op: 'setSkippedUpdateVersion', version: '1.2.3' },
      { op: 'setSkippedUpdateVersion', version: '10.20.30-beta.1' },
      { op: 'setSkippedUpdateVersion', version: '1.2.3-' },
      { op: 'setSkippedUpdateVersion', version: `1.2.3-${'a'.repeat(40)}` },
      { op: 'setSkippedUpdateVersion', version: `1.2.3-${'a'.repeat(41)}` },
      { op: 'setSkippedUpdateVersion', version: '1.2' },
      { op: 'setSkippedUpdateVersion', version: '1.2.3.4' },
      { op: 'setSkippedUpdateVersion', version: 'v1.2.3' },
      { op: 'setSkippedUpdateVersion', version: '1.2.3-a_b' },
      { op: 'setSkippedUpdateVersion', version: '1.2.3\n' },
      { op: 'setSkippedUpdateVersion', version: '' }
    ]
  },
  {
    name: 'setGithubDefaultBranch: 기록·같은 값·쓸 수 없는 이름·링크 없는 저장소는 쓰지 않는다',
    cacheDir: true,
    file: cfg({ sources: [LOCAL], repos: [R1, gh('g1', 'Acme', 'Web'), gh('g2', 'acme', 'api', {}, { defaultBranch: 'main' })] }),
    ops: [
      { op: 'setGithubDefaultBranch', id: 'g1', branch: 'main' },
      { op: 'setGithubDefaultBranch', id: 'g1', branch: 'main' },
      { op: 'setGithubDefaultBranch', id: 'g1', branch: '-x' },
      { op: 'setGithubDefaultBranch', id: 'g1', branch: 'a..b' },
      { op: 'setGithubDefaultBranch', id: 'g1', branch: 'release/1.0+x' },
      { op: 'setGithubDefaultBranch', id: 'g2', branch: 'trunk' },
      { op: 'setGithubDefaultBranch', id: 'r1', branch: 'main' },
      { op: 'setGithubDefaultBranch', id: 'nope', branch: 'main' }
    ]
  },
  {
    name: 'setGithubDefaultBranch: 이름 규칙 경계',
    cacheDir: true,
    file: cfg({ sources: [LOCAL], repos: [gh('g1', 'Acme', 'Web')] }),
    ops: ['a'.repeat(200), 'a'.repeat(201), '/a', 'a/', 'a.', 'a.lock', 'a//b', '.a', 'a/.b', 'a b', 'ä', 'a/b.c-d_e+f'].map((branch) => ({ op: 'setGithubDefaultBranch' as const, id: 'g1', branch }))
  },
  {
    name: 'setTrustedConfig: 같은 값은 쓰지 않고, 있는 키는 제자리에서 바뀌며, 없으면 끝에 붙는다',
    file: BASE,
    ops: [
      { op: 'setTrustedConfig', id: 'r1', fingerprint: FP },
      { op: 'setTrustedConfig', id: 'r1', fingerprint: FP2 },
      { op: 'setTrustedConfig', id: 'r2', fingerprint: FP },
      { op: 'setTrustedConfig', id: 'r3', fingerprint: FP }
    ]
  },
  { name: 'setTrustedConfig: 맵이 없던 설정', ops: [{ op: 'setTrustedConfig', id: 'x', fingerprint: FP }] },
  {
    name: 'setFetchApproval: 새 지문은 키를 끝으로 옮기고 null 은 지우며(빈 {} 가 남는다) 같은 값은 쓰지 않는다',
    file: BASE,
    ops: [
      { op: 'setFetchApproval', id: 'r1', fingerprint: FP },
      { op: 'setFetchApproval', id: 'r1', fingerprint: FP2 },
      { op: 'setFetchApproval', id: 'r2', fingerprint: null },
      { op: 'setFetchApproval', id: 'r2', fingerprint: null },
      { op: 'setFetchApproval', id: 'r1', fingerprint: null },
      { op: 'setFetchApproval', id: 'r3', fingerprint: '' },
      { op: 'setFetchApproval', id: 'r3', fingerprint: FP }
    ]
  },
  { name: 'setFetchApproval: 맵이 없던 설정에서 null 은 쓰지 않는다', ops: [{ op: 'setFetchApproval', id: 'x', fingerprint: null }, { op: 'setFetchApproval', id: 'x', fingerprint: FP }] },
  {
    name: '파생 값: 미러(소스를 못 찾은 저장소 제외·wsl distro·ssh host)와 fetch 설정(기본 5분·지문은 목록 안의 id 만·id 순)',
    file: cfg({
      sources: [LOCAL, WSL, SSH, GH_SOURCE, { id: 'wsl:Empty', kind: 'wsl', label: 'x', distro: '' }, { id: 'ssh:odd', kind: 'ssh', label: 'x' }],
      repos: [
        repo('rb', 'local', 'C:\\b', 'b'),
        repo('ra', 'wsl:Ubuntu', '/a', 'a'),
        repo('rc', 'ssh:box', '/c', 'c', { group: 'g' }),
        repo('rd', 'gone', '/d', 'd'),
        repo('re', 'wsl:Empty', '/e', 'e'),
        repo('rf', 'ssh:odd', '/f', 'f'),
        repo('rg', 'github', 'P', 'g', { github: { owner: 'o', repo: 'r' } })
      ],
      ui: { ...UI, autoFetchMinutes: 0 },
      fetchApprovals: { rb: FP, ra: FP2, orphan: FP, rc: 5 },
      trustedConfigs: { rg: FP, rb: FP2 }
    }),
    ops: [up({ autoFetchMinutes: 12 }), up({ autoFetchMinutes: 0 })]
  },
  { name: '파생 값: autoFetchMinutes 가 없으면 5분, null 이어도 5분', file: cfg({ ui: { autoFetchMinutes: null } }) },
  { name: '파생 값: autoFetchMinutes 가 숫자가 아니면 0', file: cfg({ ui: { autoFetchMinutes: '5' } }) },
  { name: '파생 값: autoFetchMinutes 가 음수·소수', file: cfg({ ui: { autoFetchMinutes: -3 } }) },
  { name: '파생 값: autoFetchMinutes 소수는 그대로', file: cfg({ ui: { autoFetchMinutes: 2.5 } }) }
]

export const STORE_CASES: ConfigCase[] = [...LOAD, ...SET_UI, ...OPS]

// ── GitHub 링크 순수 함수 ───────────────────────────────────────────────────────────────────

/** validateGithubLink·sanitizeGithubLink 입력(JSON 값) */
export const LINK_INPUTS: unknown[] = [
  { owner: 'a', repo: 'b' },
  { owner: 'Acme', repo: 'Web.App', defaultBranch: 'main' },
  { owner: 'a', repo: 'b', defaultBranch: '-bad' },
  { owner: 'a', repo: 'b', defaultBranch: 5 },
  { owner: 'a', repo: 'b', defaultBranch: 'release/1.0' },
  { owner: 'a'.repeat(39), repo: 'b' },
  { owner: 'a'.repeat(40), repo: 'b' },
  { owner: 'a', repo: 'b'.repeat(100) },
  { owner: 'a', repo: 'b'.repeat(101) },
  { owner: 'a-b', repo: 'b_c.d' },
  { owner: '-a', repo: 'b' },
  { owner: 'a', repo: '-b' },
  { owner: 'a-', repo: 'b' },
  { owner: 'a.', repo: 'b' },
  { owner: 'a', repo: 'b.' },
  { owner: '.', repo: 'b' },
  { owner: '..', repo: 'b' },
  { owner: 'a', repo: '.' },
  { owner: 'a', repo: '..' },
  { owner: 'con', repo: 'b' },
  { owner: 'CON', repo: 'b' },
  { owner: 'a', repo: 'con' },
  { owner: 'a', repo: 'con.txt' },
  { owner: 'a', repo: 'Con.git' },
  { owner: 'a', repo: 'nul' },
  { owner: 'a', repo: 'aux.x.y' },
  { owner: 'a', repo: 'prn' },
  { owner: 'a', repo: 'com1' },
  { owner: 'a', repo: 'COM9' },
  { owner: 'a', repo: 'com0' },
  { owner: 'a', repo: 'lpt1' },
  { owner: 'a', repo: 'LPT9.log' },
  { owner: 'a', repo: 'com10' },
  { owner: 'a', repo: 'comx' },
  { owner: 'a', repo: 'console' },
  { owner: 'a', repo: 'x.git' },
  { owner: 'a', repo: 'x.GIT' },
  { owner: 'a', repo: 'x.Git' },
  { owner: 'a', repo: '.git' },
  { owner: 'a', repo: 'git' },
  { owner: 'a', repo: 'x.gitx' },
  { owner: 'a', repo: 'x git' },
  { owner: 'a', repo: 'x/y' },
  { owner: 'a', repo: 'x\\y' },
  { owner: 'a', repo: '한글' },
  { owner: '', repo: 'b' },
  { owner: 'a', repo: '' },
  { owner: 'a' },
  { repo: 'b' },
  { owner: 5, repo: 'b' },
  { owner: 'a', repo: null },
  {},
  [],
  ['a', 'b'],
  null,
  5,
  'a/b',
  true
]

/** isSafeBranch 입력 */
export const BRANCH_INPUTS: unknown[] = [
  'main',
  'release/1.0',
  'feature/a+b_c-d.e',
  'a'.repeat(200),
  'a'.repeat(201),
  '',
  '-a',
  '/a',
  'a/',
  'a.',
  'a.lock',
  'a.lock/b',
  'a//b',
  'a..b',
  '.a',
  'a/.b',
  'a/b.',
  'a b',
  'a@b',
  'a~b',
  'a:b',
  'ä',
  '..',
  '.',
  'HEAD',
  5,
  null
]

const CACHE_BASES = [CACHE, `${CACHE}\\`, 'C:\\golden\\cache\\..\\cache', 'c:/golden/cache', '\\\\host\\share\\cache', 'D:\\', 'C:\\golden\\Cache']

/** (base, path) 쌍 — githubCacheSlug·isGithubCachePath·cachePathMatchesLink 에 쓴다. 드라이브 절대 경로·UNC 만(resolve 가 작업 디렉터리를 읽지 않게) */
export const SLUG_CASES: [string, string][] = [
  [CACHE, `${CACHE}\\acme\\web.git`],
  [CACHE, `${CACHE}\\acme\\web.git\\`],
  [CACHE, `${CACHE}\\Acme\\web.git`],
  [CACHE, `${CACHE}\\acme\\Web.git`],
  [CACHE, `${CACHE}\\acme\\web.GIT`],
  [CACHE, `${CACHE}\\acme\\web`],
  [CACHE, `${CACHE}\\acme\\.git`],
  [CACHE, `${CACHE}\\acme\\web.git\\objects`],
  [CACHE, `${CACHE}\\acme`],
  [CACHE, CACHE],
  [CACHE, `${CACHE}\\..\\cache\\acme\\web.git`],
  [CACHE, `${CACHE}\\acme\\..\\acme\\web.git`],
  [CACHE, `${CACHE}\\.\\acme\\web.git`],
  [CACHE, `${CACHE}\\acme\\\\web.git`],
  [CACHE, `C:/golden/cache/acme/web.git`],
  [CACHE, `c:\\GOLDEN\\Cache\\acme\\web.git`],
  [CACHE, `C:\\golden\\other\\acme\\web.git`],
  [CACHE, `C:\\golden\\cache2\\acme\\web.git`],
  [CACHE, `C:\\golden\\acme\\web.git`],
  [CACHE, `D:\\golden\\cache\\acme\\web.git`],
  [CACHE, `\\\\host\\share\\golden\\cache\\acme\\web.git`],
  [CACHE, `${CACHE}\\-a\\web.git`],
  [CACHE, `${CACHE}\\a.\\web.git`],
  [CACHE, `${CACHE}\\con\\web.git`],
  [CACHE, `${CACHE}\\acme\\con.git`],
  [CACHE, `${CACHE}\\acme\\a b.git`],
  [CACHE, `${CACHE}\\${'a'.repeat(39)}\\b.git`],
  [CACHE, `${CACHE}\\${'a'.repeat(40)}\\b.git`],
  [CACHE, `${CACHE}\\a\\${'b'.repeat(100)}.git`],
  [CACHE, `${CACHE}\\a\\${'b'.repeat(101)}.git`],
  [CACHE, `${CACHE}\\a\\x.git.git`],
  [CACHE, `${CACHE}\\acme\\web.git\0`],
  [`${CACHE}\\`, `${CACHE}\\acme\\web.git`],
  ['C:\\golden\\cache\\..\\cache', `${CACHE}\\acme\\web.git`],
  ['c:/golden/cache', `C:\\golden\\cache\\acme\\web.git`],
  ['\\\\host\\share\\cache', '\\\\host\\share\\cache\\acme\\web.git'],
  ['\\\\HOST\\Share\\cache', '\\\\host\\share\\CACHE\\acme\\web.git'],
  ['\\\\host\\share\\cache', '\\\\host\\other\\cache\\acme\\web.git'],
  ['D:\\', 'D:\\acme\\web.git'],
  ['D:\\', 'D:\\x\\acme\\web.git'],
  ['D:\\', 'C:\\acme\\web.git'],
  ['C:\\golden\\Cache', `${CACHE}\\acme\\web.git`],
  ['C:\\', 'C:\\a\\b.git'],
  [CACHE, 'C:\\golden\\cache\\a\\b.git'],
  [CACHE, 'C:\\golden\\cache\\\u00c4\\b.git']
]

/** githubCachePath·githubRemoteUrl 입력(링크는 검증을 통과한 값) */
export const PATH_LINKS: { owner: string; repo: string }[] = [
  { owner: 'Acme', repo: 'Web' },
  { owner: 'a', repo: 'b.c' },
  { owner: 'A-B', repo: 'X_Y.Z' }
]

export { CACHE_BASES }

/** cachePathMatchesLink 입력(base 는 CACHE) */
export const MATCH_CASES: [string, { owner: string; repo: string }][] = [
  [`${CACHE}\\acme\\web.git`, { owner: 'Acme', repo: 'Web' }],
  [`${CACHE}\\acme\\web.git`, { owner: 'acme', repo: 'web' }],
  [`${CACHE}\\acme\\web.git`, { owner: 'acme', repo: 'api' }],
  [`${CACHE}\\acme\\web.git`, { owner: 'globex', repo: 'web' }],
  [`${CACHE}\\Acme\\web.git`, { owner: 'acme', repo: 'web' }],
  [`${CACHE}\\acme\\web.git\\x`, { owner: 'acme', repo: 'web' }]
]
