// 악성 저장소 방어: .git/config 에 심은 filter 드라이버(clean)가 WIP status·diff 에서 실행되지 않아야 한다.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { filterOverrides, wipFileDiff, wipStatus } from '../src/main/git/wip'

let dir: string
let marker: string
const git = (...a: string[]): string =>
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], { cwd: dir, encoding: 'utf8' })

/**
 * 크기는 같고 내용만 다르게 바꾸고 mtime 을 옮긴다. 크기가 다르면 git 이 내용을 읽지 않고 "수정됨"으로 판단하므로,
 * 같은 크기여야 내용을 해시하려고 clean 필터를 돈다.
 */
let flip = 0
function dirty(): void {
  writeFileSync(join(dir, 'a.txt'), flip++ % 2 ? 'hellp\n' : 'jello\n')
  const t = new Date(Date.now() + 5000)
  utimesSync(join(dir, 'a.txt'), t, t)
}

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), 'antler-wipfilter-'))
  marker = join(dir, '..', `pwned-${process.pid}-${Date.now()}`).replace(/\\/g, '/')
  git('init', '-q', '-b', 'main')
  writeFileSync(join(dir, 'a.txt'), 'hello\n')
  writeFileSync(join(dir, '.gitattributes'), '*.txt filter=evil\n')
  git('add', '.')
  git('commit', '-q', '-m', 'init')
  // 커밋한 뒤에 심는다(실제 공격처럼 저장소 설정에만 있다). git 은 필터를 sh 로 실행한다
  git('config', 'filter.evil.clean', `touch "${marker}"; cat`)
  git('config', 'filter.evil.required', 'true')
})

afterAll(() => {
  rmSync(dir, { recursive: true, force: true })
  rmSync(marker, { force: true })
})

describe('WIP filter 드라이버 무력화', () => {
  it('대조군: 그냥 git status 를 돌리면 심어 둔 명령이 실행된다(테스트가 의미 있음을 확인)', () => {
    dirty()
    git('status', '--porcelain')
    expect(existsSync(marker)).toBe(true)
    rmSync(marker, { force: true })
  })

  it('wipStatus·wipFileDiff 는 명령을 실행하지 않고, required 필터여도 실패하지 않는다', async () => {
    const exec = new LocalExecutor()
    dirty()
    const st = await wipStatus(exec, dir)
    expect(st.unstaged.map((f) => f.path)).toContain('a.txt')
    const d = await wipFileDiff(exec, dir, { area: 'unstaged', path: 'a.txt' })
    expect(d.hunks.length).toBeGreaterThan(0)
    expect(existsSync(marker)).toBe(false)
  })

  it('git-lfs 기본 명령은 덮어 쓰지 않는다', async () => {
    git('config', 'filter.lfs.clean', 'git-lfs clean -- %f')
    git('config', 'filter.lfs.process', 'git-lfs filter-process')
    const args = await filterOverrides(new LocalExecutor(), dir)
    expect(args.join(' ')).toContain('filter.evil.clean=')
    expect(args.join(' ')).not.toContain('filter.lfs.')
  })

  it('git-lfs 처럼 보이게 뒤에 명령을 붙인 값은 덮어 쓴다', async () => {
    git('config', 'filter.lfs.clean', 'git-lfs clean -- %f; touch x')
    const args = await filterOverrides(new LocalExecutor(), dir)
    expect(args).toContain('filter.lfs.clean=')
  })
})
