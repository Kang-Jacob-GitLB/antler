// 악성 저장소 방어: `git diff --no-index` 도 `-C` 저장소의 filter 드라이버(clean)를 돈다 — repoExtras 파일 읽기·wip untracked diff 가 막아야 한다.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { repoExtras } from '../src/main/git/repoExtras'
import { wipFileDiff, wipStatus } from '../src/main/git/wip'

const root = mkdtempSync(join(tmpdir(), 'antler-noindex-'))
const fwd = (p: string): string => p.replace(/\\/g, '/')
// 표지 명령: 표지 파일을 만들고 stdin 을 그대로 내보낸다. 임시 디렉터리 안에서만 동작한다
const script = join(root, 'mark.js')
writeFileSync(script, "require('fs').writeFileSync(process.argv[2], 'x'); process.stdin.pipe(process.stdout)\n")

let repo: string
let marker: string
let n = 0
const git = (...a: string[]): string =>
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], { cwd: repo, encoding: 'utf8' })

/** 저장소를 새로 만들고 표지 filter 를 설정에 심는다(driver: 드라이버 이름) */
function setup(driver = 'evil'): void {
  repo = join(root, `r${n++}`)
  marker = join(root, `MARK${n}`)
  mkdirSync(repo)
  git('init', '-q', '-b', 'main')
  writeFileSync(join(repo, 'a.txt'), 'a\n')
  git('add', '.')
  git('commit', '-q', '-m', 'init')
  git('config', `filter.${driver}.clean`, `node "${fwd(script)}" "${fwd(marker)}"`)
}

afterAll(() => rmSync(root, { recursive: true, force: true }))
beforeEach(() => setup())

describe('--no-index 의 filter 실행 차단', () => {
  it('대조군: 가드 없이 --no-index 를 돌리면 심어 둔 명령이 실행된다', () => {
    writeFileSync(join(repo, '.gitattributes'), '.gitattributes filter=evil\n')
    try {
      git('diff', '--no-index', '--no-ext-diff', '--no-textconv', '--no-color', '--', '/dev/null', '.gitattributes')
    } catch {
      // 차이가 있으면 종료 코드 1 이라 execFileSync 가 던진다 — 표지만 확인한다
    }
    expect(existsSync(marker)).toBe(true)
  })

  it('repoExtras: 작업 트리 .gitattributes 규칙이 filter 에 맞아도 실행하지 않고 LFS 판정은 그대로다', async () => {
    writeFileSync(join(repo, '.gitattributes'), '.gitattributes filter=evil\n*.bin filter=lfs diff=lfs merge=lfs -text\n')
    const ex = await repoExtras(new LocalExecutor(), repo)
    expect(existsSync(marker)).toBe(false)
    expect(ex.lfs).toBe(true)
  })

  it('repoExtras: .git/info/attributes 도 마찬가지다', async () => {
    const info = git('rev-parse', '--path-format=absolute', '--git-path', 'info/attributes').trim()
    writeFileSync(info, `attributes filter=evil\n*.bin filter=lfs\n`)
    const ex = await repoExtras(new LocalExecutor(), repo)
    expect(existsSync(marker)).toBe(false)
    expect(ex.lfs).toBe(true)
  })

  it('repoExtras: 무력화할 수 없는 저장소(이름에 =)는 파일을 읽지 않고 lfs:false 다', async () => {
    setup('a=b')
    writeFileSync(join(repo, '.gitattributes'), '.gitattributes filter=a\n*.bin filter=lfs\n')
    const ex = await repoExtras(new LocalExecutor(), repo)
    expect(existsSync(marker)).toBe(false)
    expect(ex.lfs).toBe(false)
  })

  it('wipFileDiff(untracked): 명령을 실행하지 않고 내용은 그대로 나온다', async () => {
    writeFileSync(join(repo, '.gitattributes'), '*.txt filter=evil\n')
    writeFileSync(join(repo, 'u.txt'), 'hello\n')
    const ex = new LocalExecutor()
    const st = await wipStatus(ex, repo)
    expect(st.untracked).toContain('u.txt')
    const d = await wipFileDiff(ex, repo, { area: 'untracked', path: 'u.txt' }, undefined, new Set(['u.txt']))
    expect(existsSync(marker)).toBe(false)
    expect(d.hunks.length).toBeGreaterThan(0)
    expect(JSON.stringify(d.hunks)).toContain('hello')
  })

  it('wipFileDiff(untracked): 무력화할 수 없는 저장소는 거부한다', async () => {
    setup('a=b')
    writeFileSync(join(repo, '.gitattributes'), '*.txt filter=a\n')
    writeFileSync(join(repo, 'u.txt'), 'hello\n')
    await expect(wipFileDiff(new LocalExecutor(), repo, { area: 'untracked', path: 'u.txt' }, undefined, new Set(['u.txt']))).rejects.toThrow()
    expect(existsSync(marker)).toBe(false)
  })
})
