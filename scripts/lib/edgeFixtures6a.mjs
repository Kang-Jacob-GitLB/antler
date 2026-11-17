// Phase 6a PR 1 골든 입력: 코드 탐색기·LFS·gitflow·활동 집계·기본 브랜치·WIP 경로 검증의 순수 함수 입력(git 을 실행하지 않는다).
// make-edge-fixtures.mjs 가 부른다. `.bin` 은 nulRecords 골든이 전부 돌리므로 피한다(.dat·.txt·.json).
// JSON 은 입력 목록(`[케이스, …]`)이고 골든이 `{in, out}` 로 짝을 남긴다.

/** @param {{ out: (name: string, data: string | Buffer) => void, H: (c: string) => string, NUL: string }} ctx */
export function make6aFixtures({ out, H, NUL }) {
  const json = (name, cases) => out(name, `${JSON.stringify(cases, null, 1)}\n`)
  const cfgs = (...pairs) => pairs.map(([k, v]) => `${k}\n${v}${NUL}`).join('')

  // ---- tree: ls-tree -z 출력. 대소문자·디렉터리 우선·비 BMP·소문자화로 길이가 달라지는 İ/ẞ·UTF-16 순서·건너뛸 레코드·깨진 UTF-8·마지막 NUL 없음 ----
  {
    const oid = H('a')
    const rec = (mode, type, name) => Buffer.from(`${mode} ${type} ${oid}\t${name}${NUL}`, 'utf8')
    out(
      'tree-edge.dat',
      Buffer.concat([
        rec('100644', 'blob', 'B.txt'),
        rec('100644', 'blob', 'a.txt'),
        rec('040000', 'tree', 'zdir'),
        rec('100644', 'blob', 'A.txt'),
        rec('040000', 'tree', 'Adir'),
        rec('100644', 'blob', 'b.txt'),
        rec('100644', 'blob', '\u0130stanbul'), // İ → 'i' + U+0307
        rec('100644', 'blob', 'istanbul'),
        rec('100644', 'blob', 'istanbuz'),
        rec('100644', 'blob', '\u1e9e.txt'), // ẞ → ß
        rec('100644', 'blob', '\u00df.txt'),
        rec('100644', 'blob', '\u{1d49c}.txt'), // 비 BMP: UTF-16 으로는 U+FF5E 보다 앞
        rec('100644', 'blob', '\uff5e.txt'),
        rec('100644', 'blob', '한글.txt'),
        rec('100644', 'blob', '가나다'),
        rec('100644', 'blob', '\u03a3\u03a3'), // 단어 끝이 아닌 대문자 시그마
        rec('100644', 'blob', 'A\u03a3'), // 단어 끝 Σ → ς
        rec('100644', 'blob', 'a\u03c2'),
        rec('100755', 'blob', 'run.sh'),
        rec('120000', 'blob', 'link'),
        rec('160000', 'commit', 'sub'),
        rec('100644', 'blob', '--output=x.txt'),
        rec('100644', 'blob', '-rf'),
        rec('100644', 'blob', 'with\ttab'),
        rec('100644', 'blob', 'sp ace'),
        Buffer.from(`100644 blob ${oid} no-tab${NUL}`), // 탭 없음
        Buffer.from(`100644 blob\t${oid}x${NUL}`), // oid 없음
        Buffer.from(` blob ${oid}\tempty-mode${NUL}`),
        Buffer.from(`100644 blob ${oid}\t${NUL}`), // 이름 없음
        Buffer.from(`100644 blob ${oid} extra field\tmore-fields${NUL}`),
        Buffer.from(`${NUL}${NUL}`),
        Buffer.from(`100644 blob ${oid}\tbad-`),
        Buffer.from([0xff, 0xfe, 0xe3, 0x81, 0x00]),
        Buffer.from(`100644 blob ${oid}\ttruncated-last`) // 마지막 NUL 없음
      ])
    )
  }

  // ---- LFS 포인터: CRLF·확장 키·중복 키·15자리 크기·1024(UTF-16 단위) 경계 ----
  {
    const URL = 'https://git-lfs.github.com/spec/v1'
    const OID = '4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393'
    const ptr = (extra = '', size = '12345') => `version ${URL}\noid sha256:${OID}\nsize ${size}\n${extra}`
    const base = ptr()
    // 전체 길이가 정확히 `units` UTF-16 단위가 되도록 ext 줄을 채운다(`ch` 의 단위 수로 나눠 떨어져야 한다)
    const padTo = (units, ch) => {
      const head = `${base}ext-0-x `
      const fill = units - head.length - 1
      return `${head}${ch.repeat(fill / ch.length)}\n`
    }
    json('lfs-pointer-cases.json', [
      ptr(),
      ptr().replace(/\n/g, '\r\n'),
      ptr().trimEnd(),
      ptr('ext-0-foo sha256:abc\next-1-bar x\n'),
      `version ${URL}\nsize 7\noid sha256:${OID}\n`,
      ptr('unknown value\n'),
      ptr(`oid sha256:${OID}\n`),
      ptr('size 1\n'),
      `version ${URL}\noid sha256:${OID}\n`,
      `version ${URL}\nsize 1\n`,
      `version ${URL}\nsize 1\nx 1\n`,
      ptr('', '0'),
      ptr('', '007'),
      ptr('', '999999999999999'),
      ptr('', '1234567890123456'),
      ptr('', '-1'),
      ptr('', '1.5'),
      ptr('', '１２'),
      ptr('', ''),
      ptr('', '12 '),
      ptr().replace(OID, OID.toUpperCase()),
      ptr().replace(OID, OID.slice(1)),
      ptr().replace('sha256', 'md5'),
      ptr().replace(`version ${URL}`, 'version https://example.com/v1'),
      ptr().replace('version', 'Version'),
      `version ${URL}\n`,
      '',
      '\n',
      ptr('\n'),
      ptr(' x\n'),
      ptr('novalue\n'),
      `${ptr().trimEnd()}\r`, // 끝의 단독 \r 은 값에 남는다
      ptr().replace('\noid', '\r\noid'),
      padTo(1024, 'x'),
      padTo(1025, 'x'),
      padTo(1024, '가'), // UTF-16 으로 1024 단위지만 UTF-8 로는 1024 바이트를 넘는다
      padTo(1025, '가'),
      padTo(1024, '😀'),
      padTo(1022, '😀'),
      `\ufeff${ptr()}` // BOM 이 앞에 있으면 첫 줄이 어긋난다
    ])
  }

  json('check-attr-cases.json', [
    '',
    'a.bin\0filter\0lfs\0',
    'a.bin\0filter\0lfs\0b.txt\0filter\0unspecified\0c d\ne.psd\0filter\0lfs\0',
    'a\0filter\0lfs', // 마지막 NUL 없음
    'a\0filter\0',
    'a\0filter',
    'a\0diff\0lfs\0',
    'a\0filter\0LFS\0',
    'a\0filter\0lfs2\0',
    'filter\0filter\0lfs\0',
    '\0filter\0lfs\0',
    '한글 경로.psd\0filter\0lfs\0',
    'x\0filter\0lfs\0y\0filter\0lfs\0z\0filter\0set\0'
  ])

  json('added-lines-cases.json', [
    '',
    'diff --no-index a b\n--- /dev/null\n+++ b/x\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n',
    '+++ b/x\n+before hunk\n@@ -0,0 +1 @@\n+only\n',
    '@@ -0,0 +1,2 @@\n+a\n-b\n c\n+d',
    '@@ x\n+\n+\n+',
    '@@ a @@\n++double\n+ spaced\n+\ttab\n',
    '@@\r\n+crlf\r\n+x\r\n',
    '@@ -1 +1 @@\n\\ No newline at end of file\n+x',
    'Binary files /dev/null and b/x differ'
  ])

  json('gitattributes-cases.json', [
    '',
    '*.psd filter=lfs diff=lfs merge=lfs -text\n',
    '# *.psd filter=lfs\n',
    '  # indented comment filter=lfs\n',
    '*.bin\tfilter=lfs\r\n',
    'filter=lfs',
    'filter=lfs\n',
    '*.a filter=lfsx\n*.b xfilter=lfs\n*.c filter=lfs2\n*.d filter=lfs-extra\n',
    '*.a filter==lfs\n',
    '*.a filter = lfs\n',
    '*.a -filter=lfs\n',
    '*.a filter=LFS\n',
    'a filter=lfsx filter=lfs',
    '*.a\u0085filter=lfs\n', // U+0085 는 JS \s 가 아니다
    '*.a\ufefffilter=lfs\u00a0\n',
    '*.a\u2028filter=lfs\u3000\n',
    '\u0085# not trimmed so not a comment\u0085filter=lfs\n',
    '\ufeff# bom comment filter=lfs\n',
    'x\n\n   \n# c\n*.z filter=lfs\n',
    '*.a filter=lfs\u0085\n',
    '*.a diff=lfs\n'
  ])

  // ---- gitflow 설정: 제어 문자·U+2028·100(UTF-16 단위) 경계·같은 키 덮어쓰기·키 대소문자 ----
  json('gitflow-cases.json', [
    '',
    cfgs(
      ['gitflow.branch.master', 'main'],
      ['gitflow.branch.develop', 'dev'],
      ['gitflow.prefix.feature', 'f/'],
      ['gitflow.prefix.release', 'r/'],
      ['gitflow.prefix.hotfix', 'h/'],
      ['gitflow.prefix.bugfix', 'b/'],
      ['gitflow.prefix.support', 's/']
    ),
    cfgs(['GitFlow.Branch.Master', 'M'], ['GITFLOW.PREFIX.FEATURE', 'F/']),
    cfgs(['gitflow.branch.master', 'a'], ['gitflow.branch.master', 'b']),
    cfgs(['gitflow.branch.master', 'a'], ['gitflow.branch.master', 'bad\u0007']),
    cfgs(['gitflow.prefix.nope', 'x'], ['other.key', 'y'], ['gitflow.branch', 'z'], ['gitflow.prefix.', 'z']),
    cfgs(['gitflow.branch.master', 'x'.repeat(100)]),
    cfgs(['gitflow.branch.master', 'x'.repeat(101)]),
    cfgs(['gitflow.branch.master', '가'.repeat(100)]),
    cfgs(['gitflow.branch.master', '😀'.repeat(50)]),
    cfgs(['gitflow.branch.master', '😀'.repeat(51)]),
    cfgs(['gitflow.branch.master', 'a\nb'], ['gitflow.branch.develop', 'a\tb']),
    cfgs(['gitflow.branch.master', 'a\u007f'], ['gitflow.branch.develop', 'a\u009f']),
    cfgs(['gitflow.prefix.feature', 'a\u2028b'], ['gitflow.prefix.release', 'a\u2029b']),
    cfgs(['gitflow.prefix.feature', 'a\u0085b'], ['gitflow.prefix.release', '\u00a0'], ['gitflow.prefix.hotfix', ' '], ['gitflow.prefix.bugfix', '\ufeff']),
    cfgs(['gitflow.prefix.feature', ' pad '], ['gitflow.prefix.release', 'a\u00a0']),
    cfgs(['gitflow.branch.master', '']),
    'gitflow.branch.master\0gitflow.branch.develop\nok\0no-newline',
    `\n${NUL}gitflow.branch.develop\nok${NUL}`,
    'gitflow.branch.master\nno terminator'
  ])

  // ---- 활동 집계: 시간 스트림(청크 분할·2^32 감김)과 구간 집계 ----
  out('time-stream.txt', '1700000000\n1700000060\n\n 17000 00\nabc123def\n4294967295\n4294967296\n4294967297\n18446744073709551616\n99999999999999999999999\n0\n007\n9007199254740993\n1700003600')
  out('time-stream-huge.txt', `${'9'.repeat(400)}\n5\n${'1'.repeat(310)}\n`)

  {
    const NOW = 1700000000
    const H1 = 3600
    const spread = (start, count, step) => Array.from({ length: count }, (_, i) => start + i * step)
    json('bucketize-cases.json', [
      { times: [], now: NOW },
      { times: [NOW - 5], now: NOW },
      { times: [NOW - 5, NOW - 5, NOW - 5], now: NOW },
      { times: spread(NOW - 49 * 60, 50, 60), now: NOW },
      { times: spread(1400000000, 600, 7 * 86400 - 13), now: NOW }, // 수년 → 주 이상 단위
      { times: [0, NOW - 100, 4000000000, NOW - 3600], now: NOW }, // 0·먼 미래는 total 에만
      { times: [0, 4000000000], now: NOW }, // 전부 범위 밖
      { times: [NOW + 172800, NOW + 172801], now: NOW }, // now + 2일은 안, 1초 더는 밖
      { times: [NOW + 172801], now: NOW },
      { times: [NOW - 399 * H1, NOW], now: NOW }, // 1시간 단위로 정확히 400 구간
      { times: [NOW - 399 * H1 - 1, NOW], now: NOW }, // 400 구간을 넘으면 6시간 단위
      { times: [NOW - 399 * 6 * H1, NOW], now: NOW },
      { times: [NOW - 399 * 6 * H1 - 1, NOW], now: NOW },
      { times: [NOW - 399 * 86400, NOW], now: NOW },
      { times: [NOW - 399 * 86400 - 1, NOW], now: NOW },
      { times: [NOW - 399 * 7 * 86400, NOW], now: NOW },
      { times: [NOW - 399 * 7 * 86400 - 1, NOW], now: NOW },
      { times: [NOW - 399 * 30 * 86400, NOW], now: NOW },
      { times: [NOW - 399 * 30 * 86400 - 1, NOW], now: NOW },
      { times: [1, NOW], now: NOW }, // 1970 부터
      { times: [4294967295, 4294967000, 4294960000], now: 4294967200 },
      { times: spread(NOW - 3 * 86400, 300, 900), now: NOW + 0.5 },
      { times: [86399, 86400, 86401], now: 86400 } // 단위 경계에 걸친 값
    ])
  }

  // ---- 기본 브랜치 ----
  {
    const br = (name, hash, full) => ({ name, fullName: full ?? `refs/heads/${name}`, hash })
    const rb = (remote, name, hash) => br(`${remote}/${name}`, hash, `refs/remotes/${remote}/${name}`)
    const group = (name, branches) => ({ name, branches })
    const origin = [group('origin', [rb('origin', 'main', 'R1'), rb('origin', 'dev', 'R2')])]
    json('default-branch-cases.json', [
      { originHead: 'refs/remotes/origin/main', local: [br('main', 'L1')], remotes: origin },
      { originHead: 'refs/remotes/origin/main\n', local: [br('main', 'L1')], remotes: origin },
      { originHead: '  refs/remotes/origin/dev \r\n', local: [], remotes: origin },
      { originHead: '\ufeffrefs/remotes/origin/main', local: [br('main', 'L1')], remotes: origin }, // BOM 도 JS trim 이 지운다
      { originHead: '\u0085refs/remotes/origin/main', local: [br('main', 'L1')], remotes: origin }, // U+0085 는 지우지 않는다
      { originHead: '\u00a0refs/remotes/origin/main\u3000', local: [br('main', 'L1')], remotes: origin },
      { originHead: 'refs/remotes/origin/gone', local: [br('master', 'L2'), br('main', 'L1')], remotes: origin },
      { originHead: 'refs/remotes/origin/gone', local: [br('master', 'L2')], remotes: origin },
      { originHead: 'refs/heads/main', local: [br('master', 'L2')], remotes: origin },
      { originHead: 'origin/main', local: [br('main', 'L1')], remotes: origin },
      { originHead: '', local: [br('master', 'L2')], remotes: [] },
      { originHead: null, local: [br('main', 'L1'), br('master', 'L2')], remotes: [] },
      { originHead: null, local: [br('develop', 'L3')], remotes: origin },
      { originHead: null, local: [], remotes: [] },
      { originHead: 'refs/remotes/up/main', local: [], remotes: [group('origin', [rb('origin', 'main', 'R1')]), group('up', [rb('up', 'x', 'U0'), rb('up', 'main', 'U1')])] },
      { originHead: 'refs/remotes/origin/main', local: [], remotes: [group('origin', [rb('origin', 'main', 'first')]), group('dup', [rb('origin', 'main', 'second')])] },
      { originHead: 'refs/remotes/', local: [br('main', 'L1')], remotes: origin },
      { originHead: 'refs/remotes/origin/main', local: [br('Main', 'L9'), br('master', 'L2')], remotes: [] }
    ])
  }

  // ---- filter 드라이버 무력화: `git config -z --get-regexp` 출력(각 케이스가 stdout 전체) ----
  {
    const LFS = [
      ['filter.lfs.clean', 'git-lfs clean -- %f'],
      ['filter.lfs.smudge', 'git-lfs smudge -- %f'],
      ['filter.lfs.process', 'git-lfs filter-process']
    ]
    json('filter-overrides-cases.json', [
      '',
      cfgs(...LFS),
      cfgs(['filter.lfs.smudge', 'git-lfs smudge --skip -- %f'], ['filter.lfs.process', 'git-lfs filter-process --skip']),
      cfgs(['filter.lfs.clean', '  git-lfs clean -- %f \r\n'], ['filter.lfs.smudge', '\ufeffgit-lfs smudge -- %f\u00a0\u3000']),
      cfgs(['filter.lfs.clean', 'git-lfs clean -- %f\u0085']),
      cfgs(['filter.lfs.clean', 'git-lfs clean -- %f; touch pwned']),
      cfgs(['filter.lfs.clean', 'git-lfs clean -- %f'], ['filter.lfs.smudge', 'git-lfs smudge -- %f && echo x']),
      cfgs(['filter.evil.clean', 'sh -c "touch x"']),
      cfgs(['filter.b.clean', 'x'], ['filter.a.smudge', 'y'], ['filter.b.process', 'z'], ['filter.a.clean', 'git-lfs clean -- %f']),
      cfgs(['filter.a.b.clean', 'x'], ['filter.c.clean.process', 'y']),
      cfgs(['filter.MyDrv.clean', 'x'], ['Filter.x.clean', 'x'], ['FILTER.y.clean', 'x']),
      cfgs(['filter..clean', 'x'], ['filter.a.required', 'x'], ['filter.a', 'x'], ['xfilter.a.clean', 'x'], ['filter.a.cleans', 'x'], ['filter.clean', 'x']),
      cfgs(['filter.a=b.clean', 'x']),
      cfgs(['filter.a=b.clean', 'git-lfs clean -- %f']),
      cfgs(['filter.ok.clean', 'x'], ['filter.a=b.smudge', 'y']),
      cfgs(['filter.=.process', 'x']),
      cfgs(['filter.한글 드라이버.clean', 'x']),
      cfgs(['filter.sp ace.clean', '']),
      'filter.a.clean\nmulti\nline value\0',
      'filter.a.clean\0\0filter.b.smudge\0',
      'filter.a\nb.clean\nvalue\0', // 개행이 든 서브섹션: 첫 개행에서 키가 잘려 드라이버로 보이지 않는다
      'filter.nonl.process',
      `${cfgs(['filter.a.clean', 'x'])}trailing without nul`,
      // 잘못된 UTF-8 키는 `-c` 로 덮을 수 없으니 거부한다(#114). 케이스는 `{parts}`: 문자열 | {hex} | {repeat: [단위, 횟수]}({i} = 번호)
      { parts: ['filter.', { hex: 'ff' }, `.clean\ncmd${NUL}`] },
      { parts: ['filter.x.', { hex: 'c328' }, `\ncmd${NUL}`] },
      { parts: [`filter.ok.clean\nx${NUL}other.`, { hex: 'ff' }, `\nv${NUL}`] }, // 드라이버 키가 아니어도 키가 깨졌으면 거부
      { parts: ['filter.', { hex: 'e0a080' }, `.clean\ncmd${NUL}`] }, // 유효한 3바이트(U+0800)
      { parts: ['filter.', { hex: 'c0af' }, `.clean\ncmd${NUL}`] }, // 과잉 인코딩
      { parts: ['filter.', { hex: 'eda080' }, `.clean\ncmd${NUL}`] }, // 서로게이트
      { parts: ['filter.', { hex: 'f4908080' }, `.clean\ncmd${NUL}`] }, // U+110000
      { parts: ['filter.x.clean\ncmd', { hex: 'ff' }, NUL] }, // 값의 잘못된 바이트는 무시
      { parts: ['filter.x.clean\n', { hex: 'ff' }, NUL, 'filter.y.smudge\nz', NUL] },
      { parts: ['filter.a.clean\nx', NUL, 'filter.', { hex: 'fe' }, '.clean\ny', NUL] },
      'filter.�.clean\ncmd\0',
      'filter.x.clean\n�\0',
      cfgs(['filter.\u{1f600}.clean', 'x']),
      // 드라이버 수 상한 256: 같은 이름의 반복은 세지 않는다(lfs 기본값도 드라이버로 센다)
      { parts: [{ repeat: [`filter.d{i}.clean\ncmd${NUL}`, 256] }] },
      { parts: [{ repeat: [`filter.d{i}.clean\ncmd${NUL}`, 257] }] },
      { parts: [{ repeat: [`filter.d{i}.clean\ncmd${NUL}filter.d{i}.smudge\ncmd${NUL}`, 256] }] },
      { parts: [{ repeat: [`filter.d{i}.clean\ncmd${NUL}`, 256] }, `filter.lfs.clean\ngit-lfs clean -- %f${NUL}`] },
      { parts: [{ repeat: [`filter.d{i}.clean\ncmd${NUL}`, 256] }, `filter.extra.clean\ncmd${NUL}`] },
      // 명령줄 총 길이 상한 24000(UTF-16 단위): 드라이버 하나의 `-c` 묶음은 83 + 4×이름 길이다
      { parts: ['filter.', { repeat: ['a', 2979] }, `.clean\ncmd${NUL}filter.`, { repeat: ['b', 2979] }, `.clean\ncmd${NUL}`] }, // 23998
      { parts: ['filter.', { repeat: ['a', 2979] }, `.clean\ncmd${NUL}filter.`, { repeat: ['b', 2980] }, `.clean\ncmd${NUL}`] }, // 24002
      // 이름이 비 BMP 면 UTF-16 단위로 센다(문자 수로 세면 둘 다 통과한다)
      { parts: ['filter.', { repeat: ['\u{1f600}', 1489] }, `a.clean\ncmd${NUL}filter.`, { repeat: ['b', 2979] }, `.clean\ncmd${NUL}`] }, // 23998
      { parts: ['filter.', { repeat: ['\u{1f600}', 1490] }, `.clean\ncmd${NUL}filter.`, { repeat: ['b', 2979] }, `.clean\ncmd${NUL}`] } // 24002
    ])
  }

  // ---- WIP 경로 검증(assertRelPath)·8.3 구성 요소·서브모듈 경로 ----
  json('rel-path-cases.json', [
    'a.txt',
    'a/b/c.txt',
    '한글/파일 이름.txt',
    'a\\b',
    '.gitignore',
    'dir/.github/x',
    'a/b/',
    '..a',
    'a..',
    '...',
    '~1',
    'C',
    '',
    '..',
    '../x',
    'a/..',
    'a/../b',
    'a\\..\\b',
    '.git',
    '.GIT',
    '.Git/config',
    'a/.git/x',
    'a\\.GIT',
    'a/.git',
    '.git/',
    '/abs',
    '\\abs',
    '//server/share',
    'C:/x',
    'c:x',
    'z:',
    '1:/x',
    'ab:/x',
    'a\0b',
    '\0',
    'GIT~1/config', // 문자열 검사로는 못 막는다(허용 목록·링크 확인 몫)
    '.git./config',
    '.git /config',
    '.git\u00a0/x',
    '.G\u0130T/x', // İ 는 소문자화하면 'i̇' 라 `.git` 이 아니다
    '.g\u0131t/x',
    '.\u212aEEP', // Kelvin 기호
    'a'.repeat(4096),
    'a'.repeat(4097),
    '가'.repeat(4096),
    '가'.repeat(4097),
    '😀'.repeat(2048),
    '😀'.repeat(2049),
    `${'a'.repeat(4094)}/b`,
    `${'a'.repeat(4095)}/b`,
    5,
    null
  ])
  json('short-name-cases.json', ['', 'GIT~1', 'a/GIT~1/config', 'x\\PROGRA~2\\y', 'foo~12', '~1', 'a~0', 'a~', 'a~b', 'a~1b', '~', 'a/b', 'x.git~', '~１', 'a~1/', 'a~1\\', 'a/b~2/c', '한글~1', 'a~ 1', 'a~1 ', 'GIT~1.', 'GIT~1 ', 'GIT~1./config', 'GIT~1 /config', 'a/GIT~1. ./c', 'GIT~1...', 'GIT~1\\ ', 'x\\GIT~12.\\y', '.', ' ', '~. ', '~.', 'a~1.b', 'a~ .', 'a~1\t', 'a~1 ', 'a~1　'])
  json('submodule-path-cases.json', [
    'sub',
    'a/b',
    'a\\b',
    '한글/모듈',
    'a.b/.c',
    '...',
    'a/..b',
    '',
    '/abs',
    '\\abs',
    'C:/x',
    'c:x',
    'z:',
    '1:/x',
    '../x',
    'a/../b',
    'a\\..\\b',
    'a//b',
    'a\\\\b',
    'a/',
    './a',
    'a/.',
    '.',
    '..',
    '//host/share',
    '..\\x',
    '.git',
    'a/.git/x'
  ])

  // ---- 코드 탐색기 입력 검증: 전체 커밋 해시·디렉터리/파일 경로·resolveRev 의 ref 모양·이미지 확장자 ----
  {
    const h = (c, n = 40) => c.repeat(n)
    json('commit-id-cases.json', [
      h('a'),
      h('A'),
      'aB'.repeat(20),
      h('f', 64),
      h('F', 64),
      h('a', 39),
      h('a', 41),
      h('a', 63),
      h('a', 65),
      h('a', 24),
      '',
      'abc',
      h('g'),
      `${h('a')}\n`,
      `\n${h('a')}`,
      ` ${h('a')}`,
      `--${h('a', 38)}`,
      `${h('a', 39)}\u0661`, // 아랍 숫자 1
      `${h('a', 39)}\uff41`, // 전각 a
      `${h('a', 39)}\u017f`, // ſ(긴 s) — /i 로도 hex 가 아니다
      5,
      null
    ])
    json('normalize-dir-cases.json', [
      '',
      '/',
      '///',
      '/a/',
      'a/b',
      '/src/a//',
      'a//b',
      'a/./b',
      'a/../b',
      '..',
      '.',
      '/./',
      '/../',
      'a/..',
      '../a',
      'a\0b',
      '\0',
      '한글/디렉터리',
      'a\\b',
      'a\\..\\b',
      ' ',
      ' a ',
      'a/ /b',
      '...',
      '..a',
      'a b/c d',
      '.git',
      'a/.git/b',
      5,
      null
    ])
    json('resolve-ref-cases.json', [
      'HEAD',
      'head',
      'Head',
      'HEAD~1',
      'HEAD^',
      'HEAD@{1}',
      'main',
      'abcd',
      'ABCD',
      'abc',
      'a'.repeat(64),
      'A'.repeat(64),
      'a'.repeat(65),
      h('a'),
      'refs/heads/main',
      'refs/heads/a/b',
      'refs/heads/한글',
      'refs/tags/v1.0',
      'refs/remotes/origin/main',
      'refs/remotes/origin/feature/x',
      'refs/remotes/origin/*',
      'refs/remotes/*',
      'refs/heads/*',
      'refs/heads/',
      'refs/heads',
      'refs/stash',
      'refs/heads/x y',
      'refs/heads/..',
      'refs/heads/a..b',
      'refs/heads/x.lock',
      'refs/heads/@{x}',
      'refs/heads/a//b',
      'refs/heads/.hidden',
      'refs/heads/x/',
      `refs/heads/${'a'.repeat(1011)}`,
      `refs/heads/${'a'.repeat(1012)}`,
      '--output=x',
      '-x',
      '--end-of-options',
      '',
      ' HEAD',
      'HEAD\n',
      'HEAD\0'
    ])
    // `constructor`·`toString`·`__proto__` 같은 확장자는 넣지 않는다 — TS 는 객체 조회라 함수가 잡히는 결함이 있고 Rust 는 None 이다(docs/tauri.md)
    json('image-mime-cases.json', [
      'a.png',
      'A.PNG',
      'dir/x.JPG',
      'x.jpeg',
      'x.gif',
      'x.webp',
      'x.bmp',
      'x.ico',
      'x.svg',
      'x.SVG',
      'x.sVg',
      'x.avif',
      'noext',
      '.png',
      'a.',
      'dir.png/file',
      'dir.png\\file',
      'a.png.txt',
      'a.tar.gz',
      'a.b.PNG',
      'x.pn',
      'x.pngg',
      'x.\u0130cx',
      'x.\u017fvg',
      'x.p\u0131ng',
      '한글.PNG',
      'a/b/c/.avif',
      ''
    ])
  }
}
