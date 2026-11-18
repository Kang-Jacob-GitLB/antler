// 악성 저장소 방어(실측으로 재현한 세 건). 표지는 임시 디렉터리 안에만 쓰는 무해한 node 스크립트다.
//  A. 빈 filter 드라이버 이름(`[filter ""]` + `filter=`)은 가드가 덮지 못해 거부한다.
//  B. 서브모듈 inline diff: 상위 저장소 diff.submodule=diff + 인덱스엔 파일·작업 트리엔 중첩 저장소 → 자식 git diff 가 그 저장소의 diff.external 을 실행한다.
//  C. porcelain `git diff` 는 stat 만 바뀐 파일이 있으면 index 를 써서 갱신하고 post-index-change 훅을 실행한다(GIT_OPTIONAL_LOCKS=0 이어도).
// WIP diff(staged·unstaged)는 plumbing(diff-index·diff-files)이라 B·C 가 일어나지 않는다.
import { execFileSync } from 'node:child_process'
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { wipFileDiff, wipStatus } from '../src/main/git/wip'

const fwd = (p: string): string => p.replace(/\\/g, '/')
const exec = new LocalExecutor()

let root: string
let marker: string
let script: string

const gitIn = (cwd: string, ...a: string[]): string =>
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })

/** 크기는 그대로 두고 mtime 만 옮겨 "stat 만 바뀐" 파일을 만든다 */
function touchOnly(file: string): void {
  const t = new Date(Date.now() + 5000)
  utimesSync(file, t, t)
}

function newRepo(name: string, extra: string[] = []): string {
  const dir = join(root, name)
  mkdirSync(dir)
  gitIn(dir, 'init', '-q', '-b', 'main', ...extra)
  return dir
}

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'antler-wipplumbing-'))
  marker = fwd(join(root, 'pwned'))
  script = fwd(join(root, 'mark.js'))
  // 표지를 남기고 stdin 을 그대로 stdout 으로 넘긴다(필터·훅·외부 diff 어디에 꽂아도 무해하다)
  writeFileSync(script, `require('fs').appendFileSync(${JSON.stringify(marker)}, 'x\\n'); process.stdin.resume(); process.stdin.pipe(process.stdout)`)
})

afterAll(() => {
  rmSync(root, { recursive: true, force: true })
})

const marked = (): boolean => existsSync(marker)
const reset = (): void => rmSync(marker, { force: true })

describe('A. 이름이 빈 filter 드라이버', () => {
  let repo: string
  beforeAll(() => {
    repo = newRepo('a')
    writeFileSync(join(repo, 'a.txt'), 'hello\n')
    gitIn(repo, 'add', '.')
    gitIn(repo, 'commit', '-q', '-m', 'init')
    // `git config` 는 빈 서브섹션을 못 쓰는 경우가 있어 파일에 직접 쓴다
    appendFileSync(join(repo, '.git', 'config'), `[filter ""]\n\tclean = node ${script}\n`)
    appendFileSync(join(repo, '.git', 'info', 'attributes'), '* filter=\n')
  })

  it('대조군: 그냥 git status 는 빈 이름 드라이버의 명령을 실행한다', () => {
    reset()
    writeFileSync(join(repo, 'a.txt'), 'jello\n')
    touchOnly(join(repo, 'a.txt'))
    gitIn(repo, 'status', '--porcelain')
    expect(marked()).toBe(true)
    reset()
  })

  it('wipStatus·wipFileDiff 는 실행하지 않고 거부한다', async () => {
    reset()
    writeFileSync(join(repo, 'a.txt'), 'hellp\n')
    touchOnly(join(repo, 'a.txt'))
    await expect(wipStatus(exec, repo)).rejects.toThrow(/filter/)
    await expect(wipFileDiff(exec, repo, { area: 'unstaged', path: 'a.txt' })).rejects.toThrow(/filter/)
    await expect(wipFileDiff(exec, repo, { area: 'staged', path: 'a.txt' })).rejects.toThrow(/filter/)
    expect(marked()).toBe(false)
  })
})

describe('B. 서브모듈 inline diff + 중첩 저장소의 diff.external', () => {
  let repo: string
  beforeAll(() => {
    repo = newRepo('b')
    writeFileSync(join(repo, 'sub'), 'file\n')
    gitIn(repo, 'add', '.')
    gitIn(repo, 'commit', '-q', '-m', 'init')
    // 인덱스에는 파일 `sub`, 작업 트리에는 중첩 저장소(타입 변경)
    rmSync(join(repo, 'sub'))
    const sub = join(repo, 'sub')
    mkdirSync(sub)
    gitIn(sub, 'init', '-q', '-b', 'main')
    writeFileSync(join(sub, 'f.txt'), 'a\n')
    gitIn(sub, 'add', '.')
    gitIn(sub, 'commit', '-q', '-m', 's')
    writeFileSync(join(sub, 'f.txt'), 'b\n')
    appendFileSync(join(sub, '.git', 'config'), `[diff]\n\texternal = node ${script}\n`)
    gitIn(repo, 'config', 'diff.submodule', 'diff')
  })

  it('대조군: 예전 porcelain 호출은 중첩 저장소의 diff.external 을 실행한다', () => {
    reset()
    gitIn(repo, 'diff', '--no-color', '--no-ext-diff', '--no-textconv', '-U3', '--no-renames', '--', 'sub')
    expect(marked()).toBe(true)
    reset()
  })

  it('wipFileDiff(unstaged) 는 실행하지 않는다', async () => {
    reset()
    // status 는 이 경로를 `.T` 로 보고하므로 UI 에서 diff 를 요청할 수 있다
    const st = await wipStatus(exec, repo)
    expect(st.unstaged.map((f) => f.path)).toContain('sub')
    await wipFileDiff(exec, repo, { area: 'unstaged', path: 'sub' })
    expect(marked()).toBe(false)
  })

  it('전역 인자 diff.submodule=short 가 저장소 설정을 덮는다', async () => {
    const r = await exec.run(repo, ['config', '--get', 'diff.submodule'])
    expect(r.stdout.toString().trim()).toBe('short')
    const r2 = await exec.run(repo, ['config', '--get', 'submodule.recurse'])
    expect(r2.stdout.toString().trim()).toBe('false')
  })
})

describe('C. index 쓰기와 post-index-change 훅', () => {
  let repo: string
  const indexBytes = (): Buffer => readFileSync(join(repo, '.git', 'index'))

  beforeAll(() => {
    repo = newRepo('c')
    writeFileSync(join(repo, 'a.txt'), 'hello\n')
    writeFileSync(join(repo, 'b.txt'), 'one\n')
    gitIn(repo, 'add', '.')
    gitIn(repo, 'commit', '-q', '-m', 'init')
    // 리눅스는 실행 비트가 없으면 훅을 건너뛴다(대조군이 재현되지 않는다)
    writeFileSync(join(repo, '.git', 'hooks', 'post-index-change'), `#!/bin/sh\nnode "${script}"\n`, { mode: 0o755 })
  })

  it('대조군: porcelain git diff 는 stat 만 바뀐 파일이 있으면 index 를 쓰고 훅을 실행한다', () => {
    reset()
    touchOnly(join(repo, 'a.txt'))
    const before = indexBytes()
    execFileSync('git', ['-C', repo, 'diff', '--no-color', '--', 'a.txt'], { env: { ...process.env, GIT_OPTIONAL_LOCKS: '0' }, stdio: 'ignore' })
    expect(marked()).toBe(true)
    expect(indexBytes().equals(before)).toBe(false)
    reset()
  })

  it('wipFileDiff(unstaged·staged) 는 index 를 쓰지 않고 훅을 실행하지 않는다', async () => {
    reset()
    // 대조군이 index 를 갱신했으니 다시 stat 만 바뀐 상태로 만든다
    const t = new Date(Date.now() + 9000)
    utimesSync(join(repo, 'a.txt'), t, t)
    const before = indexBytes()
    const d = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'a.txt' })
    await wipFileDiff(exec, repo, { area: 'staged', path: 'a.txt' })
    expect(marked()).toBe(false)
    expect(indexBytes().equals(before)).toBe(true)
    // 내용이 같은 파일은 UI 에서 빈 diff 로 보인다(hunk 없음)
    expect(d.hunks).toEqual([])
    expect(d.binary).toBe(false)
  })

  it('wipStatus 도 index 를 쓰지 않고 훅을 실행하지 않는다(GIT_OPTIONAL_LOCKS=0)', async () => {
    reset()
    const t = new Date(Date.now() + 12000)
    utimesSync(join(repo, 'b.txt'), t, t)
    const before = indexBytes()
    await wipStatus(exec, repo)
    expect(marked()).toBe(false)
    expect(indexBytes().equals(before)).toBe(true)
  })
})

describe('plumbing 으로 바꾼 WIP diff 의 출력', () => {
  let repo: string
  beforeAll(() => {
    repo = newRepo('out')
    writeFileSync(join(repo, 'a.txt'), 'l1\nl2\nl3\n')
    writeFileSync(join(repo, 'old name.txt'), 'x\ny\nz\nw\n')
    gitIn(repo, 'add', '.')
    gitIn(repo, 'commit', '-q', '-m', 'init')
  })

  it('unstaged: 헤더·hunk 가 기존 porcelain 과 같다', async () => {
    writeFileSync(join(repo, 'a.txt'), 'l1\nL2\nl3\n')
    const d = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'a.txt' })
    expect(d.hunks).toHaveLength(1)
    expect(d.hunks[0]!.lines.map((l) => `${l.t}${l.text}`)).toEqual([' l1', '-l2', '+L2', ' l3'])
    expect(d.hunks[0]!.oldStart).toBe(1)
  })

  it('staged: 이름 변경(-M)·삭제·추가', async () => {
    gitIn(repo, 'mv', 'old name.txt', 'new name.txt')
    writeFileSync(join(repo, 'new name.txt'), 'x\ny\nZ\nw\n')
    gitIn(repo, 'add', '-A')
    const ren = await wipFileDiff(exec, repo, { area: 'staged', path: 'new name.txt', oldPath: 'old name.txt' })
    expect(ren.meta.some((m) => m.startsWith('rename from '))).toBe(true)
    expect(ren.hunks[0]!.lines.some((l) => l.t === '+' && l.text === 'Z')).toBe(true)
  })

  it('staged: 커밋이 없는 저장소는 빈 트리와 비교한다(전부 추가)', async () => {
    const empty = newRepo('unborn')
    writeFileSync(join(empty, 'n.txt'), 'a\nb\n')
    gitIn(empty, 'add', 'n.txt')
    const d = await wipFileDiff(exec, empty, { area: 'staged', path: 'n.txt' })
    expect(d.meta).toContain('new file mode 100644')
    expect(d.hunks[0]!.lines.map((l) => l.t)).toEqual(['+', '+'])
  })

  it('staged: SHA-256 저장소도 커밋 전에 동작한다', async () => {
    let empty: string
    try {
      empty = newRepo('unborn256', ['--object-format=sha256'])
    } catch {
      return // 이 git 이 sha256 을 지원하지 않는다
    }
    writeFileSync(join(empty, 'n.txt'), 'a\n')
    gitIn(empty, 'add', 'n.txt')
    const d = await wipFileDiff(exec, empty, { area: 'staged', path: 'n.txt' })
    expect(d.hunks[0]!.lines.map((l) => l.t)).toEqual(['+'])
  })

  it('unstaged: 삭제된 파일·바이너리', async () => {
    rmSync(join(repo, 'a.txt'))
    const del = await wipFileDiff(exec, repo, { area: 'unstaged', path: 'a.txt' })
    expect(del.meta).toContain('deleted file mode 100644')
    expect(del.hunks[0]!.lines.every((l) => l.t === '-')).toBe(true)
  })
})
