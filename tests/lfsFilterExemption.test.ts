// 악성 저장소 방어: git-lfs 기본 명령도 filter 가드가 무력화해야 한다. git-lfs 의 clean 은 저장소 설정의
// `lfs.extension.<이름>.clean` 명령을 실행하고 `.git/lfs/objects` 에 객체를 쓴다(읽기 전용 경계 위반).
// git-lfs 가 없으면 건너뛴다(표지 명령이 git-lfs 안에서 돌아야 의미가 있다).
import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, describe, expect, it } from 'vitest'
import { LocalExecutor } from '../src/main/executors/LocalExecutor'
import { repoExtras } from '../src/main/git/repoExtras'
import { wipFileDiff, wipStatus } from '../src/main/git/wip'

const hasLfs = spawnSync('git', ['lfs', 'version'], { encoding: 'utf8' }).status === 0

const root = mkdtempSync(join(tmpdir(), 'antler-lfsex-'))
const fwd = (p: string): string => p.replace(/\\/g, '/')
// 표지 명령: 표지 파일을 만들고 stdin 을 그대로 내보낸다. 임시 디렉터리 안에서만 동작한다
const script = join(root, 'mark.js')
writeFileSync(script, "require('fs').writeFileSync(process.argv[2], 'x'); process.stdin.pipe(process.stdout)\n")

let n = 0
interface Repo {
  dir: string
  marker: string
  git: (...a: string[]) => string
}

/** git-lfs 를 저장소 설정에 직접 연결한 저장소(`git lfs install --local` 과 같은 값) */
function makeRepo(): Repo {
  const dir = join(root, `r${n++}`)
  const marker = join(root, `MARK${n}`)
  mkdirSync(dir)
  const git = (...a: string[]): string =>
    execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@e.x', '-c', 'commit.gpgsign=false', ...a], { cwd: dir, encoding: 'utf8' })
  git('init', '-q', '-b', 'main')
  git('config', 'core.autocrlf', 'false')
  git('config', 'filter.lfs.clean', 'git-lfs clean -- %f')
  git('config', 'filter.lfs.smudge', 'git-lfs smudge -- %f')
  git('config', 'filter.lfs.process', 'git-lfs filter-process')
  git('config', 'filter.lfs.required', 'true')
  return { dir, marker, git }
}

const lfsDir = (r: Repo): string => join(r.dir, '.git', 'lfs', 'objects')

/** 크기는 같고 mtime 만 옮긴다 — stat 이 달라야 git 이 내용을 해시하려고 clean 을 돈다 */
function touch(file: string, ahead: number): void {
  const t = new Date(Date.now() + ahead)
  utimesSync(file, t, t)
}

afterAll(() => rmSync(root, { recursive: true, force: true }))

describe.skipIf(!hasLfs)('git-lfs 기본 filter 도 실행하지 않는다 (git-lfs 가 없으면 건너뜀)', () => {
  /** 커밋 전에는 확장을 끄고, 심은 뒤의 상태를 돌려준다 — 공격은 커밋된 저장소의 설정에만 있다 */
  function attackRepo(): Repo {
    const r = makeRepo()
    writeFileSync(join(r.dir, '.gitattributes'), '.gitattributes filter=lfs\n*.bin filter=lfs diff=lfs merge=lfs -text\n')
    writeFileSync(join(r.dir, 'a.bin'), 'payload-1\n')
    r.git('add', '.')
    r.git('commit', '-q', '-m', 'init')
    rmSync(join(r.dir, '.git', 'lfs'), { recursive: true, force: true })
    // git-lfs 는 명령을 직접 쪼개 실행하므로 따옴표를 쓰지 않는다(임시 경로에는 공백이 없다)
    r.git('config', 'lfs.extension.x.clean', `node ${fwd(script)} ${fwd(r.marker)}`)
    r.git('config', 'lfs.extension.x.smudge', 'cat')
    r.git('config', 'lfs.extension.x.priority', '0')
    return r
  }

  it('대조군: 가드 없이 --no-index 를 돌리면 lfs.extension 명령이 실행되고 .git/lfs 가 생긴다', () => {
    const r = attackRepo()
    spawnSync('git', ['diff', '--no-index', '--no-ext-diff', '--no-textconv', '--no-color', '--', '/dev/null', '.gitattributes'], { cwd: r.dir })
    expect(existsSync(r.marker)).toBe(true)
    expect(existsSync(lfsDir(r))).toBe(true)
  })

  it('repoExtras: 표지도 .git/lfs 도 만들지 않고 LFS 판정은 그대로다', async () => {
    const r = attackRepo()
    const ex = await repoExtras(new LocalExecutor(), r.dir)
    expect(existsSync(r.marker)).toBe(false)
    expect(existsSync(join(r.dir, '.git', 'lfs'))).toBe(false)
    expect(ex.lfs).toBe(true)
  })

  it('wipStatus·wipFileDiff(unstaged·untracked): 표지도 .git/lfs 도 만들지 않는다', async () => {
    const r = attackRepo()
    const ex = new LocalExecutor()
    // 같은 크기로 내용을 바꾸고 mtime 을 옮긴다(해시하려면 clean 이 돈다)
    writeFileSync(join(r.dir, 'a.bin'), 'payload-2\n')
    touch(join(r.dir, 'a.bin'), 5000)
    writeFileSync(join(r.dir, 'u.bin'), 'new\n')
    writeFileSync(join(r.dir, '.gitattributes'), '.gitattributes filter=lfs\n*.bin filter=lfs diff=lfs merge=lfs -text\n# edit\n')
    const st = await wipStatus(ex, r.dir)
    expect(st.unstaged.map((f) => f.path)).toContain('a.bin')
    await wipFileDiff(ex, r.dir, { area: 'unstaged', path: 'a.bin' })
    await wipFileDiff(ex, r.dir, { area: 'unstaged', path: '.gitattributes' })
    await wipFileDiff(ex, r.dir, { area: 'untracked', path: 'u.bin' }, undefined, new Set(st.untracked))
    expect(existsSync(r.marker)).toBe(false)
    expect(existsSync(join(r.dir, '.git', 'lfs'))).toBe(false)
  })
})

describe.skipIf(!hasLfs)('화면 영향: LFS 저장소의 작업 중 변경 (git-lfs 가 없으면 건너뜀)', () => {
  /** LFS 로 커밋한 뒤 파일을 지우고 다시 체크아웃한다 — 체크아웃 직후 수정 없는 LFS 파일 */
  function checkedOut(): Repo {
    const r = makeRepo()
    writeFileSync(join(r.dir, '.gitattributes'), '*.bin filter=lfs diff=lfs merge=lfs -text\n')
    writeFileSync(join(r.dir, 'big.bin'), 'binary-ish payload\n')
    r.git('add', '.')
    r.git('commit', '-q', '-m', 'init')
    rmSync(join(r.dir, 'big.bin'))
    r.git('checkout', '--', 'big.bin')
    return r
  }

  it('체크아웃 직후 수정 없는 LFS 파일은 stat 캐시 덕분에 수정됨으로 보이지 않는다', async () => {
    const r = checkedOut()
    // 실사용처럼 파일이 인덱스보다 먼저 쓰인 상태로 만든다. 같은 타임스탬프 단위에 쓰이면(racy-git) git 이 내용을 다시 해시하는데,
    // 필터를 끈 Antler 는 원본과 포인터를 비교해 수정됨으로 본다 — 체크아웃 직후 같은 순간에만 생기는 알려진 한계다(CI 리눅스에서 관찰)
    touch(join(r.dir, 'big.bin'), -10_000)
    r.git('status', '--porcelain') // 인덱스 stat 갱신(실사용에서도 직전 status 가 있다)
    const st = await wipStatus(new LocalExecutor(), r.dir)
    expect(st.staged).toEqual([])
    expect(st.unstaged).toEqual([])
    expect(st.untracked).toEqual([])
  })

  it('stat 만 바뀐 LFS 파일은 포인터가 아닌 원본과 비교돼 수정됨으로 보이고, diff 는 포인터가 아니라 실제 내용이다(알려진 한계)', async () => {
    const r = checkedOut()
    touch(join(r.dir, 'big.bin'), 7000)
    const ex = new LocalExecutor()
    const st = await wipStatus(ex, r.dir)
    expect(st.unstaged.map((f) => f.path)).toEqual(['big.bin'])
    const d = await wipFileDiff(ex, r.dir, { area: 'unstaged', path: 'big.bin' })
    const text = JSON.stringify(d)
    expect(text).toContain('binary-ish payload')
  })
})
