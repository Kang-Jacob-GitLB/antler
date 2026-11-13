#!/usr/bin/env node
// 골든 테스트용 경계 조건 fixture 를 만든다(git 을 실행하지 않는다 — 손으로 짠 바이트 열).
// 빈 입력·마지막 NUL 없음·잘못된 레코드·숫자가 아닌 날짜·깨진 UTF-8·정렬 경계를 담는다.
// 사용: node scripts/make-edge-fixtures.mjs  →  tests/fixtures/edge/*
import { mkdirSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const OUT = resolve(import.meta.dirname, '..', 'tests', 'fixtures', 'edge')
mkdirSync(OUT, { recursive: true })

const US = '\x1f'
const NUL = '\0'
const H = (c) => c.repeat(40)
const out = (name, data) => writeFileSync(join(OUT, name), data)
const join8 = (...f) => f.join(US)

out('empty.bin', Buffer.alloc(0))

// ---- log: 마지막 NUL 없음, 앞 개행, 빈 레코드, 필드 부족, 숫자 아닌 날짜, 제목 속 US, 깨진 UTF-8 ----
out(
  'log-edge.bin',
  Buffer.concat([
    Buffer.from(
      [
        join8(H('a'), `${H('b')} ${H('c')}`, '홍길동', 'dev@example.com', '1700000000', '1700000060', 'HEAD -> refs/heads/main, tag: refs/tags/v1.0, refs/remotes/origin/main, refs/remotes/origin/HEAD', 'feat: 한글 제목'),
        '',
        '\n' + join8(H('b'), '', 'a b', '', ' 12 ', '', 'HEAD', ' 앞뒤 공백 제목 '),
        join8(H('c'), H('d'), 'x', 'x@x', 'abc', '', 'refs/stash, refs/heads/a/b, , tag: refs/tags/t, refs/foo/bar', 'x'),
        'too' + US + 'few' + US + 'fields',
        join8(H('d'), '', 'n', 'e', '1', '2', '', '제목' + US + '속' + US + '구분자'),
        join8(H('e'), '', 'n', 'e', '0x10', '1e3', 'HEAD -> refs/heads/feature/한글', ''),
        join8(H('f'), '', 'n', 'e', '', '-5', '  HEAD  ,  refs/remotes/up/stream/x  ', 'trim')
      ].join(NUL) + NUL
    ),
    // 깨진 UTF-8 바이트가 든 제목, 그리고 종결 NUL 없는 마지막 레코드
    Buffer.from(join8(H('9'), '', 'n', 'e', '3', '4', '', 'bad-'), 'utf8'),
    Buffer.from([0xff, 0xfe, 0xe3, 0x81]),
    Buffer.from(NUL + join8(H('8'), H('7'), 'last', 'e', '5', '6', 'HEAD', '끝'), 'utf8')
  ])
)

// ---- stash: 잘못된 ref, 필드 부족, 앞 개행, 메시지 속 US, 마지막 NUL 없음 ----
out(
  'stash-edge.bin',
  [
    [H('a'), 'stash@{0}', '1700000000', 'On main: 임시 저장'].join(US),
    '\n' + [H('b'), 'stash@{12}', '1700000001', 'WIP on x: abc 메시지'].join(US),
    [H('c'), 'stash@{x}', '1', 'bad ref'].join(US),
    [H('d'), 'stash@{3}', '2'].join(US),
    [H('e'), 'stash@{4}', 'nan', 'a', 'b', 'c'].join(US),
    '',
    [H('f'), 'stash@{5}', '', 'no trailing nul'].join(US)
  ].join(NUL)
)

// ---- worktree: 속성 모양 다양, 첫 worktree 전의 떠돌이 필드, 마지막 NUL 없음 ----
out(
  'worktree-edge.bin',
  [
    'stray value',
    'worktree C:/repo/주 저장소',
    'HEAD ' + H('a'),
    'branch refs/heads/main',
    '',
    'worktree C:/wt/작업 트리',
    'HEAD ' + '0'.repeat(40),
    'locked',
    'prunable gitdir file points to non-existent location',
    '',
    'worktree C:/wt/detached',
    'HEAD ' + H('b'),
    'detached',
    'locked 이유 있음',
    'unknown-key value',
    'bare',
    '',
    '',
    'worktree C:/wt/no-values',
    'branch',
    'HEAD',
    'branch refs/heads/feature/x',
    'worktree C:/wt/last'
  ].join(NUL)
)

// ---- for-each-ref: 줄마다 NUL 필드 10개. 부족한 줄·빈 줄·CR·정렬 경계 ----
const ref = (...f) => f.join(NUL)
const R = (name, o = {}) =>
  ref(name, o.hash ?? H('1'), o.peeled ?? '', o.head ?? ' ', o.up ?? '', o.track ?? '', o.symref ?? '', o.type ?? 'commit', o.ptype ?? '', o.created ?? '1700000000')
out(
  'refs-edge.bin',
  [
    R('refs/heads/main', { head: '*', up: 'origin/main', track: 'ahead 2, behind 1' }),
    R('refs/heads/gone', { up: 'origin/gone', track: 'gone' }),
    R('refs/heads/ahead-only', { up: 'origin/ahead-only', track: 'ahead 3' }),
    R('refs/heads/no-up', { track: 'ahead 9' }),
    R('refs/heads/Feature', {}),
    R('refs/heads/feature', {}),
    R('refs/heads/feature-2', {}),
    R('refs/heads/feature/2', {}),
    R('refs/heads/feature_2', {}),
    R('refs/heads/b10', {}),
    R('refs/heads/b9', {}),
    R('refs/heads/b09', {}),
    R('refs/heads/feature/한글-브랜치', {}),
    R('refs/heads/Zed', {}),
    R('refs/heads/alpha', {}),
    '',
    'refs/heads/short' + NUL + H('1') + NUL + NUL,
    R('refs/remotes/origin/HEAD', { symref: 'refs/remotes/origin/main' }),
    R('refs/remotes/origin/main', {}),
    R('refs/remotes/origin/team/HEAD', {}),
    R('refs/remotes/team/upstream/feature/x', {}),
    R('refs/remotes/team/upstream/main', {}),
    R('refs/remotes/team/other', {}),
    R('refs/remotes/ghost/main', {}),
    R('refs/remotes/solo', {}),
    R('refs/tags/v1.2.0', { created: '100' }),
    R('refs/tags/v1.10.0', { created: '100' }),
    R('refs/tags/v1.9.0', { created: '100' }),
    R('refs/tags/old', { created: '' }),
    R('refs/tags/older', { created: '' }),
    R('refs/tags/new', { created: '200' }),
    R('refs/tags/bad-date', { created: 'abc' }),
    R('refs/tags/blank-date', { created: ' ' }),
    R('refs/tags/ann', { hash: H('2'), peeled: H('3'), type: 'tag', ptype: 'commit', created: '150' }),
    R('refs/tags/tree-tag', { hash: H('4'), type: 'tree' }),
    R('refs/tags/tag-of-tag', { hash: H('5'), peeled: H('6'), type: 'tag', ptype: 'tag', created: '150' }),
    R('refs/other/ignored', {}),
    R('refs/heads/crlf-end', {}) + '\r'
  ].join('\n') + '\n'
)
// 마지막 줄 개행 없음
out('refs-no-trailing-newline.bin', R('refs/heads/main', { head: '*' }) + '\n' + R('refs/tags/v1', {}))

// refs 파서에 같이 넘기는 `git remote` 출력(CRLF·빈 줄·이름 속 '/')
out('remotes-edge.txt', 'origin\r\nteam\r\n\r\nteam/upstream\nsolo')

// ======== 2차 이식(diff·wipStatus·changes·commit·blame·fileHistory·submodule) ========
// 확장자: .diff(unified diff)·.dat(NUL 등 바이너리 입력)·.txt. `.bin` 은 nulRecords 골든이 전부 돌리므로 쓰지 않는다.
const lines = (...l) => l.join('\n')

// ---- diff ----
out(
  'diff-basic.diff',
  lines(
    'diff --git a/src/a.ts b/src/a.ts',
    'index 1111111..2222222 100644',
    '--- a/src/a.ts',
    '+++ b/src/a.ts',
    '@@ -1,6 +1,7 @@ function 한글(x) {',
    ' keep',
    '', // 선행 공백까지 잘린 빈 문맥 줄
    '-old 안녕',
    '+new 🙂 emoji',
    '--- dash dash',
    '+++ plus plus',
    '\\ No newline at end of file',
    '한글 첫 글자는 접두로 먹힌다',
    '@@ -20 +21,0 @@',
    '@@ -30,2 +30,3 @@',
    ' ctx',
    '+added',
    ' ctx2',
    '\\   No newline  ',
    ''
  )
)
out('diff-small.diff', lines('@@ -0,0 +1,2 @@', '+a', '+b', ''))
out(
  'diff-meta.diff',
  lines(
    'diff --git a/old name.ts b/새 이름.ts',
    'old mode 100644',
    'new mode 100755',
    'similarity index 90%',
    'dissimilarity index 10%',
    'rename from old name.ts',
    'rename to 새 이름.ts',
    'copy from c1',
    'copy to c2',
    'new file mode 100755',
    'deleted file mode 100644',
    'index 1..2 100644',
    '--- a/old name.ts',
    '+++ b/새 이름.ts',
    '@@ -1 +1 @@',
    '-1',
    '+2',
    'new file mode 100000', // hunk 안에서는 내용 줄(접두 n 은 문맥)이라 meta 가 아니다
    ''
  )
)
out('diff-meta-only.diff', lines('diff --git a/s b/s', 'old mode 100644', 'new mode 100755', ''))
out('diff-binary.diff', lines('diff --git a/x.bin b/x.bin', 'new file mode 100644', 'index 0000000..1234567', 'Binary files /dev/null and b/x.bin differ', ''))
out(
  'diff-gitbinary.diff',
  lines('diff --git a/y.bin b/y.bin', 'index 1111111..2222222 100644', 'GIT binary patch', 'literal 5', 'Mc$_X', '', 'literal 3', 'Kc$@', '')
)
out(
  'diff-typechange.diff',
  lines(
    'diff --git a/a b/a',
    'deleted file mode 100644',
    'index 1111111..0000000',
    '--- a/a',
    '+++ /dev/null',
    '@@ -1,2 +0,0 @@',
    '-line 1',
    '-line 2',
    'diff --git a/a b/a',
    'new file mode 120000',
    'index 0000000..2222222',
    '--- /dev/null',
    '+++ b/a',
    '@@ -0,0 +1 @@',
    '+target/path',
    '\\ No newline at end of file',
    ''
  )
)
out(
  'diff-crlf.diff',
  lines(
    'diff --git a/w.txt b/w.txt\r',
    'new file mode 100755\r',
    '@@ -1,4 +1,4 @@ fn()\r',
    ' a\r',
    '-b\r',
    '+c\r',
    '+\r',
    '\r',
    '\\ No newline at end of file\r',
    ''
  )
)
out(
  'diff-weird.diff',
  lines(
    '+++ stray plus before hunk',
    'random header line',
    'Binary files notice differ',
    '@@ -1 +1 @@',
    '-a',
    '+b',
    '@@ malformed header @@',
    ' after malformed',
    '@@@ -1,2 -1,2 +1,2 @@@ combined',
    '+x',
    '@@ -5,1 +6,1 @@ with\rCR inside',
    ' y',
    '@@ -100000000000000000000,3 +1 @@ huge',
    ' z',
    '@@ -0,0 +0,0 @@ empty',
    '@@ -3,2 +3,2 @@ line\u2028separator',
    ' q',
    '@@ -7 +7 @@',
    '@@ -8 +8 @@x',
    '@@ -9 +9 @@  two spaces',
    '@@ -010,02 +0x1,3 @@ bad',
    '@@ -11,1 +11,1 @@ tail 한글\r',
    ' w',
    '@@ -12 +12 @@ nl',
    ' last line without newline'
  )
)
// 줄 하나가 아주 길고 개행이 없다(maxBytes 경로의 개행 없는 조각)
out('diff-longline.diff', '@@ -1 +1 @@\n+' + 'x'.repeat(200))
// 기본 상한(3000)을 넘는 큰 hunk — 상한 이후에 @ 로 시작하는 줄(헤더)도 있다
out(
  'diff-big.diff',
  lines(
    'diff --git a/big.txt b/big.txt',
    'index 1..2 100644',
    '--- a/big.txt',
    '+++ b/big.txt',
    '@@ -1,3100 +1,3100 @@ big',
    ...Array.from({ length: 3100 }, (_, i) => (i % 3 === 0 ? ` ctx ${i}` : i % 3 === 1 ? `-old ${i}` : `+new ${i} 한글`)),
    '@@ -4000 +4000 @@',
    '+tail',
    ''
  )
)
// 기본 상한을 정확히 채우고 끝나는 diff(잘린 것이 아니다)
out('diff-exact3000.diff', lines('@@ -0,0 +1,3000 @@', ...Array.from({ length: 3000 }, (_, i) => `+l${i}`), ''))

// ---- changes: name-status / numstat ----
out(
  'name-status-edge.dat',
  [
    'M', 'a.txt',
    'A', '한글 경로.txt',
    'M', 'bin.png',
    'M', 'half',
    'R100', 'old 1', 'new 1',
    'T', 't',
    'U', 'u',
    'X', 'x',
    'B', 'b',
    'C075', 'from', 'to',
    'R', 'o', 'n',
    'Rabc', 'o2', 'n2',
    'R 50', 'o3', 'n3',
    'R0x10', 'o4', 'n4',
    'R1e1', 'o5', 'n5',
    'MM', 'second char',
    '', 'empty-code',
    'M', '',
    'R100', 'only-old'
  ].join(NUL) + NUL
)
out('name-status-no-trailing-nul.dat', ['M', 'a', 'A', 'b'].join(NUL))
out('name-status-dangling.dat', ['M', 'a', 'D'].join(NUL) + NUL)
out(
  'numstat-edge.dat',
  [
    '1\t2\ta.txt',
    '-\t-\tbin.png',
    '3\t-\thalf',
    '-\t4\thalf2',
    'abc\tx\tbad num',
    ' 5\t 6 \tspaces',
    '0x10\t1e1\thex',
    '\t\tempty nums',
    'noTabs',
    'one\ttab',
    '5\t6\t', 'old 1', 'new 1',
    '9\t9\t한글 경로.txt',
    '1\t1\ta.txt',
    '7\t8\t', 'o'
  ].join(NUL)
)

// ---- wipStatus: git status --porcelain=v2 -z ----
const HX = ` ${H('1')} ${H('2')}`
const one = (xy, path) => `1 ${xy} N... 100644 100644 100644${HX} ${path}`
const two = (xy, score, path) => `2 ${xy} N... 100644 100644 100644${HX} ${score} ${path}`
const unm = (path) => `u UU N... 100644 100644 100644 100644 ${H('1')} ${H('2')} ${H('3')} ${path}`
out(
  'wip-status.dat',
  [
    one('.M', 'src/modified.ts'),
    one('M.', 'src/staged.ts'),
    one('MM', 'src/both 공백 한글.ts'),
    one('A.', 'new.ts'),
    one('AM', 'added-then-edited.ts'),
    one('.D', 'deleted-in-tree.ts'),
    one('D.', 'deleted-staged.ts'),
    one('T.', 'typechanged.ts'),
    two('R.', 'R100', 'renamed new.ts'),
    'renamed old.ts',
    two('RM', 'R085', '한글 새.ts'),
    '한글 옛.ts',
    two('C.', 'C090', 'copy-to.ts'),
    'copy-from.ts',
    unm('conflict file.ts'),
    '? untracked.txt',
    '? dir/한글 파일.txt',
    '? 공백 있는 파일.txt'
  ].join(NUL) + NUL
)
out(
  'wip-edge.dat',
  [
    '',
    '# comment',
    '! ignored',
    one('..', 'no-change.ts'),
    one('UM', 'unknown-letter.ts'),
    `1 .M N... 100644 100644 100644${HX}`, // 경로 필드 없음
    `1 M. N... 100644 100644 100644${HX} `, // 경로가 빈 문자열
    two('R.', 'R100', 'new-eats-next.ts'),
    'orig-eaten.ts',
    two('..', 'R100', 'made-nothing.ts'), // 아무 목록에도 안 들어가도 다음 레코드가 원래 경로로 소비된다
    '? swallowed-as-orig.txt',
    one('A.', 'dup.ts'),
    one('.M', 'dup.ts'),
    one('.M', 'same.ts'),
    one('.M', 'same.ts'),
    one('MM', 'mm.ts'),
    'u UU truncated',
    '?',
    '? ',
    '?x',
    '? 마지막 untracked',
    two('R.', 'R1', 'last-rename-no-orig')
  ].join(NUL)
)

// ---- commit: git log -1 -z --format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%s%x00%b ----
const commit = (parents, body, o = {}) =>
  [H('a'), parents, o.an ?? '홍길동', o.ae ?? 'dev@example.com', o.at ?? '1700000000', o.cn ?? 'Committer', o.ce ?? 'c@example.com', o.ct ?? '1700000100', o.s ?? 'feat: 제목 한글', body].join(NUL) + NUL
out('commit-meta.dat', commit(H('b'), '본문 첫 줄\n\n본문 둘째 줄 한글\n'))
out('commit-merge.dat', commit(`${H('b')} ${H('c')}`, ''))
out('commit-root.dat', commit('', 'root\n'))
out('commit-edge.dat', commit(H('b'), '\n\u00a0 \t 공백 둘러싼 \u3000\ufeff\n\n', { an: '', ae: '', at: 'abc', cn: '', ce: '', ct: ' 12 ', s: '제목' + US + '속' }))
out('commit-hex-date.dat', commit(H('b'), 'x', { at: '0x10', ct: '1e3' }))
out('commit-short.dat', ['a', 'b', 'c'].join(NUL))
out('commit-nine.dat', [H('a'), '', 'n', 'e', '1', 'n', 'e', '2', 'subject only'].join(NUL) + NUL)
out('commit-no-terminator.dat', [H('a'), '', 'n', 'e', '1', 'n', 'e', '2', 's', 'body'].join(NUL))
out('commit-extra.dat', commit(H('b'), 'body') + ['extra', 'fields'].join(NUL))

// ---- blame: git blame --porcelain ----
const meta = (name, mail, time, summary, ...more) => [`author ${name}`, `author-mail ${mail}`, `author-time ${time}`, 'author-tz +0900', `committer ${name}`, `committer-mail ${mail}`, `committer-time ${time}`, 'committer-tz +0900', `summary ${summary}`, ...more, 'filename src/a.ts']
out(
  'blame.txt',
  lines(
    `${H('a')} 1 1 2`,
    ...meta('홍길동', '<hong@example.com>', '1700000000', 'feat: 한글 요약'),
    '\tline one',
    `${H('a')} 2 2`,
    '\tline two',
    `${H('b')} 1 3 1`,
    ...meta('Bob Lee', '<bob@example.com>', '1700000100', 'second', `previous ${H('a')} src/a.ts`, 'boundary'),
    '\t\tindented\twith tab',
    `${H('a')} 3 4 1`,
    '\tline four',
    `${H('b')} 7 5 1`,
    '\t',
    `${H('b')} 8 6 1`,
    '\tcrlf\r',
    ''
  )
)
out(
  'blame-edge.txt',
  lines(
    'garbage before the first header',
    '',
    `${'A'.repeat(40)} 1 1 1`, // 대문자 16진수는 머리가 아니다
    `${'a'.repeat(39)} 1 1 1`,
    `${H('a')} 1 1 1 1`,
    `${H('a')} 1 1 1 extra`,
    `${H('a')} 1`,
    `${H('c')} 1 1 1`,
    'author',
    'author-mail <>',
    'author-time abc',
    'unknown-key some value',
    'boundary',
    'boundary',
    '\tfirst\tcontent',
    `${H('d')} 2 2 1`,
    'author-mail <only-open',
    'author-time 0x10',
    '\tsecond',
    `${H('e')} 3 3 1`,
    'author-mail bare@close>',
    'author-time -5',
    '\tthird',
    `${H('f')} 4 4 1`,
    'author-mail <<double>>',
    'author-time 1e3',
    'summary',
    '\tfourth',
    `${'1'.repeat(64)} 5 5`, // 64자 해시(sha256)
    'author-time Infinity',
    '\tbin\0ary',
    `${H('1')} 6 6`, // 40자 해시 — 위의 64자와는 다른 커밋
    '\tx',
    `${H('9')} 7 7 1`,
    'author-time ',
    'author-time  12 ',
    '\tlast without newline'
  )
)
out('blame-dangling.txt', lines(`${H('a')} 1 1 1`, ...meta('n', '<e>', '1', 's')))

// ---- fileHistory: git log --follow -z --name-status -M --format=%x01%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s ----
const fh = (hash, parents, subject, status, at = '1700000000') => `\x01${[hash, parents, '홍길동', 'dev@example.com', at, subject].join(US)}${NUL}\n${status}`
out(
  'filehistory.dat',
  [
    fh(H('a'), H('b'), 'feat: 수정', ['M', 'src/a.ts', ''].join(NUL)),
    fh(H('b'), H('c'), 'rename', ['R100', 'old/한글 경로.ts', 'src/a.ts', ''].join(NUL)),
    `\x01${[H('c'), `${H('d')} ${H('e')}`, 'm', 'm@x', '1700000300', 'Merge branch'].join(US)}${NUL}`,
    fh(H('d'), '', 'root add', ['A', 'old/한글 경로.ts', ''].join(NUL), '1700000400')
  ].join('')
)
out(
  'filehistory-edge.dat',
  [
    'leading garbage without marker',
    fh(H('a'), '', 'empty path', ['M', '', ''].join(NUL), 'abc'),
    fh(H('b'), '', 'rename w/o new path', ['R090', 'old', '', ''].join(NUL)),
    fh(H('c'), '', 'rename missing parts', ['R', ''].join(NUL)),
    fh(H('d'), '', 'status T', ['T', 't.ts', ''].join(NUL)),
    fh(H('e'), '', 'status X unknown', ['X', 'x.ts', ''].join(NUL)),
    fh(H('f'), '', 'many newlines', '\n\n' + ['D', 'gone.ts', ''].join(NUL)),
    `\x01${[H('1'), '', 'n', 'e', '1'].join(US)}${NUL}\nM${NUL}short${NUL}`, // 필드 부족 → 건너뜀
    fh('2'.repeat(64), '', 'sha256 hash', ['M', 'sha256.ts', ''].join(NUL)),
    `\x01${'3'.repeat(65)}${US}p${US}n${US}e${US}1${US}65 hex is not a boundary${NUL}\nM${NUL}no.ts${NUL}`, // 앞 레코드에 붙는다
    fh('4'.repeat(39), '', '39 hex is not a boundary', ['M', 'no2.ts', ''].join(NUL)),
    fh(H('5'), `${H('6')} ${H('7')}`, `제목${US}속${US}구분자 \x01notahash`, ['C050', 'copy-from.ts', 'copy-to.ts', ''].join(NUL)),
    `\x01${[H('8'), '', 'n', 'e', '2', 'no nul in this chunk'].join(US)}`
  ].join('')
)

// ---- submodule ----
out(
  'gitlinks.dat',
  [
    `100644 ${H('b')} 0\tREADME.md`,
    `160000 ${H('a')} 0\tlibs/한글 모듈`,
    `160000 ${H('b')} 0\tvendor/x`,
    `100755 ${H('b')} 0\tbuild.sh`,
    ''
  ].join(NUL)
)
out(
  'gitlinks-edge.dat',
  [
    '',
    `160000 ${H('a')} 1\tconflict`,
    `160000 ${H('b')} 2\tconflict`,
    `160000 ${H('c')} 3\tconflict`,
    `160000 ${H('d')} 0\tconflict`,
    `160000 ${H('a')} 0`, // 탭 없음
    '160000  0\tempty-sha',
    `160000 ${H('a')} 0\t`, // 경로 비어 있음
    `1600001 ${H('a')} 0\tnot-gitlink`,
    '160000 ',
    `160000 ${H('e')}\tno stage`,
    `160000 ${H('f')} 0\ta\tb with tab`,
    `160000 ${H('1')} 0\tlast without nul`
  ].join(NUL)
)
const cfg = (key, value) => `${key}\n${value}`
out(
  'gitmodules.dat',
  [
    cfg('submodule.libs/foo.bar.path', 'libs/foo.bar'),
    cfg('submodule.libs/foo.bar.url', 'https://example.com/a.git'),
    cfg('submodule.libs/foo.bar.branch', 'develop'),
    cfg('submodule.한글 이름.path', 'libs/한글 모듈'),
    cfg('submodule.한글 이름.url', '../rel/x.git'),
    cfg('submodule.nopath.url', 'https://example.com/n.git'),
    'submodule.active\n',
    ''
  ].join(NUL)
)
out(
  'gitmodules-edge.dat',
  [
    cfg('submodule.first.PATH', 'dup/path'),
    cfg('submodule.first.URL', 'u1'),
    cfg('submodule.second.path', 'dup/path'), // 같은 경로를 쓰는 두 번째 이름이 덮어쓴다(자리는 처음 그대로)
    cfg('submodule.second.branch', 'main'),
    cfg('submodule.third.path', 'third'),
    cfg('submodule.third.url', ''), // 빈 url 은 생략
    cfg('submodule.third.update', 'checkout'),
    cfg('other.x.path', 'ignored'),
    cfg('submodule.path', 'no name'),
    cfg('submodule..path', 'empty name'),
    cfg('submodule.nl.path', 'multi\nline value'),
    'submodule.novalue.path',
    cfg('submodule.empty.path', ''),
    cfg('submodule.dotted.name.v2.path', 'dotted'),
    ''
  ].join(NUL)
)
out('probe-ok.txt', `/r/.git/modules/x\n${H('a')}\n`)
out('probe-nohead.txt', '/r/.git\nHEAD\n')
out('probe-crlf-upper.txt', `C:/r/.git\r\n${'ABCDEF'.repeat(6)}ABCD\r\n`)
out('probe-sha256.txt', `/r/.git\n${'a1'.repeat(32)}\n`)
out('probe-bad-length.txt', `/r/.git\n${'a'.repeat(41)}\n`)
out('probe-dir-only.txt', '/r/.git')
out('probe-empty-dir.txt', `\n${H('a')}\n`)
