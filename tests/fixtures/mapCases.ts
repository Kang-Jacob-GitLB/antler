// 응답 매퍼 골든(tests/golden/net/ghmap.json·jiramap.json)의 입력 표. 입력은 JS 객체가 아니라 응답 원문(JSON 텍스트)이다 —
// 중복 키·서로게이트 이스케이프는 객체로 쓸 수 없고, Rust 는 같은 원문을 자체 해석기(net/json.rs)로 읽는다.
// 표본은 tests/ghApi·ghChecks·ghAccessibleRepos·launchpadQuery·jiraAdf·jiraHardening·jiraService 가 쓰는 응답 모양(PR·리뷰·체크·ADF 문서·워크로그·dev-status·시각 문자열)을 따랐고,
// 경계 사례(null·틀린 타입·제어 문자·거대 값)를 더했다. 그 테스트들의 픽스처 객체 자체는 가짜 서버·서비스에 묶여 있어 그대로 import 하지 않았다.

/** JSON 텍스트로 */
export const J = (v: unknown): string => JSON.stringify(v)

const user = (login: string): Record<string, unknown> => ({ login, avatar_url: `https://a/${login}.png` })

export const PULL = (n: number, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  number: n,
  title: `PR ${n}`,
  state: 'open',
  draft: false,
  merged_at: null,
  user: user('alice'),
  labels: [{ name: 'bug', color: 'ff0000' }],
  assignees: [user('bob')],
  requested_reviewers: [],
  head: { ref: `feat-${n}`, sha: `sha${n}` },
  base: { ref: 'main' },
  updated_at: '2026-01-02T00:00:00Z',
  created_at: '2026-01-01T00:00:00Z',
  html_url: `https://github.com/octo/hello/pull/${n}`,
  ...extra
})

export const ISSUE = (n: number, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  number: n,
  title: `이슈 ${n}`,
  state: 'open',
  user: user('carol'),
  labels: [{ name: 'bug', color: '#ff0000' }, 'plain'],
  assignees: [],
  comments: 2,
  updated_at: '2026-02-02T00:00:00Z',
  created_at: '2026-02-01T00:00:00Z',
  html_url: `https://github.com/octo/hello/issues/${n}`,
  ...extra
})

const rv = (login: string, state: unknown, t: number | null): Record<string, unknown> => ({
  user: user(login),
  state,
  submitted_at: t === null ? null : new Date(t * 1000).toISOString()
})

/** [응답 원문 목록] — `fn` 마다 TS 구현이 받는 인자 모양은 tests/goldenMaps.ts 에 있다 */
export interface MapCase {
  fn: string
  in: string[]
}

const c = (fn: string, ...inputs: unknown[]): MapCase => ({ fn, in: inputs.map((x) => (typeof x === 'string' && x.startsWith('RAW:') ? x.slice(4) : J(x))) })
/** 원문 그대로(JSON 으로 다시 쓰지 않는다) */
const raw = (t: string): string => `RAW:${t}`

const SHA1 = '0'.repeat(39) + '1'

// ── 매퍼 단위 ─────────────────────────────────────────────────────────────

const PULL_VARIANTS: unknown[] = [
  PULL(1),
  PULL(2, { merged_at: '2026-01-03T00:00:00Z', state: 'closed' }),
  PULL(3, { state: 'closed' }),
  PULL(4, { draft: true, comments: 7 }),
  PULL(5, { user: null }),
  PULL(6, { labels: ['x', { name: 'n', color: '#00ff00' }, { color: null }, { name: 'only' }], assignees: [null, user('z'), {}] }),
  PULL(7, { head: null, base: undefined }),
  { title: 'no number' },
  PULL(8, { updated_at: null, created_at: '', comments: null, html_url: null, title: null }),
  PULL(9, { updated_at: '2026-01-02T09:00:00.123+0900', created_at: '2026-01-02' }),
  PULL(10, { updated_at: 'not a date', created_at: true }),
  PULL(11, { title: 5, state: 7, draft: 'true', merged_at: 0 }),
  PULL(12, { merged_at: 'x' }),
  PULL(13, { labels: 'bug' }),
  PULL(14, { labels: [null] }),
  PULL(15, { labels: [{ name: 'a', color: 5 }] }),
  PULL(16, { assignees: { login: 'x' } }),
  PULL(17, { labels: [], assignees: null }),
  PULL(18, { title: '제어\u0000\u001f문자와 "따옴표" \\ 그리고 이모지 😀', labels: [{ name: '\u007f', color: '' }] }),
  PULL(19, { number: '19', comments: 1e21 }),
  PULL(20, { user: [], head: [], base: 'main' })
]

const ISSUE_VARIANTS: unknown[] = [
  ISSUE(1),
  ISSUE(2, { state: 'closed', pull_request: { merged_at: '2026-02-03T00:00:00Z' } }),
  ISSUE(3, { state: 'closed', pull_request: {}, draft: true }),
  ISSUE(4, { user: null, labels: null, assignees: null, comments: undefined }),
  ISSUE(5, { state: 'weird' }),
  ISSUE(6, { labels: 'x' }),
  ISSUE(7, { updated_at: '2026-02-02T00:00:00+09:00', closed_at: '2026-02-04T00:00:00Z', body_html: '<p>x</p>' }),
  ISSUE(8, { closed_at: 'bad', body_html: null }),
  ISSUE(9, { pull_request: 'yes' }),
  ISSUE(10, { pull_request: null })
]

const REVIEW_SETS: [unknown[], number][] = [
  [[rv('a', 'APPROVED', 1), rv('b', 'CHANGES_REQUESTED', 2)], 0],
  [[rv('a', 'APPROVED', 1), rv('b', 'COMMENTED', 2)], 0],
  [[rv('a', 'COMMENTED', 1)], 2],
  [[], 1],
  [[], 0],
  [[rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'APPROVED', 2)], 0],
  [[rv('a', 'APPROVED', 1), rv('a', 'CHANGES_REQUESTED', 2)], 0],
  [[rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'COMMENTED', 2)], 0],
  [[rv('a', 'CHANGES_REQUESTED', 1), rv('a', 'DISMISSED', 2)], 0],
  [[rv('a', 'PENDING', 1)], 0],
  [[rv('a', 'PENDING', 1)], 3],
  // 순서가 뒤집혀 들어와도 제출 시각순으로 본다, 같은 시각은 들어온 순서
  [[rv('a', 'APPROVED', 5), rv('a', 'CHANGES_REQUESTED', 2)], 0],
  [[rv('a', 'APPROVED', 2), rv('a', 'CHANGES_REQUESTED', 2)], 0],
  [[rv('a', 'approved', 1), rv('b', 'Changes_Requested', 2)], 0],
  [[rv('a', null, 1), rv('b', undefined, 2), rv('c', '', 3)], 1],
  [[rv('a', 'APPROVED', null), rv('a', 'DISMISSED', 1)], 0],
  [[{ state: 'APPROVED' }, { user: null, state: 'COMMENTED' }], 0],
  [[{ user: { login: 5 }, state: 'APPROVED' }, { user: { login: '5' }, state: 'CHANGES_REQUESTED' }, { user: { login: 5 }, state: 'DISMISSED' }], 0],
  [[rv('a', 'ı', 1)], 0],
  [[rv('a', 'STALE', 1)], 0],
  [[rv('a', 5, 1)], 0],
  [[null], 0],
  [[5, 'x'], 0],
  [[rv('a', 'APPROVED', 1)], 0.5]
]

const CHECK_RUNS: unknown[] = [
  { status: 'queued' },
  { status: 'in_progress' },
  { status: 'completed', conclusion: 'success' },
  { status: 'completed', conclusion: 'skipped' },
  { status: 'completed', conclusion: 'neutral' },
  { status: 'completed', conclusion: 'stale' },
  { status: 'completed', conclusion: 'action_required' },
  { status: 'completed', conclusion: 'failure' },
  { status: 'completed', conclusion: 'cancelled' },
  { status: 'completed', conclusion: 'timed_out' },
  { status: 'completed', conclusion: 'startup_failure' },
  { status: 'completed', conclusion: null },
  { status: 'completed' },
  { status: 'COMPLETED', conclusion: 'success' },
  { status: 5, conclusion: 'success' },
  {},
  { conclusion: 'success' }
]

const CHECK_LISTS: string[][] = [[], ['success', 'failure', 'pending'], ['success', 'pending'], ['success', 'neutral'], ['neutral', 'neutral'], ['neutral'], ['none'], ['pending', 'failure'], ['success']]

const RUNS_DATA = {
  check_runs: [
    { name: 'build', status: 'completed', conclusion: 'success', html_url: 'https://github.com/octo/hello/runs/1', started_at: '2026-01-01T00:00:00Z', completed_at: '2026-01-01T00:01:05Z' },
    { name: 'lint', status: 'in_progress', conclusion: null, started_at: '2026-01-01T00:00:00Z' },
    { name: 'e2e', status: 'completed', conclusion: 'failure' },
    { status: 'completed', conclusion: 'neutral', html_url: '', started_at: '2026-01-01T00:00:10Z', completed_at: '2026-01-01T00:00:05Z' },
    { name: 'same', status: 'completed', conclusion: 'success', started_at: '2026-01-01T00:00:10Z', completed_at: '2026-01-01T00:00:10Z' }
  ]
}
const STATUS_DATA = { statuses: [{ context: 'ci/legacy', state: 'success', target_url: 'https://ci.example/1' }, { context: 'ci/err', state: 'error', target_url: null }, { state: 'pending' }] }

const RUNS_PAGES: unknown[] = [
  RUNS_DATA,
  {},
  { check_runs: null },
  { check_runs: [] },
  { check_runs: [null] },
  { check_runs: [5, 'x'] },
  { check_runs: {} }
]
const STATUS_PAGES: unknown[] = [STATUS_DATA, {}, { statuses: null }, { statuses: [null] }, { statuses: 3 }, { statuses: [{ context: 'a', state: 'success', target_url: 'u' }] }]

const WORKFLOW_RUN = {
  id: 77,
  name: 'CI',
  display_title: 'fix: x',
  head_branch: 'main',
  head_sha: SHA1,
  event: 'push',
  status: 'completed',
  conclusion: 'success',
  run_number: 12,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:05:00Z',
  html_url: 'https://github.com/o/r/actions/runs/77'
}
const JOBS = {
  jobs: [
    { id: 1, name: 'test', status: 'completed', conclusion: 'cancelled', started_at: '2026-01-01T00:00:00Z', completed_at: '2026-01-01T01:02:03Z', html_url: 'https://github.com/o/r/job/1' },
    { id: 2, name: 'deploy', status: 'queued', conclusion: null },
    { id: 3, status: null, conclusion: '', started_at: 'x', completed_at: 'y' },
    {}
  ]
}

const REPO = (owner: string, name: string, pushed: unknown, over: Record<string, unknown> = {}): Record<string, unknown> => ({
  name,
  full_name: `${owner}/${name}`,
  owner: { login: owner, type: 'User' },
  private: false,
  fork: false,
  archived: false,
  default_branch: 'main',
  description: null,
  pushed_at: pushed,
  html_url: `https://github.com/${owner}/${name}`,
  ...over
})

const REPOS: unknown[] = [
  REPO('octo', 'a', '2026-01-01T00:00:00Z'),
  REPO('org', 'b', '2026-03-01T00:00:00.500Z', { owner: { login: 'org', type: 'Organization' }, private: true, fork: true, archived: true, default_branch: 'dev', description: '설명' }),
  REPO('octo', 'c', null),
  REPO('octo', 'd', 'garbage'),
  REPO('octo', 'e', 12345),
  REPO('octo', 'f', '2026-02-01T00:00:00+09:00', { full_name: '', default_branch: '', description: '' }),
  REPO('octo', 'g', '2026-02-01T00:00:00Z', { private: 'true', fork: 1, archived: null, owner: { login: 'octo', type: 'Bot' } }),
  REPO('octo', 'h', '2026-02-01T00:00:00Z', { name: '' }),
  REPO('octo', 'i', '2026-02-01T00:00:00Z', { html_url: 5 }),
  REPO('octo', 'j', '2026-02-01T00:00:00Z', { owner: null }),
  REPO('octo', 'k', '2026-02-01T00:00:00Z', { owner: { login: '' } }),
  REPO('octo', 'l', '2026-02-01T00:00:00Z', { full_name: 5, description: 5 }),
  null,
  'str',
  5,
  [],
  {},
  REPO('octo', 'm', '2026-01-01T00:00:00Z')
]

const REMOTE_URLS: unknown[] = [
  'https://api.github.com/repos/octo/hello',
  'https://api.github.com/repos/Octo%20Cat/he%2Fllo',
  'https://api.github.com/repos/한글/%ED%95%9C',
  'https://api.github.com/repos/a/b/c',
  'https://api.github.com/repos/a/',
  'https://api.github.com/repos//b',
  'https://api.github.com/repos/a',
  'http://api.github.com/repos/a/b',
  'https://api.github.com/repos/a/b?x=1',
  'https://api.github.com/repos/a/b/',
  'https://api.github.com/repos/%E0%A4%A/b',
  'https://api.github.com/repos/%/b',
  'https://api.github.com/repos/%ff/b',
  'https://api.github.com/repos/%C3%28/b',
  'https://api.github.com/repos/a%0Ab/c',
  'https://api.github.com/repos/a\nb/c',
  'https://api.github.com/repos/a/b\n',
  'https://API.github.com/repos/a/b',
  'https://api.github.com/repos/a b/c',
  '',
  null,
  5
]

const SEARCH_ITEMS = (n: number): Record<string, unknown>[] =>
  Array.from({ length: n }, (_, i) => ({
    ...ISSUE(i + 1, { repository_url: i % 2 ? 'https://api.github.com/repos/Octo/Hello' : 'https://api.github.com/repos/other/repo', updated_at: `2026-0${(i % 9) + 1}-01T00:00:00Z` }),
    ...(i % 3 === 0 ? { pull_request: { merged_at: null }, draft: i % 2 === 0 } : {})
  }))

export function ghMapCases(): MapCase[] {
  const out: MapCase[] = []
  for (const p of PULL_VARIANTS) out.push(c('mapPull', p))
  out.push(c('mapPull', null), c('mapPull', 5), c('mapPull', 'x'), c('mapPull', []))
  out.push(c('mapPull', raw('{"number":1,"title":"a","title":"b","labels":[{"name":"\\ud800x","color":"#\\udc00"}]}')))
  for (const p of PULL_VARIANTS) out.push(c('mapSearchPull', p))
  out.push(c('mapSearchPull', ISSUE(1, { pull_request: { merged_at: '2026-01-01T00:00:00Z' } })), c('mapSearchPull', null))
  for (const i of ISSUE_VARIANTS) out.push(c('mapIssue', i), c('getIssue', i), c('mapSearchPull', i))
  out.push(c('mapIssue', null), c('getIssue', null))
  for (const u of [null, {}, user('x'), { login: null, avatar_url: null }, { login: 5, avatar_url: false }, [], 'str', { login: '' }]) out.push(c('mapUser', u))
  for (const [reviews, req] of REVIEW_SETS) out.push(c('aggregateReviews', reviews, req))
  for (const r of CHECK_RUNS) out.push(c('checkRunState', r))
  for (const st of ['success', 'pending', 'error', 'failure', '', null, 5, 'SUCCESS']) out.push(c('commitStatusState', st))
  for (const l of CHECK_LISTS) out.push(c('aggregateChecks', l))
  for (const st of ['SUCCESS', 'FAILURE', 'ERROR', 'PENDING', 'EXPECTED', 'success', '', null, 3]) out.push(c('rollupState', st))
  for (const n of [0, 1, 3, 50]) out.push(c('rollupQuery', n))

  // 조회 결과 변환(가짜 client 로 실제 함수를 돌린다)
  const detailReviews = [rv('a', 'APPROVED', 1), rv('b', 'CHANGES_REQUESTED', 2), rv('b', 'DISMISSED', 3), rv('c', 'PENDING', 4), { user: null, state: 'COMMENTED', submitted_at: '2026-01-05T00:00:00Z' }, { user: user('d'), state: 'COMMENTED' }]
  const pullFull = PULL(7, { head: { ref: 'f', sha: SHA1 }, requested_reviewers: [user('r1')], requested_teams: [{}], body_html: '<p>본문</p>', mergeable: true, additions: 3, deletions: 1, changed_files: 2, comments: 4 })
  out.push(c('getPull', pullFull, detailReviews, RUNS_DATA, STATUS_DATA))
  out.push(c('getPull', PULL(8, { head: { ref: 'f' } }), [], RUNS_DATA, STATUS_DATA))
  out.push(c('getPull', PULL(9, { head: { ref: 'f', sha: SHA1 } }), [], { check_runs: null }, STATUS_DATA))
  out.push(c('getPull', PULL(10, { head: { sha: SHA1 }, mergeable: null, requested_reviewers: [user('x'), user('y')] }), [rv('a', 'COMMENTED', 1)], { check_runs: [] }, { statuses: [] }))
  out.push(c('getPull', PULL(11, { head: { sha: SHA1 }, requested_reviewers: 'ab' }), [], {}, {}))
  out.push(c('getPull', PULL(12, { head: { sha: SHA1 }, requested_reviewers: null, requested_teams: 'abc' }), [], {}, {}))
  out.push(c('getPull', PULL(13, { head: { sha: SHA1 } }), [null], {}, {}))
  out.push(c('getPull', PULL(14, { head: { sha: SHA1 } }), [{ submitted_at: '2026-01-01T00:00:00Z', state: 5 }], {}, {}))
  out.push(c('getPull', PULL(15, { head: { sha: SHA1 } }), { not: 'array' }, {}, {}))
  out.push(c('getPull', null, [], {}, {}))
  const pulls = [PULL(1, { head: { ref: 'a', sha: SHA1 }, requested_reviewers: [user('x')] }), PULL(2, { head: { ref: 'b' } }), PULL(3, { head: { ref: 'c', sha: SHA1 }, requested_teams: [{}], merged_at: '2026-01-01T00:00:00Z' })]
  out.push(c('listPulls', pulls, [rv('a', 'APPROVED', 1)], RUNS_DATA, STATUS_DATA))
  out.push(c('listPulls', pulls, [], { check_runs: [] }, { statuses: [] }))
  out.push(c('listPulls', pulls, [], { check_runs: [null] }, STATUS_DATA))
  out.push(c('listPulls', pulls, [null], RUNS_DATA, STATUS_DATA))
  out.push(c('listPulls', pulls, { not: 'array' }, RUNS_DATA, STATUS_DATA))
  out.push(c('listPulls', [], [], {}, {}), c('listPulls', null, [], {}, {}), c('listPulls', [null], [], {}, {}), c('listPulls', { x: 1 }, [], {}, {}))
  out.push(c('listPullsAnon', pulls), c('listPullsAnon', [PULL(5, { labels: 'x' })]))
  const filtered = [ISSUE(1, { pull_request: { merged_at: null } }), ISSUE(2), ISSUE(3, { pull_request: {}, state: 'closed' }), ISSUE(4, { pull_request: 0 })]
  out.push(c('listPullsFiltered', filtered), c('listPullsFiltered', []), c('listPullsFiltered', [null]))
  out.push(c('listIssues', filtered), c('listIssues', []), c('listIssues', [null]), c('listIssues', null), c('listIssues', [ISSUE(1, { labels: 'x' })]))
  out.push(c('searchPullsByKey', { items: filtered }), c('searchPullsByKey', {}), c('searchPullsByKey', { items: null }), c('searchPullsByKey', null), c('searchPullsByKey', { items: [null] }), c('searchPullsByKey', { items: 'x' }))
  const files = [
    { filename: 'a.ts', status: 'modified', additions: 3, deletions: 1, patch: '@@ -1 +1 @@\n-a\n+b' },
    { filename: 'new.ts', previous_filename: 'old.ts', status: 'renamed', additions: 0, deletions: 0 },
    { filename: 'bin.png', status: 'added' },
    { filename: 'weird', status: 'mystery', patch: '' },
    { filename: 'p', status: 'ADDED', patch: null, previous_filename: '' },
    { status: 'removed', additions: null, deletions: 5 },
    { filename: 'u', status: 'unchanged' }
  ]
  out.push(c('getPullFiles', files), c('getPullFiles', []), c('getPullFiles', [null]), c('getPullFiles', {}), c('getPullFiles', null), c('getPullFiles', [5]))
  const commits = [
    { sha: 'a'.repeat(40), commit: { message: 'm1\n\nbody', author: { name: 'Alice', date: '2026-01-01T00:00:00Z' }, committer: { date: '2026-01-02T00:00:00Z' } }, author: user('alice') },
    { sha: 'b'.repeat(40), commit: { message: 'm2', author: { name: null, date: null }, committer: { date: '2026-01-02T00:00:00Z' } }, author: null },
    { sha: 'c'.repeat(40), commit: null, author: { login: 'ghost-login' } },
    { sha: 'd'.repeat(40) },
    { commit: { author: { name: '', date: 'x' } }, author: {} },
    { sha: 5, commit: 'x', author: 'y' }
  ]
  out.push(c('getPullCommits', commits), c('getPullCommits', []), c('getPullCommits', [null]))
  out.push(c('getComments', [{ user: user('a'), body_html: '<p>hi</p>', created_at: '2026-01-01T00:00:00Z', html_url: 'https://x/1' }, { user: null }, {}, { body_html: null, created_at: 'bad' }]))
  out.push(c('getComments', []), c('getComments', [null]), c('getComments', {}))
  out.push(c('getFilters', [{ name: 'bug', color: 'ff0000' }, { name: 'a', color: '#00f' }], [user('a'), null, {}]), c('getFilters', [], []), c('getFilters', [null], []), c('getFilters', [], [null]), c('getFilters', null, []), c('getFilters', [], 'x'))
  out.push(c('fetchCheckDetails', RUNS_DATA, STATUS_DATA))
  for (const r of RUNS_PAGES) out.push(c('fetchCheckDetails', r, STATUS_DATA))
  for (const st of STATUS_PAGES) out.push(c('fetchCheckDetails', RUNS_DATA, st))
  out.push(c('fetchCheckDetails', null, STATUS_DATA), c('fetchCheckDetails', RUNS_DATA, null))
  out.push(c('mapRun', WORKFLOW_RUN), c('mapRun', {}), c('mapRun', null), c('mapRun', { ...WORKFLOW_RUN, status: 'in_progress', conclusion: null, id: '77', run_number: null }), c('mapRun', { ...WORKFLOW_RUN, conclusion: 'cancelled' }), c('mapRun', { ...WORKFLOW_RUN, status: '', name: null }))
  out.push(c('listRuns', { workflow_runs: [WORKFLOW_RUN, { id: 78, status: 'queued' }] }), c('listRuns', {}), c('listRuns', null), c('listRuns', { workflow_runs: [null] }), c('listRuns', { workflow_runs: 'x' }))
  out.push(c('listJobs', JOBS), c('listJobs', {}), c('listJobs', { jobs: [null] }), c('listJobs', null), c('listJobs', { jobs: {} }))
  const roll = (s: string | null | undefined): unknown => (s === undefined ? null : { statusCheckRollup: s === null ? null : { state: s } })
  out.push(c('fetchCommitChecks', { repository: { c0: roll('SUCCESS'), c1: roll('FAILURE'), c2: roll('PENDING'), c3: roll(null), c4: null } }, 5))
  out.push(c('fetchCommitChecks', { repository: null }, 2), c('fetchCommitChecks', {}, 2), c('fetchCommitChecks', null, 2), c('fetchCommitChecks', { repository: 'x' }, 1), c('fetchCommitChecks', { repository: { c0: { statusCheckRollup: { state: 5 } } } }, 1))
  out.push(c('fetchCommitChecks', { repository: { c0: roll('ERROR'), c1: roll('EXPECTED') } }, 3), c('fetchCommitChecks', { repository: [] }, 2))

  // 접근 가능한 저장소
  for (const r of REPOS) out.push(c('mapAccessibleRepo', r))
  out.push(c('accessibleRepos', REPOS.slice(0, 6), REPOS.slice(6)))
  out.push(c('accessibleRepos', [REPO('a', 'x', '2026-01-01T00:00:00Z'), REPO('a', 'y', '2026-01-01T00:00:00Z'), REPO('a', 'z', '2026-01-01T00:00:01Z')]))
  out.push(c('accessibleRepos', { not: 'array' }), c('accessibleRepos', []), c('accessibleRepos', null), c('accessibleRepos', [], [REPO('b', 'q', '2026-05-01T00:00:00Z')]))

  // Launchpad
  for (const u of REMOTE_URLS) out.push(c('slugFromRepositoryUrl', u))
  out.push(c('sections'))
  const remotes = { r1: [{ owner: 'octo', repo: 'hello' }], r2: [{ owner: 'OCTO', repo: 'Hello' }, { owner: 'x', repo: 'y' }], r3: [], r4: [{ owner: 'Other', repo: 'REPO' }] }
  const items = SEARCH_ITEMS(8)
  out.push(c('fetchLaunchpad', { authored: items, review: items.slice(2, 6), issues: items.slice(4) }, remotes))
  out.push(c('fetchLaunchpad', { authored: [...items, ...items], review: [], issues: [] }, remotes))
  out.push(c('fetchLaunchpad', { authored: [{ ...ISSUE(1), repository_url: 'https://api.github.com/repos/octo/hello', number: undefined }, { ...ISSUE(2), repository_url: 'https://api.github.com/repos/octo/hello', number: null }, { ...ISSUE(3), repository_url: 5 }], review: [], issues: [] }, remotes))
  out.push(c('fetchLaunchpad', { authored: [null], review: [], issues: [] }, remotes))
  out.push(c('fetchLaunchpad', { authored: [], review: [], issues: [] }, {}))
  out.push(c('fetchLaunchpad', { authored: [{ ...ISSUE(1), repository_url: 'https://api.github.com/repos/octo/hello', labels: 'x' }], review: [], issues: [] }, remotes))
  out.push(c('fetchLaunchpad', { authored: [{ ...ISSUE(1), repository_url: 'https://api.github.com/repos/Straße/x' }, { ...ISSUE(2), repository_url: 'https://api.github.com/repos/İ/x' }], review: [], issues: [] }, { s1: [{ owner: 'strasse', repo: 'x' }], s2: [{ owner: 'i̇', repo: 'x' }], s3: [{ owner: 'STRASSE', repo: 'X' }] }))
  for (const d of [{ items: [] }, { items: [ISSUE(1)], incomplete_results: true }, { items: [], total_count: 1001 }, { items: [], total_count: 1000 }, { items: [], total_count: '2000' }, { items: [], total_count: null, incomplete_results: 0 }, {}, null, { items: [], total_count: {} }, { items: [], incomplete_results: 'no' }]) out.push(c('searchPage', d))
  return out
}

// ── 날짜 ──────────────────────────────────────────────────────────────────

/** `Date.parse` 에 먹일 문자열(시간대에 따라 달라지는 것도 포함 — 골든은 'tz' 로 적는다) */
export function dateStrings(): string[] {
  const out = new Set<string>([
    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.5Z', '2026-01-01T00:00:00.123456Z', '2026-01-01T00:00:00.Z', '2026-01-01T00:00:00,5Z',
    '2026-01-01T00:00Z', '2026-01-01T00Z', '2026-01-01', '2026-01', '2026', '2026-1-1', '2026-01-01T0:00Z', '2026-01-01T00:00:00+09:00', '2026-01-01T00:00:00-09:30',
    '2026-01-01T00:00:00+0900', '2026-01-01T00:00:00+09', '2026-01-01T00:00:00+9:00', '2026-01-01T00:00:00+24:00', '2026-01-01T00:00:00+23:59', '2026-01-01T00:00:00+09:60',
    '2026-01-01T00:00:00-0000', '2026-01-01T00:00:00+00:00', '2026-01-01T24:00:00Z', '2026-01-01T24:00:00.000Z', '2026-01-01T24:00:00.5Z', '2026-01-01T24:00:01Z', '2026-01-01T24:01Z',
    '2026-01-01T23:60Z', '2026-01-01T23:59:60Z', '2026-01-01T23:59:59Z', '2026-02-29', '2026-02-30', '2026-02-31', '2024-02-29', '2026-04-31', '2026-13-01', '2026-00-10', '2026-01-00', '2026-01-32',
    '2026-01-01t00:00:00z', '2026-01-01 00:00:00Z', '2026-01-01T00:00:00 Z', '  2026-01-01', '2026-01-01T00:00:00Z ', '2026-01-01T00:00:00GMT', '2026-01-01T00:00:00+09:00:00',
    '2026-01-01T00:00:00', '2026-01-01T00:00', '2026-01-01 00:00:00', '2026-01-01Z', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00.999Z', '1969-12-31T23:59:59.500Z', '1969-12-31T23:59:59Z',
    '0000-01-01', '0001-01-01T00:00:00Z', '9999-12-31T23:59:59.999Z', '-000001-01-01', '+002026-01-01', '275760-09-13', '+275760-09-13T00:00:00Z', '+275760-09-13T00:00:00.001Z', '',
    'Mon, 01 Jan 2026 00:00:00 GMT', 'Jan 1 2026', '1/2/2026', 'x', '12345', '１２３４', '2026-01-01T00:00:00\u0000Z', '2026-01-01T００:00Z', '٢٠٢٦-01-01',
    // 시각 없이 오프셋만·짧은 날짜에 시각·24시·확장 연도의 경계(파서의 모든 가지)
    '2026-01-01+09:00', '2026-01-01-09:00', '2026+09:00', '2026-01+0900', '2026-01-01+0900', '2026T00:00Z', '2026-01T00:00Z', '2026-01-01T24:00Z', '2026-01-01T24:00:00+09:00',
    '-000000-01-01', '+000000-01-01', '-000000-01-01T00:00:00Z', '+002026-01-01T00:00:00', '-000001-01-01T00:00:00', '+002026-01-01T00:00:00Z', '-000001-01-01T00:00:00Z', '+0020260-01-01', '+29349-01-26',
    '-271821-04-20T00:00:00Z', '-271821-04-19T23:59:59Z', '+275760-09-13T00:00:00Z', '+275760-09-13T00:00:01Z', '+275760-09-12T23:59:59.999Z',
    '2026-01-01T00:00:00+09:00Z', '2026Z', '2026-01Z', '2026-02-30T00:00:00Z', '2026-02-31T23:59:59.999+09:00', '2026-06-31', '2026-01-01T00:00:00.0000000001Z',
    '2026-01-01T00:00:00-0000', '2026-01-01T00:00:00+0000', '2026-01-01T00:00:00.000+0000', '2026-01-01T00:00:00.000-1200', '2026-01-01T00:00:00.000+1400',
    // 기존 TS 테스트(jiraService·launchpadService)가 쓰는 시각 문자열
    '2026-10-19T09:00:00.000+0900', '2026-10-01T09:00:00.000+0900', '2026-10-17T00:00:00Z', '2026-10-22T14:00:00Z', '2026-10-21T23:00:00.000+0900', '2026/10/30', '2026-01-02T00:00:00.000+0000', '2026-01-01T00:00:00.000+0000',
    '2026-01-01T10:00:00.000+0900', '2026-01-03T00:00:00Z'
  ])
  for (const y of ['0999', '1900', '1999', '2000', '2038', '2100']) for (const m of ['01', '02', '12']) for (const d of ['01', '28', '29', '30', '31']) out.add(`${y}-${m}-${d}T12:34:56.789Z`)
  for (const off of ['Z', '+00:00', '-00:00', '+01:00', '-12:00', '+14:00', '+05:30', '+0530', '-0800']) out.add(`2026-06-15T01:02:03.004${off}`)
  return [...out]
}

/** `new Date(ms).toISOString()` 에 먹일 값 */
export const ISO_MS: number[] = [0, -1, 1, 999, 1000, 86_399_999, -86_400_000, 1_767_225_600_000, 951_782_400_000, -62_167_219_200_000, -62_198_755_200_000, 253_402_300_799_999, 253_402_300_800_000, 8.64e15, -8.64e15, 8.64e15 + 1, -8.64e15 - 1, 1.9, -1.9, 0.5, 1e300, -0]


// ── Jira ──────────────────────────────────────────────────────────────────

const text = (t: unknown, marks?: unknown[]): Record<string, unknown> => ({ type: 'text', text: t, ...(marks ? { marks } : {}) })
const doc = (...content: unknown[]): Record<string, unknown> => ({ type: 'doc', version: 1, content })
const para = (...content: unknown[]): Record<string, unknown> => ({ type: 'paragraph', content })

const ADF_DOCS: unknown[] = [
  doc(
    { type: 'heading', attrs: { level: 2 }, content: [text('제목')] },
    { type: 'paragraph', content: [text('a'), { type: 'hardBreak' }, text('b', [{ type: 'em' }, { type: 'code' }])] },
    { type: 'bulletList', content: [{ type: 'listItem', content: [para(text('x'))] }] },
    { type: 'orderedList', attrs: { order: 3 }, content: [{ type: 'listItem', content: [para(text('y'))] }] },
    { type: 'codeBlock', content: [text('if (a < b) {}')] },
    { type: 'blockquote', content: [para(text('q'))] },
    { type: 'table', content: [{ type: 'tableRow', content: [{ type: 'tableHeader', content: [para(text('h'))] }, { type: 'tableCell', content: [para(text('c'))] }] }] },
    { type: 'rule' }
  ),
  doc(
    para(
      text('<script>alert(1)</script>'),
      text('js', [{ type: 'link', attrs: { href: 'javascript:alert(1)' } }]),
      text('data', [{ type: 'link', attrs: { href: 'data:text/html,<script>x</script>' } }]),
      text('ok', [{ type: 'link', attrs: { href: 'https://x.atlassian.net/a?b="1"&c=2' } }]),
      { type: 'mention', attrs: { text: '@<img src=x onerror=1>' } },
      { type: 'heading', attrs: { level: '1><script>' }, content: [text('h')] }
    )
  ),
  doc(para(text('all', [{ type: 'strong' }, { type: 'em' }, { type: 'code' }, { type: 'strike' }, { type: 'underline' }, { type: 'subsup', attrs: { type: 'sub' } }, { type: 'subsup', attrs: { type: 'sup' } }, { type: 'subsup' }, { type: 'unknown' }]))),
  doc(para(text('l', [{ type: 'link', attrs: { href: 'http://example.com:8080/a b?x=\'1\'#f' } }, { type: 'link', attrs: { href: 'relative/path' } }, { type: 'link', attrs: { href: 5 } }, { type: 'link' }, { type: 'link', attrs: 'x' }, null, 5, 'str', [], { type: 5 }]))),
  doc(para(text('u', [{ type: 'link', attrs: { href: 'https://user:pw@example.com/' } }, { type: 'link', attrs: { href: 'HTTPS://EXAMPLE.COM/%7Efoo' } }, { type: 'link', attrs: { href: 'https://例え.jp/パス' } }, { type: 'link', attrs: { href: 'ftp://x/' } }, { type: 'link', attrs: { href: 'file:///c:/x' } }, { type: 'link', attrs: { href: 'http://[::1]/' } }, { type: 'link', attrs: { href: 'http://0x7f.1/' } }]))),
  doc(
    { type: 'taskList', content: [{ type: 'taskItem', attrs: { state: 'DONE' }, content: [text('done')] }, { type: 'taskItem', attrs: { state: 'TODO' }, content: [text('todo')] }, { type: 'taskItem', content: [] }] },
    { type: 'panel', attrs: { panelType: 'info' }, content: [para(text('p'))] },
    { type: 'expand', attrs: { title: '<b>제목</b>' }, content: [para(text('e'))] },
    { type: 'nestedExpand', content: [para(text('n'))] },
    { type: 'expand', attrs: { title: 5 }, content: [] }
  ),
  doc(para({ type: 'mention', attrs: { text: '@홍길동' } }, { type: 'mention' }, { type: 'mention', attrs: { text: '' } }, { type: 'emoji', attrs: { text: '😀', shortName: ':grin:' } }, { type: 'emoji', attrs: { shortName: ':grin:' } }, { type: 'emoji' }, { type: 'status', attrs: { text: 'DONE<' } }, { type: 'status' })),
  doc(para(...[0, 1_767_225_600_000, -1, 86_399_999, 1.9, 8.64e15, -8.64e15, '1767225600000', '0x10', '', ' ', null, true, false, [5], [], {}, 'x'].map((ts) => ({ type: 'date', attrs: { timestamp: ts } })), { type: 'date' }, { type: 'date', attrs: null })),
  doc(para({ type: 'inlineCard', attrs: { url: 'https://github.com/o/r/pull/1' } }, { type: 'blockCard', attrs: { url: 'javascript:alert(1)' } }, { type: 'inlineCard', attrs: { url: 'https://a.example/?q="x"&r=<y>' } }, { type: 'inlineCard' }, { type: 'inlineCard', attrs: { url: 5 } })),
  doc({ type: 'mediaSingle', content: [{ type: 'media', attrs: { id: 'x' } }] }, { type: 'mediaGroup', content: [{ type: 'media' }, { type: 'media' }] }, { type: 'media' }),
  doc({ type: 'weird', content: [para(text('자식만'))] }, { content: [para(text('type 없음'))] }, { type: 5, content: [para(text('type 숫자'))] }, { type: ['paragraph'], content: [] }),
  ...[0, 1, 2, 3, 4, 5, 6, 7, '2', [3], null, true, 1.5, '  4 ', 'x', NaN, -1, 1e21].map((level) => doc({ type: 'heading', attrs: { level }, content: [text('h')] })),
  ...[0, 1, 2, 3, '5', null, 1e6, 999_999, 2.5, [4], '0x10', true, -3, '2'].map((order) => doc({ type: 'orderedList', attrs: { order }, content: [] })),
  doc(null, 7, para(null, 'str', text('ok', [null, 5, { type: 'strong' }, { type: 'link' }]), { type: 'text' }), { type: 'codeBlock', content: [null, text('x'), 3, { text: 5 }, 'y'] }, { type: 'bulletList', content: { not: 'array' } }, { type: 'paragraph', content: [{ type: 'text', text: 'm', marks: 'strong' }] }),
  doc(para(text(5)), para(text(null)), para({ type: 'text' }), para(text('\u0000제어\u001f\u007f문자</p><img src=x onerror=1>"\'&'))),
  doc({ type: 'codeBlock', content: 'abc' }, { type: 'codeBlock' }, { type: 'codeBlock', content: [text('a'), text('b&<')] }),
  { type: 'paragraph', content: [text('doc 가 아닌 단독 노드')] },
  text('단독 텍스트'),
  { type: 'doc' },
  { type: 'doc', content: [] },
  {},
  [],
  [{ type: 'paragraph' }],
  null,
  'plain',
  5,
  true,
  { type: 'doc', content: [{ type: 'listItem', content: [para(text('중첩'))] }] },
  // 깊이: 중첩이 깊어도 멈춘다(JSON 깊이 상한 안에서)
  ...[1, 10, 20, 30].map((n) => Array.from({ length: n }).reduce<unknown>((inner) => ({ type: 'blockquote', content: [inner] }), para(text('끝')))),
  doc(...Array.from({ length: 30 }, (_, i) => para(text(`문단 ${i}`))))
]

const jiraUser = (name: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({ displayName: name, ...extra })
const ISSUE_RAW = (n: number, f: Record<string, unknown> = {}, top: Record<string, unknown> = {}): Record<string, unknown> => ({
  id: String(10_000 + n),
  key: `ABC-${n}`,
  fields: {
    summary: `요약 ${n}`,
    status: { name: '진행 중', statusCategory: { key: 'indeterminate' } },
    issuetype: { name: 'Bug' },
    priority: { name: 'High' },
    assignee: jiraUser('홍길동', { avatarUrls: { '48x48': 'https://avatar.example/48', '24x24': 'https://avatar.example/24' } }),
    updated: '2026-01-02T03:04:05.000+0900',
    duedate: '2026-03-04',
    reporter: jiraUser('김철수', { avatarUrls: { '32x32': 'http://avatar.example/32' } }),
    created: '2026-01-01T00:00:00.000+0900',
    description: doc(para(text('설명'))),
    ...f
  },
  ...top
})

const AVATAR_USERS: unknown[] = [
  { avatarUrls: { '48x48': 'https://x/48', '24x24': 'https://x/24' } },
  { avatarUrls: { '24x24': 'https://x/24' } },
  { avatarUrls: { '32x32': 'https://x/32', '24x24': 'https://x/24' } },
  { avatarUrls: { '48x48': 'javascript:alert(1)' } },
  { avatarUrls: { '48x48': 'HTTP://x/48' } },
  { avatarUrls: { '48x48': 'hTtPs://x/48' } },
  { avatarUrls: { '48x48': `https://x/${'a'.repeat(2041)}` } },
  { avatarUrls: { '48x48': `https://x/${'a'.repeat(2040)}` } },
  { avatarUrls: { '48x48': `https://x/${'😀'.repeat(1030)}` } },
  { avatarUrls: { '48x48': '', '32x32': 'https://x/32' } },
  { avatarUrls: { '48x48': 5, '32x32': 'https://x/32' } },
  { avatarUrls: [] },
  { avatarUrls: 'x' },
  { avatarUrls: null },
  {},
  null,
  'x',
  5,
  []
]

const FIELD_LIST: unknown[] = [
  { id: 'customfield_10015', name: 'Start date', schema: { type: 'date' } },
  { id: 'customfield_10016', name: 'start date ', schema: { type: 'string' }, scope: { type: 'PROJECT' } },
  { id: 'customfield_10017', name: 'Start Date', schema: { type: 'datetime' }, scope: { project: { id: '1' } } },
  { id: 'customfield_10018', name: '시작일', schema: { type: 'date' } },
  { id: 'customfield_10019', name: 'Target start', schema: {} },
  { id: 'customfield_10020', name: 'End date', schema: { type: 'date' } },
  { id: 'customfield_10021', name: '종료일', schema: { type: 'date' } },
  { id: 'customfield_10022', name: 'target end' },
  { id: 'customfield_10023', name: 'Target End', scope: null },
  { id: 'customfield_10015', name: 'Start date', schema: { type: 'date' } },
  { id: 'duedate', name: 'Due date' },
  { id: 'customfield_', name: 'End date' },
  { id: 'customfield_1x', name: 'End date' },
  { id: 'customfield_１２', name: 'End date' },
  { id: 'customfield_10024\n', name: 'End date' },
  { id: 'customfield_10025', name: '\u00a0Start date\u00a0' },
  { id: 'customfield_10026', name: 'START DATE' },
  null,
  5,
  { id: 5, name: 'Start date' },
  { id: 'customfield_9', name: 5 }
]
const MANY_FIELDS = Array.from({ length: 12 }, (_, i) => ({ id: `customfield_${20_000 + i}`, name: 'Start date', schema: { type: i % 2 ? 'date' : 'string' } }))

const JIRA_ISSUES: unknown[] = [
  ISSUE_RAW(1),
  ISSUE_RAW(2, { assignee: null, priority: null, duedate: 'garbage', updated: 5, summary: null }),
  ISSUE_RAW(3, { duedate: '2026-03-04T00:00:00.000+0900', status: { name: 'x' } }),
  ISSUE_RAW(4, { duedate: '２０２６-03-04', assignee: { displayName: 5, avatarUrls: { '48x48': 'https://x/48' } } }),
  ISSUE_RAW(5, { duedate: '2026-3-4' }),
  ISSUE_RAW(6, { fields: 'x' }),
  { key: 5 },
  { fields: null },
  {},
  null,
  'x',
  [],
  5
]

const DETAIL_BASE = (extra: Record<string, unknown> = {}): Record<string, unknown> => ISSUE_RAW(7, { customfield_10015: '2026-02-01T00:00:00.000+0900', customfield_10016: null, customfield_10020: '2026-02-09', ...extra }, { renderedFields: { description: '<p>서버 <b>렌더</b></p>' } })

export function jiraMapCases(): MapCase[] {
  const out: MapCase[] = []
  for (const d of ADF_DOCS) out.push(c('adfToHtml', d))
  out.push(c('adfToHtml', raw('{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a\\ud83d\\ude00b\\ud800c","marks":[{"type":"link","attrs":{"href":"https://x.example/\\ud800"}}]},{"type":"text","type":"paragraph","text":"dup"}]}]}')))
  for (const n of [1, 49, 50, 51, 52, 60]) out.push(c('adfDeep', n))
  out.push(c('adfBig', 30, 50_000), c('adfBig', 30, 33_333), c('adfBig', 1, 1_000_000), c('adfBig', 2, 600_000), c('adfBigEmoji', 500_000))
  for (const u of AVATAR_USERS) out.push(c('avatarOf', u))
  for (const i of JIRA_ISSUES) out.push(c('mapSummary', i))
  for (const kind of ['cloud', 'server']) {
    out.push(c('myself', kind, { displayName: '홍', name: 'hong', emailAddress: 'h@x' }), c('myself', kind, { name: 'hong', emailAddress: 'h@x' }), c('myself', kind, { emailAddress: 'h@x' }), c('myself', kind, {}), c('myself', kind, null), c('myself', kind, { displayName: 5, name: 'n' }))
    out.push(c('getMyId', kind, { accountId: '712020:abc', name: 'hong', displayName: '홍길동' }), c('getMyId', kind, { accountId: '712020:abc' }), c('getMyId', kind, {}), c('getMyId', kind, null), c('getMyId', kind, { accountId: 'a\u0000b', name: 'a\nb' }), c('getMyId', kind, { accountId: 'x'.repeat(255), name: 'x'.repeat(255) }), c('getMyId', kind, { accountId: 'x'.repeat(256), name: 'x'.repeat(256) }), c('getMyId', kind, { accountId: '한글 이름', name: '한글 이름', displayName: '😀'.repeat(150) }), c('getMyId', kind, { accountId: 'a', name: 'a', displayName: 'b'.repeat(201) }), c('getMyId', kind, { accountId: 'a\u007f', name: 'a\u007f' }))
    const users = [
      { accountId: 'a1', accountType: 'atlassian', name: 'n1', displayName: '하나' },
      { accountId: 'a2', accountType: 'app', name: 'n2', displayName: '앱' },
      { accountId: 'a3', accountType: 'customer', name: 'n3' },
      { accountId: 'a 4', accountType: 'atlassian', name: 'n 4' },
      { accountId: '', accountType: 'atlassian', name: '' },
      { accountId: 'a6', accountType: 'atlassian', name: 'n6', displayName: 'x'.repeat(300) },
      null,
      5,
      { accountId: 'a'.repeat(129), accountType: 'atlassian', name: 'a'.repeat(129) },
      { accountId: 'a'.repeat(128), accountType: 'atlassian', name: 'a'.repeat(128) }
    ]
    out.push(c('searchUsers', kind, users), c('searchUsers', kind, []), c('searchUsers', kind, null), c('searchUsers', kind, {}), c('searchUsers', kind, Array.from({ length: 30 }, (_, i) => ({ accountId: `u${i}`, accountType: 'atlassian', name: `u${i}` }))))
    out.push(c('getTransitions', kind, { transitions: [{ id: '11', name: '진행', to: { name: '진행 중', statusCategory: { key: 'indeterminate' } } }, { id: '21', name: '완료', to: { name: '완료', statusCategory: { key: 'done' } } }, { id: '', name: 'x' }, { id: '5' }, { id: 7, name: 'n' }, null, 5, {}, { id: '9', name: 'n', to: null }] }), c('getTransitions', kind, {}), c('getTransitions', kind, null), c('getTransitions', kind, { transitions: 'x' }))
    const dates = { start: ['customfield_10015', 'customfield_10016'], end: ['customfield_10020'] }
    out.push(c('getIssue', kind, 'ABC-7', dates, DETAIL_BASE()))
    out.push(c('getIssue', kind, 'ABC-7', { start: [], end: [] }, DETAIL_BASE()))
    out.push(c('getIssue', kind, 'ABC-8', dates, DETAIL_BASE({ customfield_10015: null, customfield_10016: '2026-01-05', customfield_10020: 5, description: 'plain <text>\r\nline2 & more' })))
    out.push(c('getIssue', kind, 'ABC-9', dates, ISSUE_RAW(9, { description: null, reporter: null }, { renderedFields: null })))
    out.push(c('getIssue', kind, 'ABC-9', dates, ISSUE_RAW(9, { description: '   ' }, { renderedFields: { description: '' } })))
    out.push(c('getIssue', kind, 'ABC-9', dates, ISSUE_RAW(9, { description: '\u00a0' }, { renderedFields: { description: 5 } })))
    out.push(c('getIssue', kind, 'ABC-7', dates, DETAIL_BASE({ duedate: '2026-10-22', customfield_10015: '2026-10-01T09:00:00.000+0900', customfield_10016: '2026/10/30', customfield_10020: '2026-10-30T09:00:00.000+0900' })))
    out.push(c('getIssue', kind, 'A B/C?#한글', dates, DETAIL_BASE()), c('getIssue', kind, "AB'C()*!~", dates, DETAIL_BASE()), c('getIssue', kind, 'ABC-1', dates, null), c('getIssue', kind, 'ABC-1', dates, {}), c('getIssue', kind, 'ABC-1', dates, 5))
  }
  const cm = (n: number, body: unknown, rendered?: unknown): Record<string, unknown> => ({ author: jiraUser(`작성자${n}`, { avatarUrls: { '48x48': `https://a/${n}` } }), created: `2026-01-0${n}T00:00:00.000+0900`, body, ...(rendered === undefined ? {} : { renderedBody: rendered }) })
  const comments = [1, 2, 3, 4, 5, 6, 7].map((n) => cm(n, doc(para(text(`c${n}`))), `<p>r${n}</p>`))
  out.push(c('getComments', 'cloud', { comments: comments.slice(0, 3) }), c('getComments', 'cloud', { comments }), c('getComments', 'cloud', {}), c('getComments', 'cloud', null), c('getComments', 'cloud', { comments: 'x' }), c('getComments', 'cloud', { comments: [null, 5, {}, cm(1, 'plain text <b>')] }))
  out.push(c('getComments', 'server', { total: 3, comments: comments.slice(0, 3) }), c('getComments', 'server', { total: 7, comments: comments.slice(0, 5) }, { total: 7, comments: comments.slice(2, 7) }), c('getComments', 'server', { total: 5, comments: comments.slice(0, 5) }), c('getComments', 'server', { total: 6, comments: comments.slice(0, 5) }, { total: 6, comments: comments.slice(1, 6) }), c('getComments', 'server', {}), c('getComments', 'server', null), c('getComments', 'server', { total: '7', comments: comments }), c('getComments', 'server', { total: 7, comments: [] }, { total: 7, comments: [cm(1, 'x', '')] }), c('getComments', 'server', { total: 2, comments: [cm(1, 'a\nb', undefined), cm(2, 5, '<i>r</i>')] }), c('getComments', 'server', { total: 2, comments: [null, { body: 'z' }] }))

  // 개발 정보(issue, summary, 연동 종류별 PR 상세, 저장소 상세)
  const prSummary = { summary: { pullrequest: { byInstanceType: { GitHub: { count: 1 }, stash: {}, 'bad type': {}, ['x'.repeat(41)]: {}, '': {} } }, repository: { byInstanceType: { GitHub: {}, 'a.b-c_d': {}, '1': {}, '0': {} } } } }
  const prs = { detail: [{ pullRequests: [
    { name: 'PR 하나', url: 'https://github.com/o/r/pull/1', status: 'OPEN', author: { name: 'alice' }, repositoryName: 'o/r', source: { branch: 'feat' }, lastUpdate: '2026-01-02T00:00:00.000+0900' },
    { name: 'PR 둘', url: 'http://insecure/2', status: 'merged', author: { name: 'bob' }, repositoryName: 'o/r', source: { branch: 'fix' }, lastUpdate: 1_767_225_600_000 },
    { name: 'PR 셋', url: 'https://github.com/o/r/pull/3', status: 'DECLINED', lastUpdate: 'garbage' },
    { name: 'PR 하나(갱신)', url: 'https://github.com/o/r/pull/1', status: 'MERGED', lastUpdate: '2026-02-02T00:00:00.000+0900' },
    { name: 'no url', repositoryName: 'o/x', lastUpdate: 5 },
    { name: 'no url', repositoryName: 'o/x', status: 'open', lastUpdate: '2026-03-01' },
    { url: 'https://a b/c', name: 'sp' },
    { url: 'https://ok/with-space-after ', name: 'sp2' },
    { url: 'https://', name: 'empty' },
    null,
    5
  ] }, { pullRequests: 'x' }, null, 5, {}] }
  const repos = { detail: [{ repositories: [{ name: 'o/r', commits: [
    { id: 'abcdef1', message: '첫 줄\r\n두번째 줄', url: 'https://github.com/o/r/commit/abcdef1', author: { name: 'alice' }, authorTimestamp: 1_767_225_600_000 },
    { id: 'ABCDEF1234567890', message: '  앞뒤 공백  \n', url: 'ftp://x', author: { name: 'bob' }, authorTimestamp: '2026-01-05T00:00:00.000+0900' },
    { id: 'abc', message: 'short sha' },
    { id: 'g'.repeat(10), message: 'not hex' },
    { id: 'a'.repeat(64), message: '', authorTimestamp: null },
    { id: 'a'.repeat(65), message: 'too long' },
    { id: 'abcdef1', message: '중복 sha', authorTimestamp: 1_767_300_000_000 },
    { id: 'fedcba9', message: 'a\rb\nc', authorTimestamp: 8.64e14 },
    null
  ] }, { name: 5, commits: 'x' }, null] }, { repositories: 'x' }] }
  out.push(c('getDevInfo', 'cloud', { id: '10007' }, prSummary, prs, repos))
  out.push(c('getDevInfo', 'server', { id: '10007' }, { summary: {} }, prs, repos), c('getDevInfo', 'server', { id: 'abc' }, prSummary, prs, repos), c('getDevInfo', 'server', { id: '1234567890123456789' }, prSummary, prs, repos), c('getDevInfo', 'server', { id: '123456789012345678' }, prSummary, prs, repos), c('getDevInfo', 'server', {}, prSummary, prs, repos), c('getDevInfo', 'server', null, prSummary, prs, repos))
  out.push(c('getDevInfo', 'cloud', { id: '1' }, null, null, null), c('getDevInfo', 'cloud', { id: '1' }, { summary: { pullrequest: { byInstanceType: ['x', 'y'] }, repository: { byInstanceType: { b: {}, a: {} } } } }, { detail: [{ pullRequests: [{ name: 'p', url: 'https://x/1', lastUpdate: 8.64e15 + 1 }] }] }, {}))
  out.push(c('getDevInfo', 'cloud', { id: '1' }, prSummary, { detail: [{ pullRequests: Array.from({ length: 40 }, (_, i) => ({ name: `p${i}`, url: `https://x/${i}`, lastUpdate: 1_700_000_000_000 + i * 1000 })) }] }, { detail: [{ repositories: [{ name: 'r', commits: Array.from({ length: 40 }, (_, i) => ({ id: (0x1000000 + i).toString(16), message: `m${i}`, authorTimestamp: 1_700_000_000_000 + (i % 7) * 1000 })) }] }] }))

  // 검색
  const issues = [1, 2, 3].map((n) => ISSUE_RAW(n))
  out.push(c('search', 'cloud', { issues, nextPageToken: 'tok', isLast: false }, null, 1), c('search', 'cloud', { issues, nextPageToken: 'tok', isLast: true }, null, 1), c('search', 'cloud', { issues, nextPageToken: 'tok' }, 'prev', 7), c('search', 'cloud', { issues, nextPageToken: 'tok' }, null, 8), c('search', 'cloud', { issues: [], nextPageToken: '' }, null, 1), c('search', 'cloud', { issues: 'x' }, null, 1), c('search', 'cloud', {}, null, 1), c('search', 'cloud', null, null, 1), c('search', 'cloud', { issues: [null, 5], nextPageToken: 5 }, null, 1))
  out.push(c('search', 'server', { issues, total: 10 }, null, 1), c('search', 'server', { issues, total: 3 }, null, 1), c('search', 'server', { issues, total: 100 }, '3', 8), c('search', 'server', { issues, total: 100 }, '3', 7), c('search', 'server', { issues, total: 100 }, '25', 2), c('search', 'server', { issues, total: 100 }, '200', 2), c('search', 'server', { issues, total: 100 }, '201', 2), c('search', 'server', { issues, total: 100 }, '-1', 2), c('search', 'server', { issues, total: 100 }, '1.5', 2), c('search', 'server', { issues, total: 100 }, 'abc', 2), c('search', 'server', { issues, total: 100 }, '0x10', 2), c('search', 'server', { issues, total: 100 }, '1e1', 2), c('search', 'server', { issues, total: 100 }, ' 5 ', 2), c('search', 'server', { issues, total: 100 }, 'Infinity', 2), c('search', 'server', { issues }, null, 1), c('search', 'server', { issues, total: '10' }, null, 1), c('search', 'server', { issues: [], total: 10 }, null, 1), c('search', 'server', {}, null, 1), c('search', 'server', null, null, 1))

  for (const l of [FIELD_LIST, MANY_FIELDS, [], null, {}, 'x', [FIELD_LIST[0]], [...FIELD_LIST].reverse()]) out.push(c('pickDateFields', l))

  // 워크로그
  const wl = (id: string, author: Record<string, unknown>, started: unknown, extra: Record<string, unknown> = {}): Record<string, unknown> => ({ id, author, started, timeSpentSeconds: 3600, comment: 'c', ...extra })
  const me = { id: 'acc-me', name: '나' }
  const meS = { id: 'me-dc', name: '나' }
  const cloudWl = (issue: string, list: unknown[], total?: number): Record<string, unknown> => ({ key: issue, fields: { summary: `s-${issue}`, issuetype: { name: 'Task' }, worklog: { worklogs: list, total: total ?? list.length } } })
  const q = { from: '2026-01-10', to: '2026-01-12', users: [] as string[] }
  const adfComment = { type: 'doc', content: [para(text('첫 줄'), { type: 'hardBreak' }, text('둘째')), para(text('셋째'))] }
  const cloudEntries = [
    wl('1', { accountId: 'acc-me', displayName: '나' }, '2026-01-10T09:00:00.000+0900', { comment: adfComment }),
    wl('2', { accountId: 'acc-other', displayName: '남' }, '2026-01-10T09:00:00.000+0900'),
    wl('3', { accountId: 'acc-me' }, '2026-01-08T00:00:00.000+0000'),
    wl('4', { accountId: 'acc-me' }, '2026-01-07T23:59:59.999+0000'),
    wl('5', { accountId: 'acc-me' }, '2026-01-14T13:59:59.999+0000'),
    wl('6', { accountId: 'acc-me' }, '2026-01-14T14:00:00.000+0000'),
    wl('8', { accountId: 'acc-me' }, 'garbage'),
    wl('9', { accountId: 'acc-me', displayName: '' }, '2026-01-11T00:00:00Z', { timeSpentSeconds: '60', comment: undefined }),
    wl('10', { accountId: '' }, '2026-01-11T00:00:00Z'),
    wl('11', { accountId: 'acc-me' }, '2026-01-11T00:00:00Z', { comment: 'x'.repeat(600) }),
    wl('12', { accountId: 'acc-me' }, '2026-01-11T00:00:00Z', { comment: { type: 'doc', content: [para(text('😀'.repeat(300)))] } }),
    null,
    5
  ]
  out.push(c('getWorklogs', 'cloud', q, me, { issues: [cloudWl('ABC-1', cloudEntries), cloudWl('ABC-2', []), cloudWl('ABC-1', cloudEntries), { fields: {} }, null] }, {}))
  out.push(c('getWorklogs', 'cloud', { ...q, users: ['acc-other'] }, me, { issues: [cloudWl('ABC-1', cloudEntries)] }, {}))
  out.push(c('getWorklogs', 'cloud', { ...q, users: ['acc-other'], includeMe: true }, me, { issues: [cloudWl('ABC-1', cloudEntries)] }, {}))
  out.push(c('getWorklogs', 'cloud', { ...q, users: ['acc-other'], includeMe: 'yes' }, me, { issues: [cloudWl('ABC-1', cloudEntries)] }, {}))
  out.push(c('getWorklogs', 'cloud', { ...q, includeMe: true }, me, { issues: [cloudWl('ABC-1', cloudEntries)] }, {}))
  out.push(c('getWorklogs', 'cloud', q, me, { issues: [cloudWl('ABC-3', cloudEntries.slice(0, 2), 5)] }, { 'ABC-3': { worklogs: [cloudEntries[0], cloudEntries[0], cloudEntries[3], wl('21', { accountId: 'acc-me' }, '2026-01-12T10:00:00Z')], total: 4 } }))
  out.push(c('getWorklogs', 'cloud', q, me, { issues: [cloudWl('ABC-3', [], 2), cloudWl('ABC-4', [wl('31', { accountId: 'acc-me' }, '2026-01-12T10:00:00Z')])] }, { 'ABC-3': { worklogs: [wl('41', { accountId: 'acc-me' }, '2026-01-12T11:00:00Z'), wl('42', { accountId: 'acc-me' }, '2026-01-12T12:00:00Z')], total: 2 } }))
  out.push(c('getWorklogs', 'cloud', { from: '2026-02-30', to: '2026-03-02', users: [] }, me, { issues: [cloudWl('ABC-1', cloudEntries)] }, {}))
  out.push(c('getWorklogs', 'cloud', { from: 'garbage', to: '2026-01-02', users: [] }, me, { issues: [] }, {}))
  out.push(c('getWorklogs', 'cloud', { from: '2026-01-10', to: 'x', users: [] }, me, { issues: [] }, {}))
  out.push(c('getWorklogs', 'cloud', q, me, { issues: [] }, {}), c('getWorklogs', 'cloud', q, me, {}, {}), c('getWorklogs', 'cloud', q, me, null, {}), c('getWorklogs', 'cloud', q, me, { issues: [cloudWl('ABC-5', [], 2)] }, {}))
  const dcEntries = [
    wl('1', { name: 'me-dc', key: 'JIRAUSER1', displayName: '나' }, '2026-01-10T09:00:00.000+0900', { comment: 'dc 주석\n둘째' }),
    wl('2', { name: 'other', key: 'JIRAUSER2' }, '2026-01-10T09:00:00.000+0900'),
    wl('3', { name: 'JIRAUSER1', key: 'x' }, '2026-01-10T09:00:00.000+0900'),
    wl('4', { name: 'zzz', key: 'me-dc' }, '2026-01-10T09:00:00.000+0900'),
    wl('5', { key: 'me-dc' }, '2026-01-10T09:00:00.000+0900')
  ]
  out.push(c('getWorklogs', 'server', q, meS, { issues: [cloudWl('DC-1', dcEntries)], total: 1 }, {}))
  out.push(c('getWorklogs', 'server', { ...q, users: ['other'] }, meS, { issues: [cloudWl('DC-1', dcEntries)], total: 1 }, {}))
  out.push(c('getWorklogs', 'server', { ...q, users: ['other'], includeMe: true }, meS, { issues: [cloudWl('DC-1', dcEntries)], total: 1 }, {}))
  out.push(c('getWorklogs', 'server', q, meS, { issues: [cloudWl('DC-2', [], 2)], total: 1 }, { 'DC-2': { worklogs: dcEntries.slice(0, 2), total: 2 } }))
  out.push(c('getWorklogs', 'server', q, meS, { issues: Array.from({ length: 12 }, (_, i) => cloudWl(`DC-${i}`, [wl(`w${i}`, { name: 'me-dc' }, '2026-01-10T09:00:00.000+0900')])), total: 12 }, {}))
  return out
}
