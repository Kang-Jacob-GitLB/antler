// fetch 가드 순수 함수의 입력 표. tests/fetch.test.ts(TS 동작)와 tests/goldenFetch.test.ts(Rust 이식용 골든 생성)가 같은 배열을 쓴다 —
// 표를 고치면 두 쪽이 함께 움직이고, 골든을 다시 만들면 Rust 이식(src-tauri/tests/fetch_guard.rs)이 같은 결과를 내는지 본다.

/** `git config --list --show-scope -z` 출력 한 예와 파싱 결과 */
export const CONFIG_LIST_OUT = 'global\0user.name\nA B\0local\0core.sshcommand\nssh -i x\0local\0core.bare\0worktree\0remote.o.url\nu\nrl\0'
export const CONFIG_LIST_ENTRIES = [
  { scope: 'global', key: 'user.name', value: 'A B' },
  { scope: 'local', key: 'core.sshcommand', value: 'ssh -i x' },
  { scope: 'local', key: 'core.bare', value: null },
  { scope: 'worktree', key: 'remote.o.url', value: 'u\nrl' }
]

/** `--show-scope` 없는 `git config --<scope> --list -z` 출력(scope 는 local) */
export const SCOPED_LIST_OUT = 'core.bare\0Core.SshCommand\nssh -i x\0remote.o.url\nu\nrl\0'
export const SCOPED_LIST_ENTRIES = [
  { scope: 'local', key: 'core.bare', value: null },
  { scope: 'local', key: 'core.sshcommand', value: 'ssh -i x' },
  { scope: 'local', key: 'remote.o.url', value: 'u\nrl' }
]

/** 허용 목록 모델: 목록에 없는 키는 전부 "알 수 없는 설정"이다 */
export const UNKNOWN_KEYS: string[] = [
  'core.sshcommand',
  'core.gitproxy',
  'core.askpass',
  'core.alternaterefscommand',
  'core.editor',
  'core.pager',
  'fetch.prunetags',
  'remote.origin.prunetags',
  'credential.helper',
  'credential.https://example.com.helper',
  'credential.trace',
  'credential.tracesecrets',
  'credential.https://example.com.provider',
  'credential.interactive',
  'credential.username',
  'remote.origin.uploadpack',
  'remote.origin.receivepack',
  'remote.origin.vcs',
  'remote.my.fork.proxy',
  'protocol.ext.allow',
  'protocol.allow',
  'protocol.version',
  'url.ext::sh -c x%s.insteadof',
  'url.https://x/.pushinsteadof',
  'uploadpack.packobjectshook',
  'http.proxy',
  'http.version',
  'http.https://example.com/.sslcainfo',
  'http.extraheader',
  'http.emptyauth',
  'http.proactiveauth',
  'http.delegation',
  'http.allowntlmauth',
  'http.schannelcheckrevoke',
  'http.schannelusesslcainfo',
  'http.followredirects',
  'ssh.variant',
  'trace2.eventtarget',
  'trace2.normaltarget',
  'trace2.perftarget',
  'fetch.bundleuri',
  'fetch.bundlecreationtoken',
  'transfer.bundleuri',
  'alias.st',
  'diff.external',
  'merge.tool',
  'gui.wmstate',
  'color.ui',
  'some.future.key'
]

export const ALLOWED_PAIRS: [string, string][] = [
  ['user.name', 'a'],
  ['user.email', 'a@b'],
  ['core.bare', 'false'],
  ['core.repositoryformatversion', '0'],
  ['core.filemode', 'false'],
  ['core.autocrlf', 'true'],
  ['core.hookspath', '.husky'],
  ['core.fsmonitor', 'true'],
  ['core.fsmonitor', 'false'],
  ['remote.origin.url', 'https://h/r.git'],
  ['remote.my.fork.fetch', '+refs/heads/*:refs/remotes/my.fork/*'],
  ['remote.origin.promisor', 'true'],
  ['remote.origin.partialclonefilter', 'blob:none'],
  ['branch.main.remote', 'origin'],
  ['branch.feature/x.merge', 'refs/heads/main'],
  ['branch.main.vscode-merge-base', 'origin/main'],
  ['branch.main.github-pr-owner-number', 'o#r#1'],
  ['pull.rebase', 'true'],
  ['fetch.prune', 'true'],
  ['extensions.partialclone', 'origin'],
  ['extensions.worktreeconfig', 'true'],
  ['lfs.repositoryformatversion', '0'],
  ['filter.lfs.clean', 'git-lfs clean -- %f'],
  ['filter.lfs.smudge', 'git-lfs smudge -- %f'],
  ['filter.lfs.process', 'git-lfs filter-process'],
  ['filter.lfs.required', 'true'],
  ['submodule.lib.url', 'https://h/lib.git'],
  ['submodule.active', '.'],
  ['include.path', '../x'],
  ['includeif.gitdir:~/w/.path', '~/w.cfg']
]

/** [키, 값, 허용 여부] */
export const REFSPEC_CASES: [string, string, boolean][] = [
  ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*', true],
  ['remote.origin.fetch', 'refs/heads/*:refs/remotes/origin/*', true],
  ['remote.origin.fetch', '+refs/heads/main:refs/remotes/origin/main', true],
  ['remote.my.fork.fetch', '+refs/heads/*:refs/remotes/my.fork/*', true],
  ['remote.origin.fetch', '+refs/heads/feature:refs/heads/feature', false],
  ['remote.origin.fetch', '+refs/heads/main:refs/stash', false],
  ['remote.origin.fetch', '+refs/heads/main:refs/replace/abc', false],
  ['remote.origin.fetch', '+refs/heads/main:HEAD', false],
  ['remote.origin.fetch', '+refs/heads/none/*:refs/heads/*', false],
  ['remote.origin.fetch', '+refs/heads/*:refs/remotes/other/*', false],
  ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/x/*', false],
  ['remote.origin.fetch', '+refs/heads/a:refs/remotes/origin/b', false],
  ['remote.origin.fetch', '+refs/tags/*:refs/tags/*', false],
  ['remote.origin.fetch', '+refs/*:refs/remotes/origin/*', false],
  ['remote.origin.fetch', '^refs/heads/x', false],
  ['remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*:refs/heads/x', false]
]

/** 값까지 맞아야 허용하는 키: [키, 값, 허용 여부] */
export const VALUE_CASES: [string, string | null, boolean][] = [
  ['filter.lfs.clean', 'evil --x', false],
  ['filter.lfs.process', 'sh -c "touch x"', false],
  ['filter.lfs.required', 'false', false],
  ['core.fsmonitor', '.git/hooks/fsmonitor-watchman', false],
  ['core.bare', 'true', false],
  ['filter.lfs.clean', null, false],
  ['filter.other.clean', 'git-lfs clean -- %f', false]
]

/** 거부해야 하는 remote URL(전송 도우미는 사유 문구까지 확인한다) */
export const HELPER_URLS: [string, RegExp][] = [
  ['ext::sh -c "touch /tmp/x"', /ext::/],
  ['fd::17', /fd::/],
  ['foo::bar', /foo::/]
]
export const OPTION_LIKE_URLS = ['-oProxyCommand=x', 'ssh://-oProxyCommand=x/repo', 'ssh://git@-host/repo']
export const SAFE_URLS = ['https://github.com/a/b.git', 'git@github.com:a/b.git', 'file:///tmp/r.git', '../r.git', 'C:/r.git', 'ssh://git@host/r']

/** UNC·네트워크 경로·NT 네임스페이스 */
export const UNC_REJECT = [
  '//host@8080/DavWWWRoot/x.git',
  '//host/share/x.git',
  '\\\\host\\share\\x.git',
  '\\\\?\\UNC\\host\\share\\x.git',
  '\\\\.\\pipe\\x',
  '/\\host/share/x',
  '  \\\\host\\share\\x.git',
  'file://host/share/x.git',
  'file://localhost/share/x.git',
  'file:////host/share/x.git',
  'FILE://host/share/x.git',
  'file:\\\\host\\share\\x.git',
  // NT 네임스페이스 접두어 — 앞이 역슬래시 하나라 // 검사를 빠져나간다(git -C '\??\C:\…' 이 실제로 열린다)
  '\\??\\UNC\\host\\share\\x.git',
  '  \\??\\UNC\\host\\share\\x.git',
  '/??/UNC/host/share/x.git',
  'file:///??/UNC/host/share/x.git',
  'file:/??/UNC/host/share/x.git'
]
export const UNC_ALLOW = ['file:///C:/repos/x.git', 'file:///tmp/r.git', '/tmp/r.git', 'C:\\repos\\x.git', 'C:/repos/x.git', '../r.git', 'https://example.com/x.git', 'git@github.com:a/b.git']

export const SSH_DEFAULT = 'ssh -o BatchMode=yes -o ConnectTimeout=15'

/** resolveSshCommand 입력과 기대값. platform 이 없으면 'linux' 로 본다 */
export interface SshCase {
  userSshCommand: string | null
  kind: 'local' | 'wsl' | 'ssh' | 'github'
  env: Record<string, string>
  platform?: 'win32' | 'linux'
  expected: string
}
export const SSH_CASES: SshCase[] = [
  { userSshCommand: 'ssh -F my', kind: 'local', env: { GIT_SSH_COMMAND: 'env-ssh' }, expected: 'ssh -F my' },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH_COMMAND: 'env-ssh', GIT_SSH: '/x/ssh' }, expected: 'env-ssh' },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH: '/opt/my ssh/ssh' }, platform: 'linux', expected: "'/opt/my ssh/ssh'" },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH: 'C:\\Program Files\\PuTTY\\plink.exe' }, platform: 'win32', expected: "'C:/Program Files/PuTTY/plink.exe'" },
  { userSshCommand: null, kind: 'local', env: {}, expected: SSH_DEFAULT },
  // 원격 환경(WSL·SSH)에서는 이 프로세스의 환경변수가 의미 없다
  { userSshCommand: null, kind: 'wsl', env: { GIT_SSH_COMMAND: 'env-ssh' }, expected: SSH_DEFAULT }
]
/** 위 표 밖의 경계(골든에만 쓴다): 빈 값은 없는 것과 같다, 작은따옴표 이스케이프, github·ssh 종류 */
export const SSH_EDGE_CASES: Omit<SshCase, 'expected'>[] = [
  { userSshCommand: '', kind: 'local', env: { GIT_SSH_COMMAND: 'env-ssh' } },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH_COMMAND: '', GIT_SSH: '/x/ssh' } },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH_COMMAND: '', GIT_SSH: '' } },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH: "/o'dd/ssh" }, platform: 'linux' },
  { userSshCommand: null, kind: 'local', env: { GIT_SSH: "C:\\o'dd\\ssh.exe" }, platform: 'win32' },
  { userSshCommand: null, kind: 'ssh', env: { GIT_SSH_COMMAND: 'env-ssh', GIT_SSH: '/x/ssh' } },
  { userSshCommand: null, kind: 'github', env: { GIT_SSH_COMMAND: 'env-ssh' } },
  { userSshCommand: 'ssh -F 한글', kind: 'wsl', env: {} }
]

/** parseGitVersion: [입력, 기대값] */
export const VERSION_PARSE_CASES: [string, [number, number, number] | null][] = [
  ['git version 2.49.0.windows.1\n', [2, 49, 0]],
  ['git version 2.43.0', [2, 43, 0]],
  ['git version 2.39', [2, 39, 0]],
  ['weird', null]
]
/** versionAtLeast(v, [2,45,1]) : [v, 기대값] */
export const VERSION_AT_LEAST_CASES: [[number, number, number] | null, boolean][] = [
  [[2, 45, 1], true],
  [[2, 49, 0], true],
  [[3, 0, 0], true],
  [[2, 45, 0], false],
  [[2, 43, 0], false],
  [null, false]
]

/** nextDelayMs: [분, 연속 실패 수, 기대 ms] */
export const BACKOFF_CASES: [number, number, number][] = [
  [5, 0, 5 * 60_000],
  [5, 1, 10 * 60_000],
  [5, 2, 20 * 60_000],
  [5, 3, 30 * 60_000],
  [1, 20, 30 * 60_000],
  [60, 5, 60 * 60_000]
]

/** classifyFailure: [오류 메시지, 기대값] */
export const CLASSIFY_CASES: [string, 'auth' | 'network' | 'protocol'][] = [
  ["fatal: transport 'file' not allowed", 'protocol'],
  ['fatal: Authentication failed for https://x', 'auth'],
  ['git@h: Permission denied (publickey).', 'auth'],
  ['fatal: could not read Username for https://x: terminal prompts disabled', 'auth'],
  ['fatal: unable to access: Could not resolve host', 'network']
]

/** 확인 창: 이 문자들은 \uXXXX 로 드러난다 */
export const DISPLAY_CODEPOINTS: [string, number][] = [
  ['한글 채움 U+115F', 0x115f],
  ['한글 채움 U+1160', 0x1160],
  ['한글 채움 U+3164', 0x3164],
  ['반각 한글 채움 U+FFA0', 0xffa0],
  ['점자 빈칸 U+2800', 0x2800],
  ['결합 그래핌 조이너 U+034F', 0x034f],
  ['변이 선택자 U+FE0F', 0xfe0f],
  ['변이 선택자 보충 U+E0100', 0xe0100],
  ['NBSP U+00A0', 0x00a0],
  ['전각 공백 U+3000', 0x3000],
  ['EM 공백 U+2003', 0x2003],
  ['결합 겹침 줄 U+0336', 0x0336],
  ['오른쪽 큰따옴표 U+201D', 0x201d],
  ['왼쪽 큰따옴표 U+201C', 0x201c],
  ['전각 따옴표 U+FF02', 0xff02],
  ['꺾쇠 따옴표 U+00BB', 0x00bb],
  ['ALM U+061C', 0x061c],
  ['소프트 하이픈 U+00AD', 0x00ad],
  ['몽골어 모음 구분자 U+180E', 0x180e]
]
