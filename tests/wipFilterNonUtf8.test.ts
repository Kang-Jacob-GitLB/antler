// 악성 저장소 방어: UTF-8 이 아닌 이름의 filter 드라이버는 `-c` 로 덮어쓸 수 없으므로 작업 트리를 읽지 않아야 한다.
import { execFileSync } from 'node:child_process'
import { appendFileSync, existsSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import type { GitExecutor, RunResult } from '../src/main/executors/GitExecutor'
import { filterOverrides, hasShortNameSegment, parseFilterDrivers, UNSAFE_FILTER_MSG, wipFileDiff, wipStatus } from '../src/main/git/wip'

const buf = (...parts: (string | number[])[]): Buffer => Buffer.concat(parts.map((p) => (typeof p === 'string' ? Buffer.from(p, 'utf8') : Buffer.from(p))))

describe('잘못된 UTF-8 필터 이름 — 실제 저장소', () => {
  let dir: string
  let marker: string
  const git = (...a: string[]): string =>
    execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], { cwd: dir, encoding: 'utf8' })
  let flip = 0
  const dirty = (): void => {
    writeFileSync(join(dir, 'a.txt'), flip++ % 2 ? 'hellp\n' : 'jello\n')
    const t = new Date(Date.now() + 5000 * (flip + 1))
    utimesSync(join(dir, 'a.txt'), t, t)
  }

  beforeAll(() => {
    dir = mkdtempSync(join(tmpdir(), 'antler-wipfilter-ff-'))
    marker = join(dir, 'marker.out').replace(/\\/g, '/')
    // 표지 명령: 임시 디렉터리 안에 파일 하나를 만들 뿐인 node 스크립트
    const script = join(dir, '..', `mark-${process.pid}.cjs`)
    writeFileSync(script, `require('node:fs').writeFileSync(${JSON.stringify(marker)}, 'x')\n`)
    scriptPath = script.replace(/\\/g, '/')
    git('init', '-q', '-b', 'main')
    writeFileSync(join(dir, 'a.txt'), 'hello\n')
    writeFileSync(join(dir, '.gitattributes'), buf('* filter=', [0xff], '\n'))
    git('add', '.')
    git('commit', '-q', '-m', 'init')
    // 커밋한 뒤 저장소 설정에만 심는다(실제 공격과 같다)
    // 설정 값 안의 따옴표는 `\"` 로 써야 남는다(공백이 든 node 경로를 sh 가 한 낱말로 읽게)
    const node = process.execPath.replace(/\\/g, '/')
    appendFileSync(join(dir, '.git', 'config'), buf('[filter "', [0xff], `"]\n\tclean = \\"${node}\\" \\"${scriptPath}\\"; cat\n`))
  })
  let scriptPath = ''
  afterAll(() => {
    rmSync(dir, { recursive: true, force: true })
    rmSync(scriptPath, { force: true })
  })

  it('대조군: 그냥 git status 는 심어 둔 명령을 실행한다(시험이 의미 있음)', () => {
    dirty()
    git('status', '--porcelain')
    expect(existsSync(marker)).toBe(true)
    rmSync(marker, { force: true })
  })

  it('wipStatus·wipFileDiff 는 실행하지 않고 "안전하게 무력화할 수 없음" 으로 거부한다', async () => {
    const exec = new LocalExecutor()
    dirty()
    await expect(wipStatus(exec, dir)).rejects.toThrow(UNSAFE_FILTER_MSG)
    await expect(wipFileDiff(exec, dir, { area: 'unstaged', path: 'a.txt' })).rejects.toThrow(UNSAFE_FILTER_MSG)
    expect(existsSync(marker)).toBe(false)
  })
})

describe('parseFilterDrivers', () => {
  it('잘못된 UTF-8 키(필터 이름·접미사 어디든)와 U+FFFD 는 던진다', () => {
    expect(() => parseFilterDrivers(buf('filter.', [0xff], '.clean\ncmd\0'))).toThrow(UNSAFE_FILTER_MSG)
    expect(() => parseFilterDrivers(buf('filter.x.', [0xc3, 0x28], '\ncmd\0'))).toThrow(UNSAFE_FILTER_MSG)
    expect(() => parseFilterDrivers(buf('filter.�.clean\ncmd\0'))).toThrow(UNSAFE_FILTER_MSG)
  })

  it('유효한 비 ASCII 이름은 그대로 통과하고, 값의 잘못된 바이트는 무시한다', () => {
    expect(parseFilterDrivers(buf('filter.한글.clean\ncmd\0'))).toEqual(['한글'])
    expect(parseFilterDrivers(buf('filter.x.clean\ncmd', [0xff], '\0'))).toEqual(['x'])
  })

  it('접미사는 JS 에서 판정한다: required 등은 건너뛰고 lfs 기본값은 제외', () => {
    const out = parseFilterDrivers(buf('filter.a.required\ntrue\0filter.a.smudge\nx\0filter.b.process\ny\0filter.lfs.clean\ngit-lfs clean -- %f\0filter.c\0'))
    expect(out.sort()).toEqual(['a', 'b'])
  })

  it('이름에 "=" 가 있으면 던진다', () => {
    expect(() => parseFilterDrivers(buf('filter.a=b.clean\ncmd\0'))).toThrow(UNSAFE_FILTER_MSG)
  })

  it('드라이버 수 상한(256): 256개는 통과, 257개는 던진다', () => {
    const mk = (n: number): Buffer => buf(...Array.from({ length: n }, (_, i) => `filter.d${i}.clean\ncmd\0`))
    expect(parseFilterDrivers(mk(256))).toHaveLength(256)
    expect(() => parseFilterDrivers(mk(257))).toThrow(UNSAFE_FILTER_MSG)
  })
})

describe('filterOverrides', () => {
  const fake = (stdout: Buffer, code = 0): GitExecutor =>
    ({
      kind: 'local',
      run: async (_p: string, args: string[]): Promise<RunResult> => {
        // 로캘·잘못된 바이트에 기대지 않도록 조회 정규식이 `^filter\.` 로 넓어야 한다
        expect(args).toEqual(['config', '-z', '--get-regexp', '^filter\\.'])
        return { stdout, stderr: '', code }
      },
      stream: () => {
        throw new Error('unused')
      },
      dispose: async () => {}
    }) as GitExecutor

  it('키가 없으면(code 1) 빈 목록', async () => {
    expect(await filterOverrides(fake(Buffer.alloc(0), 1), '/r')).toEqual([])
  })

  it('명령줄 총 길이 상한을 넘기면 던진다(긴 이름 몇 개)', async () => {
    const long = 'n'.repeat(3000)
    const out = buf(...Array.from({ length: 10 }, (_, i) => `filter.${long}${i}.clean\ncmd\0`))
    await expect(filterOverrides(fake(out), '/r')).rejects.toThrow(UNSAFE_FILTER_MSG)
  })

  it('정상 드라이버는 -c 4쌍으로 덮는다', async () => {
    expect(await filterOverrides(fake(buf('filter.x.clean\ncmd\0')), '/r')).toEqual([
      '-c',
      'filter.x.clean=',
      '-c',
      'filter.x.smudge=',
      '-c',
      'filter.x.process=',
      '-c',
      'filter.x.required=false'
    ])
  })
})

describe('hasShortNameSegment', () => {
  it('8.3 모양을 잡고, 끝의 점·공백이 붙은 것도 잡는다', () => {
    for (const p of ['GIT~1/config', 'a/GIT~1/config', 'x\\GIT~12\\config', 'GIT~1./config', 'GIT~1 /config', 'a/GIT~1. ./c', 'GIT~1...']) {
      expect(hasShortNameSegment(p), p).toBe(true)
    }
  })
  it('일반 경로는 통과', () => {
    for (const p of ['a/b.txt', 'foo.~1x', 'dir/file', '~/x', 'a~b']) expect(hasShortNameSegment(p), p).toBe(false)
  })
})
