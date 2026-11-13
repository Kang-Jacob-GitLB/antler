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
