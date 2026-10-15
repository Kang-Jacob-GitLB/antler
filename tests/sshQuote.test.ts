import { describe, expect, it } from 'vitest'
import { GIT_ENV, buildGitArgs } from '../src/main/executors/GitExecutor'
import { buildRemoteCommand, shQuote } from '../src/main/executors/ssh/quote'
import { unquote } from './sshHarness'

const TRICKY = [
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

describe('ssh 인용', () => {
  it.each(TRICKY)('왕복: %j', (s) => {
    expect(unquote(shQuote(s))).toEqual([s])
  })

  it('NUL 은 거부한다', () => {
    expect(() => shQuote('a\0b')).toThrow(/NUL/)
    expect(() => buildRemoteCommand('/r', ['x\0'], {})).toThrow(/NUL/)
  })

  it('작은따옴표 안에 백슬래시를 두지 않는다(fish 호환)', () => {
    for (const s of TRICKY) {
      // 왼쪽부터 스캔해 '…' 구간 안에는 백슬래시가 없고, 바깥의 백슬래시는 \\ 또는 \' 쌍이어야 한다
      const q = shQuote(s)
      for (let i = 0; i < q.length; i++) {
        if (q[i] === "'") {
          const end = q.indexOf("'", i + 1)
          expect(q.slice(i + 1, end)).not.toContain('\\')
          i = end
        } else {
          expect(q[i]).toBe('\\')
          expect(["\\", "'"]).toContain(q[i + 1])
          i++
        }
      }
    }
  })

  it('env … git 명령 전체가 argv 로 복원된다', () => {
    const args = ['log', '--format=%s', '--', '한글 파일.txt', "o'neil"]
    const cmd = buildRemoteCommand("/home/me/my repo's", args, { ...GIT_ENV, EXTRA: 'a b' })
    const argv = unquote(cmd)
    expect(argv[0]).toBe('env')
    const gi = argv.indexOf('git')
    expect(argv.slice(1, gi)).toContain('EXTRA=a b')
    expect(argv.slice(1, gi)).toContain('LC_ALL=C.UTF-8')
    expect(argv.slice(gi + 1)).toEqual(buildGitArgs("/home/me/my repo's", args))
  })

  it('잘못된 환경변수 이름은 거부한다', () => {
    expect(() => buildRemoteCommand('/r', [], { 'A B': '1' })).toThrow()
  })
})
